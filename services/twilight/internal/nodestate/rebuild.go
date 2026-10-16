package nodestate

import (
	"context"
	"fmt"
	"log/slog"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

var rebuilds = promauto.NewCounter(prometheus.CounterOpts{
	Name: "twilight_node_state_rebuilds_total",
	Help: "Times this process wrote the node-state topic again from the inventory.",
})

type Producer interface {
	ProduceNodeState(operation context.Context, state kafka.NodeState) error
	ClearNodeState(operation context.Context, key string) error
	ProduceNodeStateComplete(operation context.Context, key string, complete kafka.NodeStateComplete) (int32, error)
	RefreshMetadata()
}

type Rebuild struct {
	Options   []kgo.Opt
	Producer  Producer
	Inventory *inventory.Store
	Validator *kafka.Validator
	Topic     string
	Actor     string
	Workers   int
	Stall     time.Duration
	Logger    *slog.Logger
}

type Report struct {
	Checked     bool
	Partitions  int32
	Incomplete  []int32
	Rebuilt     bool
	InSync      int
	Published   int
	WrittenBack int
	Moved       int
	Unknown     int
}

type action int

const (
	inSync action = iota
	publish
	writeBack
	unknownNode
)

func effective(lifecycle string) string {
	switch lifecycle {
	case "", "enrolled", "active":
		return ""
	}
	return lifecycle
}

func decide(topic *Entry, current inventory.LifecycleState) action {
	held := effective(current.Lifecycle)
	if topic == nil {
		if held == "" {
			return inSync
		}
		return publish
	}
	if effective(topic.Lifecycle()) == held {
		return inSync
	}
	if current.Lifecycle == "" && !current.DeviceScope() {
		return unknownNode
	}
	var changed time.Time
	if current.ChangedAt != nil {
		changed = *current.ChangedAt
	}
	if topic.Time().After(changed) {
		return writeBack
	}
	return publish
}

func Key(device, installation string) string {
	if installation == "" {
		return "device/" + device
	}
	return "installation/" + device + "/" + installation
}

func scope(key string) (string, string) {
	parts := strings.Split(key, "/")
	if parts[0] == "device" {
		return parts[1], ""
	}
	return parts[1], parts[2]
}

func partitionOf(key string, partitions int32) int32 {
	return int32(kgo.StickyKeyPartitioner(nil).ForTopic("").Partition(&kgo.Record{Key: []byte(key)}, int(partitions)))
}

func CompleteKeys(partitions int32) []string {
	keys := make([]string, partitions)
	for index, found := 0, int32(0); found < partitions; index++ {
		key := kafka.NodeStateCompletePrefix + strconv.Itoa(index)
		partition := partitionOf(key, partitions)
		if keys[partition] == "" {
			keys[partition] = key
			found++
		}
	}
	return keys
}

func (rebuild Rebuild) TopicIdentity(operation context.Context) (string, error) {
	return Identity(operation, rebuild.Options, rebuild.Topic)
}

func (rebuild Rebuild) actorOf(actor *string) string {
	if actor != nil && *actor != "" {
		return *actor
	}
	return rebuild.Actor
}

func (rebuild Rebuild) publish(operation context.Context, current inventory.LifecycleState) error {
	producing, cancel := context.WithTimeout(operation, 10*time.Second)
	defer cancel()
	at := time.Now()
	if current.ChangedAt != nil {
		at = *current.ChangedAt
	}
	if current.DeviceScope() {
		if effective(current.Lifecycle) == "" {
			return rebuild.Producer.ClearNodeState(producing, Key(current.DeviceID, ""))
		}
		return rebuild.Producer.ProduceNodeState(producing, kafka.NewDeviceState(current.DeviceID, current.Lifecycle, current.Reason, rebuild.actorOf(current.Actor), at))
	}
	return rebuild.Producer.ProduceNodeState(producing, kafka.NewInstallationState(current.DeviceID, current.InstallationID, current.Lifecycle, current.Reason, rebuild.actorOf(current.Actor), at))
}

func (rebuild Rebuild) restored(current inventory.LifecycleState, topic *Entry) *inventory.LifecycleState {
	lifecycle := topic.Lifecycle()
	if lifecycle == "" {
		lifecycle = "active"
	}
	at := topic.Time()
	actor := topic.Actor()
	if actor == "" {
		actor = rebuild.Actor
	}
	return &inventory.LifecycleState{DeviceID: current.DeviceID, InstallationID: current.InstallationID, Lifecycle: lifecycle, Reason: topic.Reason(), Actor: &actor, ChangedAt: &at}
}

type tally struct {
	mutex  sync.Mutex
	report *Report
}

func (counts *tally) add(taken action) {
	counts.mutex.Lock()
	defer counts.mutex.Unlock()
	switch taken {
	case inSync:
		counts.report.InSync++
	case publish:
		counts.report.Published++
	case writeBack:
		counts.report.WrittenBack++
	case unknownNode:
		counts.report.Unknown++
	}
}

func (rebuild Rebuild) resolve(operation context.Context, key string, topic *Entry, counts *tally) error {
	device, installation := scope(key)
	var taken action
	failure := rebuild.Inventory.ApplyLifecycle(operation, device, installation, func(current inventory.LifecycleState) (*inventory.LifecycleState, error) {
		taken = decide(topic, current)
		switch taken {
		case publish:
			if failure := rebuild.publish(operation, current); failure != nil {
				return nil, fmt.Errorf("publish %s: %w", key, failure)
			}
			if topic != nil {
				topic.published = true
			}
			rebuild.Logger.Debug("published a node state from the inventory", "key", key, "lifecycle", current.Lifecycle)
		case writeBack:
			replacement := rebuild.restored(current, topic)
			rebuild.Logger.Warn("the node-state topic holds a later lifecycle than the inventory; it is written into the inventory",
				"key", key, "inventory_lifecycle", current.Lifecycle, "topic_lifecycle", replacement.Lifecycle, "topic_time", kafka.FormatTime(topic.Time()), "actor", *replacement.Actor)
			return replacement, nil
		case unknownNode:
			rebuild.Logger.Warn("the node-state topic holds a lifecycle for a node the inventory does not hold; it stays on the topic",
				"key", key, "topic_lifecycle", topic.Lifecycle(), "topic_time", kafka.FormatTime(topic.Time()), "actor", topic.Actor())
		}
		return nil, nil
	})
	if failure != nil {
		return failure
	}
	counts.add(taken)
	return nil
}

type pool struct {
	work    chan func() error
	cancel  context.CancelFunc
	waiting sync.WaitGroup
	once    sync.Once
	failure error
}

func newPool(operation context.Context, workers int) (*pool, context.Context) {
	working, cancel := context.WithCancel(operation)
	running := &pool{work: make(chan func() error), cancel: cancel}
	for range max(workers, 1) {
		running.waiting.Add(1)
		go func() {
			defer running.waiting.Done()
			for task := range running.work {
				if working.Err() != nil {
					continue
				}
				if failure := task(); failure != nil {
					running.once.Do(func() {
						running.failure = failure
						cancel()
					})
				}
			}
		}()
	}
	return running, working
}

func (running *pool) submit(working context.Context, task func() error) bool {
	select {
	case running.work <- task:
		return true
	case <-working.Done():
		return false
	}
}

func (running *pool) wait() error {
	close(running.work)
	running.waiting.Wait()
	running.cancel()
	return running.failure
}

func (rebuild Rebuild) Run(operation context.Context, onlyIfIncomplete bool) (Report, error) {
	started := time.Now()
	var report Report
	if onlyIfIncomplete {
		checked, failure := Read(operation, rebuild.Options, rebuild.Topic, rebuild.Validator, rebuild.Logger, false, rebuild.Stall)
		if failure != nil {
			return report, fmt.Errorf("read %s: %w", rebuild.Topic, failure)
		}
		report = Report{Checked: true, Partitions: checked.Partitions, Incomplete: checked.Incomplete()}
		if len(report.Incomplete) == 0 {
			return report, nil
		}
	}
	snapshot, failure := Read(operation, rebuild.Options, rebuild.Topic, rebuild.Validator, rebuild.Logger, true, rebuild.Stall)
	if failure != nil {
		return report, fmt.Errorf("read %s: %w", rebuild.Topic, failure)
	}
	report = Report{Checked: true, Partitions: snapshot.Partitions, Incomplete: snapshot.Incomplete()}
	rebuild.Logger.Info("rebuilding the node-state topic from the inventory", "topic", rebuild.Topic, "partitions", snapshot.Partitions,
		"incomplete_partitions", report.Incomplete, "topic_states", len(snapshot.States), "actor", rebuild.Actor)
	counts := &tally{report: &report}
	running, working := newPool(operation, rebuild.Workers)
	visit := rebuild.Inventory.EachWithdrawnOrQuarantined(working, func(current inventory.LifecycleState) error {
		key := Key(current.DeviceID, current.InstallationID)
		topic := snapshot.States[key]
		if topic != nil {
			topic.visited = true
		}
		if decide(topic, current) == inSync {
			counts.add(inSync)
			return nil
		}
		if !running.submit(working, func() error { return rebuild.resolve(working, key, topic, counts) }) {
			return working.Err()
		}
		return nil
	})
	if visit == nil {
		for key, topic := range snapshot.States {
			if topic.visited || effective(topic.Lifecycle()) == "" {
				continue
			}
			if !running.submit(working, func() error { return rebuild.resolve(working, key, topic, counts) }) {
				break
			}
		}
	}
	if failure := running.wait(); failure != nil {
		return report, failure
	}
	if visit != nil {
		return report, fmt.Errorf("read the inventory's lifecycles: %w", visit)
	}
	rebuild.Producer.RefreshMetadata()
	for partition, key := range CompleteKeys(snapshot.Partitions) {
		producing, cancel := context.WithTimeout(operation, 10*time.Second)
		landed, failure := rebuild.Producer.ProduceNodeStateComplete(producing, key, kafka.NewNodeStateComplete(rebuild.Actor, time.Now()))
		cancel()
		if failure != nil {
			return report, fmt.Errorf("write the completion record %s: %w", key, failure)
		}
		if landed != int32(partition) {
			return report, fmt.Errorf("the completion record %s landed on partition %d, not %d: the producer does not yet see the topic's %d partitions", key, landed, partition, snapshot.Partitions)
		}
	}
	report.Rebuilt = true
	rebuilds.Inc()
	moved, failure := rebuild.republishMoved(operation, snapshot)
	report.Moved = moved
	if failure != nil {
		return report, failure
	}
	rebuild.Logger.Info("the node-state topic is rebuilt from the inventory", "topic", rebuild.Topic, "partitions", snapshot.Partitions,
		"in_sync", report.InSync, "published", report.Published, "written_back", report.WrittenBack, "unknown_nodes", report.Unknown,
		"moved", report.Moved, "seconds", time.Since(started).Seconds(), "actor", rebuild.Actor)
	return report, nil
}

func (rebuild Rebuild) republishMoved(operation context.Context, snapshot Snapshot) (int, error) {
	moved := 0
	for key, entry := range snapshot.States {
		if entry.published || partitionOf(key, snapshot.Partitions) == entry.partition {
			continue
		}
		device, installation := scope(key)
		producing, cancel := context.WithTimeout(operation, 10*time.Second)
		var failure error
		switch {
		case entry.lifecycle == 0:
			failure = rebuild.Producer.ClearNodeState(producing, key)
		case installation == "":
			failure = rebuild.Producer.ProduceNodeState(producing, kafka.NewDeviceState(device, entry.Lifecycle(), entry.reason, rebuild.actorOf(&entry.actor), entry.Time()))
		default:
			failure = rebuild.Producer.ProduceNodeState(producing, kafka.NewInstallationState(device, installation, entry.Lifecycle(), entry.reason, rebuild.actorOf(&entry.actor), entry.Time()))
		}
		cancel()
		if failure != nil {
			return moved, fmt.Errorf("publish %s on its current partition: %w", key, failure)
		}
		moved++
	}
	if moved > 0 {
		rebuild.Logger.Info("republished node states that sat on a partition their key no longer hashes to", "topic", rebuild.Topic, "partitions", snapshot.Partitions, "moved", moved)
	}
	return moved, nil
}
