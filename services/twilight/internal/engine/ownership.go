package engine

import (
	"context"
	"errors"
	"math/rand/v2"
	"slices"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/kafka"
)

const (
	sliceSessionTimeout    = 10 * time.Second
	sliceHeartbeatInterval = time.Second
)

var (
	slicesOwnedGauge = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_slices_owned",
		Help: "Partitions of the process results topic this instance owns under a lease.",
	})
	slicePartitionMismatches = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_slice_partition_mismatches_total",
		Help: "Process results that arrived in a partition other than the one their node's key hashes to.",
	})
	sliceLeaseLosses = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_slice_lease_losses_total",
		Help: "Slices this instance stopped owning without giving them up, by how: lost (the consumer group dropped this instance) or fenced (another instance took the lease).",
	}, []string{"how"})
)

var errNotOwned = errors.New("this instance does not own the node's slice")

type slice struct {
	lease   campaign.Lease
	writers sync.RWMutex
	revoked atomic.Bool
}

type pendingLease struct {
	since     time.Time
	retrying  bool
	escalated bool
	cancel    context.CancelFunc
}

type ownership struct {
	engine     *Engine
	mutex      sync.Mutex
	settled    *sync.Cond
	partitions int32
	slices     map[int32]*slice
	pending    map[int32]*pendingLease
	client     *kgo.Client
}

func newOwnership(engine *Engine) *ownership {
	current := &ownership{engine: engine, slices: map[int32]*slice{}, pending: map[int32]*pendingLease{}}
	current.settled = sync.NewCond(&current.mutex)
	return current
}

func (current *ownership) topic() string {
	return current.engine.Config.Kafka.Topics.ProcessResults
}

func (current *ownership) partitionOf(key NodeKey) (int32, bool) {
	current.mutex.Lock()
	partitions := current.partitions
	current.mutex.Unlock()
	if partitions == 0 {
		return 0, false
	}
	return kafka.KeyPartition(key.String(), partitions), true
}

func (current *ownership) leases() []campaign.Lease {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	held := make([]campaign.Lease, 0, len(current.slices))
	for _, owned := range current.slices {
		if !owned.revoked.Load() {
			held = append(held, owned.lease)
		}
	}
	return held
}

func (current *ownership) partitionCount() int32 {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	return current.partitions
}

func (current *ownership) leaseOf(partition int32) (campaign.Lease, bool) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	owned := current.slices[partition]
	if owned == nil || owned.revoked.Load() {
		return campaign.Lease{}, false
	}
	return owned.lease, true
}

func (current *ownership) hold(key NodeKey) (campaign.Lease, func(), bool) {
	partition, known := current.partitionOf(key)
	if !known {
		return campaign.Lease{}, nil, false
	}
	current.mutex.Lock()
	owned := current.slices[partition]
	current.mutex.Unlock()
	if owned == nil {
		return campaign.Lease{}, nil, false
	}
	owned.writers.RLock()
	if owned.revoked.Load() {
		owned.writers.RUnlock()
		return campaign.Lease{}, nil, false
	}
	return owned.lease, owned.writers.RUnlock, true
}

func (current *ownership) owns(key NodeKey) bool {
	_, release, owned := current.hold(key)
	if owned {
		release()
	}
	return owned
}

func (current *ownership) ownedPartitions() []int32 {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	owned := make([]int32, 0, len(current.slices))
	for partition := range current.slices {
		owned = append(owned, partition)
	}
	slices.Sort(owned)
	return owned
}

func (current *ownership) owning() bool {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	return len(current.slices) > 0
}

func (current *ownership) own(lease campaign.Lease) {
	current.mutex.Lock()
	current.slices[lease.Partition] = &slice{lease: lease}
	delete(current.pending, lease.Partition)
	owned := len(current.slices)
	current.mutex.Unlock()
	slicesOwnedGauge.Set(float64(owned))
	current.engine.registry.requestRecount()
	current.engine.presence.requestReconcile(lease.Partition)
}

func (current *ownership) share() float64 {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if current.partitions == 0 {
		return 0
	}
	return float64(len(current.slices)) / float64(current.partitions)
}

func (current *ownership) drop(partition int32, drain bool) *slice {
	current.mutex.Lock()
	owned := current.slices[partition]
	delete(current.slices, partition)
	count := len(current.slices)
	partitions := current.partitions
	current.mutex.Unlock()
	slicesOwnedGauge.Set(float64(count))
	if owned == nil {
		return nil
	}
	owned.revoked.Store(true)
	if drain {
		owned.writers.Lock()
		owned.writers.Unlock()
	}
	current.engine.dispatcher.dropPartition(partition, partitions)
	current.engine.registry.forget(partition)
	return owned
}

func (current *ownership) load(operation context.Context) bool {
	backoff := time.Second
	for {
		loading, cancel := context.WithTimeout(operation, 15*time.Second)
		partitions, failure := database.NodePartitionCount(loading, current.engine.Pool)
		cancel()
		if failure == nil && partitions > 0 {
			current.mutex.Lock()
			current.partitions = partitions
			current.mutex.Unlock()
			return true
		}
		if operation.Err() != nil {
			return false
		}
		current.engine.Logger.Warn("the inventory's partition count was not read; slices wait for it", "partitions", partitions, "error", failure)
		if !sleep(operation, jitter(backoff)) {
			return false
		}
		backoff = min(2*backoff, 30*time.Second)
	}
}

func (current *ownership) run(operation context.Context) {
	if !current.load(operation) {
		return
	}
	options := append(append([]kgo.Opt(nil), current.engine.KafkaOptions...),
		kgo.SessionTimeout(sliceSessionTimeout),
		kgo.HeartbeatInterval(sliceHeartbeatInterval),
		kgo.OnPartitionsAssigned(current.assigned),
		kgo.OnPartitionsRevoked(current.revoked),
		kgo.OnPartitionsLost(current.lost),
	)
	var running sync.WaitGroup
	running.Add(1)
	go func() {
		defer running.Done()
		current.retryUnleased(operation)
	}()
	kafka.ConsumeGroup(operation, options, "slices", current.engine.Config.Kafka.ResultsGroup, current.topic(), current.engine.Logger, current.apply)
	running.Wait()
	current.mutex.Lock()
	partitions := make([]int32, 0, len(current.slices))
	for partition := range current.slices {
		partitions = append(partitions, partition)
	}
	current.pending = map[int32]*pendingLease{}
	current.client = nil
	current.mutex.Unlock()
	for _, partition := range partitions {
		if owned := current.drop(partition, true); owned != nil {
			current.engine.Logger.Info("slice given up", "partition", partition, "lease_epoch", owned.lease.Epoch)
		}
	}
}

func (current *ownership) raise(operation context.Context, partition int32) (campaign.Lease, int, error) {
	bumping, cancel := context.WithTimeout(operation, 15*time.Second)
	lease, failure := current.engine.Campaigns.BumpLease(bumping, partition, current.engine.Config.Instance)
	cancel()
	if failure != nil {
		return lease, 0, failure
	}
	return lease, current.restoreInFlight(operation, lease), nil
}

func (current *ownership) restoreInFlight(operation context.Context, lease campaign.Lease) int {
	reading, cancel := context.WithTimeout(operation, 30*time.Second)
	rows, failure := current.engine.Campaigns.InFlightRows(reading, lease.Partition, time.Now())
	cancel()
	if failure != nil {
		if operation.Err() == nil {
			current.engine.Logger.Warn("the in-flight processes of a slice were not read; its nodes may be dispatched to before their results arrive",
				"partition", lease.Partition, "lease_epoch", lease.Epoch, "error", failure)
		}
		return 0
	}
	for _, row := range rows {
		current.engine.dispatcher.restore(NodeKey{DeviceID: row.DeviceID, InstallationID: row.InstallationID}, row.Pid, *row.DeadlineAt)
	}
	return len(rows)
}

func (current *ownership) assigned(operation context.Context, client *kgo.Client, assigned map[string][]int32) {
	current.mutex.Lock()
	current.client = client
	current.mutex.Unlock()
	partitions := assigned[current.topic()]
	sort.Slice(partitions, func(left, right int) bool { return partitions[left] < partitions[right] })
	var unleased []int32
	for _, partition := range partitions {
		lease, restored, failure := current.raise(operation, partition)
		if failure != nil {
			current.mutex.Lock()
			current.pending[partition] = &pendingLease{since: time.Now()}
			current.mutex.Unlock()
			unleased = append(unleased, partition)
			if operation.Err() == nil {
				current.engine.Logger.Warn("a slice was assigned but its lease was not taken; its results wait until it is", "partition", partition, "error", failure)
			}
			continue
		}
		current.own(lease)
		current.engine.Logger.Info("slice owned", "partition", partition, "lease_epoch", lease.Epoch, "in_flight", restored)
	}
	if len(unleased) > 0 {
		client.PauseFetchPartitions(map[string][]int32{current.topic(): unleased})
	}
	if len(partitions) > len(unleased) {
		current.engine.requestSweep(sweepRequest{all: true})
	}
}

func (current *ownership) revoked(_ context.Context, client *kgo.Client, revoked map[string][]int32) {
	current.release(client, revoked[current.topic()], true)
}

func (current *ownership) lost(_ context.Context, client *kgo.Client, lost map[string][]int32) {
	current.release(client, lost[current.topic()], false)
}

func (current *ownership) release(client *kgo.Client, partitions []int32, drain bool) {
	if len(partitions) > 0 {
		client.ResumeFetchPartitions(map[string][]int32{current.topic(): partitions})
	}
	for _, partition := range partitions {
		current.mutex.Lock()
		if waiting := current.pending[partition]; waiting != nil {
			delete(current.pending, partition)
			if !drain && waiting.cancel != nil {
				waiting.cancel()
			}
			for waiting.retrying {
				current.settled.Wait()
			}
		}
		current.mutex.Unlock()
		owned := current.drop(partition, drain)
		switch {
		case owned == nil:
		case drain:
			current.engine.Logger.Info("slice given up", "partition", partition, "lease_epoch", owned.lease.Epoch)
		default:
			sliceLeaseLosses.WithLabelValues("lost").Inc()
			current.engine.Logger.Warn("slice lost: the consumer group dropped this instance; its rows are left to the next owner", "partition", partition, "lease_epoch", owned.lease.Epoch)
		}
	}
}

func (current *ownership) fenced(lease campaign.Lease) {
	current.mutex.Lock()
	owned := current.slices[lease.Partition]
	client := current.client
	current.mutex.Unlock()
	if owned == nil || owned.lease != lease {
		return
	}
	if current.drop(lease.Partition, false) == nil {
		return
	}
	if client != nil {
		client.PauseFetchPartitions(map[string][]int32{current.topic(): {lease.Partition}})
	}
	sliceLeaseLosses.WithLabelValues("fenced").Inc()
	current.engine.Logger.Error("another instance took the lease of a slice this instance owned; it writes none of its rows until the group assigns it here again",
		"partition", lease.Partition, "lease_epoch", lease.Epoch)
}

func (current *ownership) retryUnleased(operation context.Context) {
	backoff := time.Second
	for {
		current.mutex.Lock()
		client := current.client
		partitions := make([]int32, 0, len(current.pending))
		for partition := range current.pending {
			partitions = append(partitions, partition)
		}
		current.mutex.Unlock()
		sort.Slice(partitions, func(left, right int) bool { return partitions[left] < partitions[right] })
		attempted := 0
		var acquired []int32
		for _, partition := range partitions {
			current.mutex.Lock()
			waiting := current.pending[partition]
			if waiting == nil || waiting.retrying {
				current.mutex.Unlock()
				continue
			}
			attempt, cancel := context.WithCancel(operation)
			waiting.retrying, waiting.cancel = true, cancel
			current.mutex.Unlock()
			attempted++
			lease, restored, failure := current.raise(attempt, partition)
			cancel()
			current.mutex.Lock()
			assigned := current.pending[partition] == waiting
			escalate := failure != nil && assigned && !waiting.escalated && time.Since(waiting.since) >= time.Minute
			waiting.escalated = waiting.escalated || escalate
			current.mutex.Unlock()
			switch {
			case failure == nil && assigned:
				current.own(lease)
				acquired = append(acquired, partition)
				current.engine.Logger.Info("slice owned", "partition", partition, "lease_epoch", lease.Epoch, "in_flight", restored)
			case escalate:
				current.engine.Logger.Error("the lease of a slice assigned to this instance has not been taken for over a minute; its results wait until it is",
					"partition", partition, "failing_since", waiting.since, "error", failure)
			case failure != nil && !assigned && lease.Epoch != 0:
				current.engine.Logger.Error("a lease raise of a slice this instance gave up committed before it could be cancelled; the slice's next owner finds its writes refused until it takes the lease again",
					"partition", partition, "lease_epoch", lease.Epoch, "error", failure)
			}
			current.mutex.Lock()
			waiting.retrying, waiting.cancel = false, nil
			current.settled.Broadcast()
			current.mutex.Unlock()
		}
		if len(acquired) > 0 && client != nil {
			client.ResumeFetchPartitions(map[string][]int32{current.topic(): acquired})
			current.engine.requestSweep(sweepRequest{all: true})
		}
		delay := time.Second
		if attempted > len(acquired) {
			delay = time.Duration(rand.Int64N(int64(backoff))) + 1
			backoff = min(2*backoff, time.Minute)
		} else {
			backoff = time.Second
		}
		if !sleep(operation, delay) {
			return
		}
	}
}

func (current *ownership) apply(operation context.Context, record *kgo.Record) error {
	if record.Value == nil {
		return nil
	}
	result, valid := kafka.Decode[kafka.ProcessResult](current.engine.Validator, kafka.ContractProcessResults, record, current.engine.Logger)
	if !valid {
		return nil
	}
	pid, failure := campaign.ParsePid(result.Pid)
	if failure != nil || pid == 0 {
		kafka.CountInvalid(record.Topic)
		current.engine.Logger.Warn("dropped a process result whose pid names no process", "topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "pid", result.Pid)
		return nil
	}
	key := NodeKey{DeviceID: result.DeviceID, InstallationID: result.InstallationID}
	partition, _ := current.partitionOf(key)
	misplaced := partition != record.Partition
	if misplaced {
		slicePartitionMismatches.Inc()
		current.engine.Logger.Error("a process result arrived in a partition other than its node's; its producer does not partition like Kafka's default partitioner",
			"topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "node_partition", partition,
			"device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String())
	}
	failure = current.engine.applyResult(operation, pid, result)
	if !errors.Is(failure, errNotOwned) {
		return failure
	}
	if misplaced {
		current.engine.Logger.Error("a misplaced process result was dropped: this instance does not own its node's slice",
			"topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "node_partition", partition, "pid", pid.String())
		return nil
	}
	return errors.Join(kafka.ErrStopPartition, failure)
}
