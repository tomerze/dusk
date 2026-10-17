//go:build integration

package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

func currentTerm(test *testing.T, pool *pgxpool.Pool) int64 {
	test.Helper()
	var term int64
	if failure := pool.QueryRow(context.Background(), `select term from leadership where id = 1`).Scan(&term); failure != nil {
		test.Fatal(failure)
	}
	return term
}

func ownEverySlice(test *testing.T, owner *Engine) {
	test.Helper()
	operation := context.Background()
	if !owner.ownership.load(operation) {
		test.Fatal("the inventory's partition count was not read")
	}
	for partition := range owner.ownership.partitions {
		lease, failure := owner.Campaigns.BumpLease(operation, partition, owner.Config.Instance)
		if failure != nil {
			test.Fatal(failure)
		}
		owner.ownership.own(lease)
	}
}

func startedRunScript(test *testing.T, store *campaign.Store, name string) *campaign.Campaign {
	test.Helper()
	operation := context.Background()
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, NodeTimeoutSeconds: 600, Gates: campaign.Gates{MinSample: 1}}
	created, failure := store.Create(operation, campaign.Definition{Name: name, Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy},
		"operator@example.org", time.Now().Add(-time.Hour))
	if failure != nil {
		test.Fatal(failure)
	}
	started, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: time.Now().Add(-time.Hour)})
	if failure != nil {
		test.Fatal(failure)
	}
	return started
}

func sliceNode(index int) NodeKey {
	return NodeKey{DeviceID: fmt.Sprintf("%032x", 0x5e0000+index), InstallationID: fmt.Sprintf("%032x", 0x1e0000+index)}
}

func TestAFencedOwnerWritesNothingAndGivesTheSliceUp(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	owner := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Logger: testsupport.Logger()})
	ownEverySlice(test, owner)
	target := startedRunScript(test, store, "fenced")
	key := sliceNode(1)
	inserted, applied, failure := owner.write(operation, nil, campaign.NewRow(target.ID, key.DeviceID, key.InstallationID, 0, campaign.StatePending), "test")
	if failure != nil || !applied {
		test.Fatalf("the owner's insert: %v %v", applied, failure)
	}
	pid := campaign.DerivePid(target.ID, key.DeviceID, key.InstallationID, 1)
	dispatched, _ := inserted.Dispatch(target.ID, campaign.Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 3}, time.Now(), time.Now().Add(time.Hour), nil)
	dispatched, applied, failure = owner.write(operation, &inserted, dispatched, "test")
	if failure != nil || !applied || dispatched.Pid != pid {
		test.Fatalf("the owner's update: %+v %v %v", dispatched, applied, failure)
	}
	partition, _ := owner.ownership.partitionOf(key)
	taken, failure := store.BumpLease(operation, partition, "twilight-elsewhere")
	if failure != nil {
		test.Fatal(failure)
	}
	before := leaseLosses(test, "fenced")
	succeeded, _, _ := dispatched.ApplyResult(target, campaign.Result{Pid: pid, Status: campaign.ResultSucceeded, Delivered: true}, time.Now(), 0)
	if _, applied, failure := owner.write(operation, &dispatched, succeeded, "test"); failure != nil || applied {
		test.Fatalf("a write under a replaced lease: %v %v", applied, failure)
	}
	result := kafka.ProcessResult{Pid: pid.String(), CampaignID: new(target.ID.String()), DeviceID: key.DeviceID, InstallationID: key.InstallationID, NamespaceID: "5d2e9a1c7b3f8e04",
		ActionKind: string(campaign.KindRunScript), Status: string(campaign.ResultSucceeded), Delivered: true}
	if failure := owner.applyResult(operation, pid, result); !errors.Is(failure, errNotOwned) {
		test.Fatalf("a result applied under a replaced lease: %v", failure)
	}
	if owner.ownership.owns(key) {
		test.Fatal("the fenced owner still believes it owns the slice")
	}
	if losses := leaseLosses(test, "fenced") - before; losses != 1 {
		test.Fatalf("%v fenced slices counted, want 1", losses)
	}
	stored, failure := store.Row(operation, target.ID, key.DeviceID, key.InstallationID)
	if failure != nil || stored.State != campaign.StateDispatching || stored.Revision != dispatched.Revision {
		test.Fatalf("the row after the fenced owner's attempts: %+v %v", stored, failure)
	}
	if current, failure := store.CurrentLease(operation, key.DeviceID, key.InstallationID); failure != nil || current != taken {
		test.Fatalf("the current lease %+v (%v), want the new owner's %+v", current, failure, taken)
	}
	if _, applied, failure := owner.writeUnder(operation, taken, &dispatched, succeeded, "test"); failure != nil || !applied {
		test.Fatalf("a write under the current lease: %v %v", applied, failure)
	}
}

func TestANewOwnerRestoresTheInFlightProcessesOfItsSlices(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	previous := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Logger: testsupport.Logger()})
	ownEverySlice(test, previous)
	target := startedRunScript(test, store, "in flight")
	now := time.Now()
	waiting := map[NodeKey]campaign.Pid{}
	for index := range 12 {
		key := sliceNode(100 + index)
		row, applied, failure := previous.write(operation, nil, campaign.NewRow(target.ID, key.DeviceID, key.InstallationID, 0, campaign.StatePending), "test")
		if failure != nil || !applied {
			test.Fatalf("insert: %v %v", applied, failure)
		}
		if index%3 == 0 {
			continue
		}
		deadline := now.Add(time.Hour)
		if index%3 == 1 {
			deadline = now.Add(-time.Minute)
		} else {
			waiting[key] = campaign.DerivePid(target.ID, key.DeviceID, key.InstallationID, 1)
		}
		next, _ := row.Dispatch(target.ID, campaign.Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 3}, now, deadline, nil)
		if _, applied, failure := previous.write(operation, &row, next, "test"); failure != nil || !applied {
			test.Fatalf("dispatch: %v %v", applied, failure)
		}
	}
	successor := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Logger: testsupport.Logger()})
	if !successor.ownership.load(operation) {
		test.Fatal("the partition count was not read")
	}
	for partition := range successor.ownership.partitions {
		lease, _, failure := successor.ownership.raise(operation, partition)
		if failure != nil {
			test.Fatalf("partition %d: %v", partition, failure)
		}
		successor.ownership.own(lease)
	}
	for index := range 12 {
		key := sliceNode(100 + index)
		pid, inFlight := waiting[key]
		successor.dispatcher.mutex.Lock()
		entry := successor.dispatcher.inFlight[key]
		successor.dispatcher.mutex.Unlock()
		switch {
		case inFlight && (entry == nil || !entry.pids[pid] || !entry.deadline.After(now)):
			test.Errorf("node %d's process %s still waits for its result but is not in flight: %+v", index, pid, entry)
		case !inFlight && entry != nil:
			test.Errorf("node %d has no process waiting for a result but is in flight: %+v", index, entry)
		}
	}
	key := sliceNode(101)
	if _, applied, _ := previous.write(operation, nil, campaign.NewRow(uuid.New(), key.DeviceID, key.InstallationID, 0, campaign.StatePending), "test"); applied {
		test.Fatal("the previous owner wrote after its slices were acquired by another")
	}
	partition, _ := successor.ownership.partitionOf(sliceNode(102))
	successor.ownership.drop(partition, true)
	successor.dispatcher.mutex.Lock()
	_, kept := successor.dispatcher.inFlight[sliceNode(102)]
	successor.dispatcher.mutex.Unlock()
	if kept {
		test.Fatal("a slice given up kept its nodes in flight")
	}
}

type sliceMember struct {
	engine  *Engine
	pool    *pgxpool.Pool
	cancel  context.CancelFunc
	stopped chan struct{}
}

func startSliceMember(test *testing.T, url string, settings config.Config, instance string) *sliceMember {
	test.Helper()
	operation := context.Background()
	pool, failure := database.Open(operation, url+"&application_name="+instance, 8)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(pool.Close)
	settings.Instance = instance
	options, failure := kafka.Options(settings.Kafka, instance)
	if failure != nil {
		test.Fatal(failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	member := &sliceMember{pool: pool, stopped: make(chan struct{})}
	member.engine = New(Dependencies{Config: settings, Pool: pool, Campaigns: campaign.NewStore(pool), Inventory: inventory.NewStore(pool), KafkaOptions: options,
		Validator: validator, Logger: testsupport.Logger().With("instance", instance)})
	running, cancel := context.WithCancel(operation)
	member.cancel = cancel
	go func() {
		defer close(member.stopped)
		member.engine.ownership.run(running)
	}()
	test.Cleanup(member.stop)
	return member
}

func (member *sliceMember) stop() {
	member.cancel()
	<-member.stopped
}

func (member *sliceMember) owned() map[int32]campaign.Lease {
	member.engine.ownership.mutex.Lock()
	defer member.engine.ownership.mutex.Unlock()
	owned := map[int32]campaign.Lease{}
	for partition, held := range member.engine.ownership.slices {
		owned[partition] = held.lease
	}
	return owned
}

func eventually(test *testing.T, what string, timeout time.Duration, check func() (bool, string)) {
	test.Helper()
	deadline := time.Now().Add(timeout)
	last := ""
	for time.Now().Before(deadline) {
		done, detail := check()
		if done {
			return
		}
		last = detail
		time.Sleep(100 * time.Millisecond)
	}
	test.Fatalf("%s did not happen within %s; last: %s", what, timeout, last)
}

func TestNoRowIsWrittenUnderAStaleLeaseAcrossARebalance(test *testing.T) {
	pool, url := testsupport.Database(test)
	brokers := testsupport.Brokers(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	suffix := strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	settings := config.Default()
	settings.Kafka.Brokers = []string{brokers}
	settings.Kafka.AllowPlaintext = true
	settings.Kafka.Topics.ProcessResults = "dusk.process-results." + suffix
	settings.Kafka.ResultsGroup = "twilight-results-" + suffix
	producer, failure := kgo.NewClient(kgo.SeedBrokers(brokers))
	if failure != nil {
		test.Fatal(failure)
	}
	defer producer.Close()
	if _, failure := kadm.NewClient(producer).CreateTopic(operation, testsupport.NodePartitions, 1, map[string]*string{"cleanup.policy": new("delete")}, settings.Kafka.Topics.ProcessResults); failure != nil {
		test.Fatal(failure)
	}
	target := startedRunScript(test, store, "rebalanced")
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	const nodes = 48
	pids := map[NodeKey]campaign.Pid{}
	for index := range nodes {
		key := sliceNode(1000 + index)
		state := campaign.StatePending
		if index%2 == 0 {
			state = campaign.StateDispatched
			pids[key] = campaign.DerivePid(target.ID, key.DeviceID, key.InstallationID, 1)
		}
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, pid, dispatched_at, deadline_at, revision)
			values ($1, $2, $3, 0, $4, nullif($5, '')::numeric, case when $5 <> '' then now() end, case when $5 <> '' then now() + interval '1 hour' end, 1)`,
			target.ID, key.DeviceID, key.InstallationID, state, map[bool]string{true: pids[key].String(), false: ""}[pids[key] != 0]); failure != nil {
			test.Fatal(failure)
		}
	}
	if _, failure := pool.Exec(operation, `create table slice_writes (partition smallint, writer text, lease_owner text, lease_epoch bigint);
		create function record_slice_write() returns trigger language plpgsql as $body$
		begin
			insert into slice_writes select new.partition, current_setting('application_name'), owner, epoch from node_partition_leases where partition = new.partition;
			return new;
		end
		$body$;
		create trigger record_slice_writes after insert or update on campaign_nodes for each row execute function record_slice_write()`); failure != nil {
		test.Fatal(failure)
	}
	produce := func(index int) {
		key := sliceNode(1000 + index)
		now := kafka.FormatTime(time.Now())
		message := map[string]any{
			"schema": "dusk.process-results/v1", "id": kafka.NewMessageID(), "time": now, "pid": pids[key].String(), "campaign_id": target.ID.String(),
			"attempt": 1, "device_id": key.DeviceID, "installation_id": key.InstallationID, "namespace_id": "5d2e9a1c7b3f8e04",
			"action_kind": "run_script", "status": "succeeded", "delivered": true, "error": nil, "started_at": now, "finished_at": now,
			"dawn_instance": "dawn-0", "output_digest": strings.Repeat("ab", 32), "output_count": 0, "output_truncated": false, "reported": nil,
		}
		encoded, _ := json.Marshal(message)
		if failure := validator.Validate(kafka.ContractProcessResults, encoded); failure != nil {
			test.Fatalf("an invalid process result: %v", failure)
		}
		producing, cancel := context.WithTimeout(operation, 30*time.Second)
		defer cancel()
		if failure := producer.ProduceSync(producing, &kgo.Record{Topic: settings.Kafka.Topics.ProcessResults, Key: []byte(key.String()), Value: encoded}).FirstErr(); failure != nil {
			test.Fatal(failure)
		}
	}
	settled := func(indexes func(int) bool) func() (bool, string) {
		return func() (bool, string) {
			for index := range nodes {
				if index%2 != 0 || !indexes(index) {
					continue
				}
				key := sliceNode(1000 + index)
				row, failure := store.Row(operation, target.ID, key.DeviceID, key.InstallationID)
				if failure != nil || row.State != campaign.StateSucceeded {
					return false, fmt.Sprintf("node %d is %s (%v)", index, row.State, failure)
				}
			}
			return true, ""
		}
	}

	first := startSliceMember(test, url, settings, "twilight-first")
	eventually(test, "the first member owning every slice", time.Minute, func() (bool, string) {
		owned := first.owned()
		return len(owned) == testsupport.NodePartitions, fmt.Sprintf("owns %v", owned)
	})
	initial := first.owned()
	for index := 0; index < nodes/2; index += 2 {
		produce(index)
	}
	eventually(test, "the first half of the results applied", time.Minute, settled(func(index int) bool { return index < nodes/2 }))

	var churning sync.WaitGroup
	stopChurn := make(chan struct{})
	churn := func(member *Engine) {
		defer churning.Done()
		for {
			select {
			case <-stopChurn:
				return
			case <-time.After(20 * time.Millisecond):
			}
			for index := 1; index < nodes; index += 2 {
				key := sliceNode(1000 + index)
				row, failure := store.Row(operation, target.ID, key.DeviceID, key.InstallationID)
				if failure != nil {
					continue
				}
				next := row
				next.LastError = member.Config.Instance
				member.write(operation, &row, next, "churn")
			}
		}
	}
	churning.Add(1)
	go churn(first.engine)
	second := startSliceMember(test, url, settings, "twilight-second")
	churning.Add(1)
	go churn(second.engine)
	eventually(test, "the slices split between the two members", 2*time.Minute, func() (bool, string) {
		firstOwned, secondOwned := first.owned(), second.owned()
		for partition := range firstOwned {
			if _, both := secondOwned[partition]; both {
				return false, fmt.Sprintf("both own partition %d", partition)
			}
		}
		return len(secondOwned) > 0 && len(firstOwned)+len(secondOwned) == testsupport.NodePartitions, fmt.Sprintf("first %v, second %v", firstOwned, secondOwned)
	})
	for index := nodes / 2; index < nodes; index += 2 {
		produce(index)
	}
	eventually(test, "the second half of the results applied", time.Minute, settled(func(int) bool { return true }))
	time.Sleep(500 * time.Millisecond)
	close(stopChurn)
	churning.Wait()

	moved := second.owned()
	for partition, stale := range initial {
		if _, taken := moved[partition]; !taken {
			continue
		}
		for index := 1; index < nodes; index += 2 {
			key := sliceNode(1000 + index)
			if owner, _ := first.engine.ownership.partitionOf(key); owner != partition {
				continue
			}
			row, failure := store.Row(operation, target.ID, key.DeviceID, key.InstallationID)
			if failure != nil {
				test.Fatal(failure)
			}
			next := row
			next.LastError = "stale"
			if _, applied, failure := store.UpdateRow(operation, stale, row, next); failure != nil || applied {
				test.Fatalf("a write under the first member's lease of partition %d after it moved: %v %v", partition, applied, failure)
			}
			break
		}
	}
	var stale, total, bySecond int
	if failure := pool.QueryRow(operation, `select count(*) filter (where writer <> lease_owner), count(*), count(*) filter (where writer = 'twilight-second') from slice_writes`).
		Scan(&stale, &total, &bySecond); failure != nil {
		test.Fatal(failure)
	}
	if stale != 0 || total == 0 || bySecond == 0 {
		test.Fatalf("%d of %d row writes were made by an instance that did not hold the slice's lease; %d were the second member's", stale, total, bySecond)
	}
	rows, failure := pool.Query(operation, `select partition, owner, epoch from node_partition_leases order by partition`)
	if failure != nil {
		test.Fatal(failure)
	}
	defer rows.Close()
	for rows.Next() {
		var partition int32
		var owner string
		var epoch int64
		if failure := rows.Scan(&partition, &owner, &epoch); failure != nil {
			test.Fatal(failure)
		}
		holder := first.owned()
		if _, held := moved[partition]; held {
			holder = moved
		}
		if lease := holder[partition]; lease.Epoch != epoch || (owner != "twilight-first" && owner != "twilight-second") {
			test.Errorf("partition %d is at epoch %d owned by %s; its member holds %+v", partition, epoch, owner, lease)
		}
	}
}

func TestAnUnleasedSliceWaitsForItsLeaseAndMisplacedResultsAreDropped(test *testing.T) {
	pool, url := testsupport.Database(test)
	brokers := testsupport.Brokers(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	suffix := strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	settings := config.Default()
	settings.Kafka.Brokers = []string{brokers}
	settings.Kafka.AllowPlaintext = true
	settings.Kafka.Topics.ProcessResults = "dusk.process-results." + suffix
	settings.Kafka.ResultsGroup = "twilight-results-" + suffix
	producer, failure := kgo.NewClient(kgo.SeedBrokers(brokers), kgo.RecordPartitioner(kgo.ManualPartitioner()))
	if failure != nil {
		test.Fatal(failure)
	}
	defer producer.Close()
	if _, failure := kadm.NewClient(producer).CreateTopic(operation, testsupport.NodePartitions, 1, map[string]*string{"cleanup.policy": new("delete")}, settings.Kafka.Topics.ProcessResults); failure != nil {
		test.Fatal(failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	target := startedRunScript(test, store, "unleased")
	const refused, served int32 = 1, 0
	byPartition := map[int32][]NodeKey{}
	for index := 0; len(byPartition[refused]) < 3 || len(byPartition[served]) < 2; index++ {
		key := sliceNode(2000 + index)
		partition := kafka.KeyPartition(key.String(), testsupport.NodePartitions)
		byPartition[partition] = append(byPartition[partition], key)
	}
	waiting, droppedNode, lateNode := byPartition[refused][0], byPartition[refused][1], byPartition[refused][2]
	early, after := byPartition[served][0], byPartition[served][1]
	for _, key := range []NodeKey{waiting, droppedNode, lateNode, early, after} {
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, pid, dispatched_at, deadline_at, revision)
			values ($1, $2, $3, 0, 'dispatched', $4::text::numeric, now(), now() + interval '1 hour', 1)`,
			target.ID, key.DeviceID, key.InstallationID, campaign.DerivePid(target.ID, key.DeviceID, key.InstallationID, 1).String()); failure != nil {
			test.Fatal(failure)
		}
	}
	if _, failure := pool.Exec(operation, fmt.Sprintf(`create function refuse_lease() returns trigger language plpgsql as $body$
		begin
			if new.partition = %d then
				raise exception 'the test holds this lease back';
			end if;
			return new;
		end
		$body$;
		create trigger refuse_lease before update on node_partition_leases for each row execute function refuse_lease()`, refused)); failure != nil {
		test.Fatal(failure)
	}
	produce := func(key NodeKey, partition int32) {
		now := kafka.FormatTime(time.Now())
		message := map[string]any{
			"schema": "dusk.process-results/v1", "id": kafka.NewMessageID(), "time": now, "pid": campaign.DerivePid(target.ID, key.DeviceID, key.InstallationID, 1).String(),
			"campaign_id": target.ID.String(), "attempt": 1, "device_id": key.DeviceID, "installation_id": key.InstallationID, "namespace_id": "5d2e9a1c7b3f8e04",
			"action_kind": "run_script", "status": "succeeded", "delivered": true, "error": nil, "started_at": now, "finished_at": now,
			"dawn_instance": "dawn-0", "output_digest": strings.Repeat("ab", 32), "output_count": 0, "output_truncated": false, "reported": nil,
		}
		encoded, _ := json.Marshal(message)
		if failure := validator.Validate(kafka.ContractProcessResults, encoded); failure != nil {
			test.Fatalf("an invalid process result: %v", failure)
		}
		producing, cancel := context.WithTimeout(operation, 30*time.Second)
		defer cancel()
		if failure := producer.ProduceSync(producing, &kgo.Record{Topic: settings.Kafka.Topics.ProcessResults, Partition: partition, Key: []byte(key.String()), Value: encoded}).FirstErr(); failure != nil {
			test.Fatal(failure)
		}
	}
	state := func(key NodeKey) campaign.NodeState {
		row, failure := store.Row(operation, target.ID, key.DeviceID, key.InstallationID)
		if failure != nil {
			test.Fatal(failure)
		}
		return row.State
	}
	mismatches := func() float64 { return metricValue(test, "twilight_slice_partition_mismatches_total", nil) }
	before := mismatches()

	member := startSliceMember(test, url, settings, "twilight-unleased")
	eventually(test, "every slice but the refused one owned and the refused one pending", time.Minute, func() (bool, string) {
		owned := member.owned()
		member.engine.ownership.mutex.Lock()
		pending := member.engine.ownership.pending[refused] != nil
		member.engine.ownership.mutex.Unlock()
		_, refusedOwned := owned[refused]
		return len(owned) == testsupport.NodePartitions-1 && !refusedOwned && pending, fmt.Sprintf("owns %v, pending %v", owned, pending)
	})
	produce(waiting, refused)
	produce(early, served)
	produce(droppedNode, served)
	produce(after, served)
	eventually(test, "the served slice applies its results, the misplaced one among them dropped", time.Minute, func() (bool, string) {
		return state(early) == campaign.StateSucceeded && state(after) == campaign.StateSucceeded && mismatches() == before+1,
			fmt.Sprintf("early %s, after %s, mismatches %v", state(early), state(after), mismatches()-before)
	})
	if state(waiting) != campaign.StateDispatched || state(droppedNode) != campaign.StateDispatched {
		test.Fatalf("a result was applied in a slice this instance has no lease of: waiting %s, misplaced %s", state(waiting), state(droppedNode))
	}

	if _, failure := pool.Exec(operation, `drop trigger refuse_lease on node_partition_leases`); failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the refused slice is owned once its lease can be taken, and its waiting result applied", 2*time.Minute, func() (bool, string) {
		return len(member.owned()) == testsupport.NodePartitions && state(waiting) == campaign.StateSucceeded, fmt.Sprintf("owns %v, waiting %s", member.owned(), state(waiting))
	})
	produce(lateNode, served)
	eventually(test, "a misplaced result applied once this instance owns its node's slice", time.Minute, func() (bool, string) {
		return state(lateNode) == campaign.StateSucceeded && mismatches() == before+2, fmt.Sprintf("late %s, mismatches %v", state(lateNode), mismatches()-before)
	})
	if state(droppedNode) != campaign.StateDispatched {
		test.Fatalf("the dropped misplaced result was applied later: %s", state(droppedNode))
	}
}

func TestAStuckLeaseAttemptHoldsBackOnlyItsOwnSlice(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	owner := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: campaign.NewStore(pool), Inventory: inventory.NewStore(pool), Logger: testsupport.Logger()})
	if !owner.ownership.load(operation) {
		test.Fatal("the partition count was not read")
	}
	client, failure := kgo.NewClient(kgo.SeedBrokers("127.0.0.1:1"))
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	topic := owner.Config.Kafka.Topics.ProcessResults
	holding, failure := pool.Begin(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	defer holding.Rollback(operation)
	if _, failure := holding.Exec(operation, `select pg_advisory_xact_lock($1)`, database.SliceLockKey+1); failure != nil {
		test.Fatal(failure)
	}
	owner.ownership.mutex.Lock()
	owner.ownership.pending[1] = &pendingLease{since: time.Now()}
	owner.ownership.pending[2] = &pendingLease{since: time.Now()}
	owner.ownership.mutex.Unlock()
	retrying, stopRetrying := context.WithCancel(operation)
	retried := make(chan struct{})
	go func() {
		owner.ownership.retryUnleased(retrying)
		close(retried)
	}()
	defer func() {
		stopRetrying()
		<-retried
	}()
	eventually(test, "the attempt on slice 1 waiting for its lock", 30*time.Second, func() (bool, string) {
		owner.ownership.mutex.Lock()
		defer owner.ownership.mutex.Unlock()
		stuck := owner.ownership.pending[1]
		return stuck != nil && stuck.retrying, "slice 1 is not being attempted"
	})
	revoked := make(chan struct{})
	go func() {
		owner.ownership.revoked(operation, client, map[string][]int32{topic: {2}})
		close(revoked)
	}()
	select {
	case <-revoked:
	case <-time.After(5 * time.Second):
		test.Fatal("revoking slice 2 waited for the stuck attempt on slice 1")
	}
	lost := make(chan struct{})
	go func() {
		owner.ownership.lost(operation, client, map[string][]int32{topic: {1}})
		close(lost)
	}()
	select {
	case <-lost:
	case <-time.After(10 * time.Second):
		test.Fatal("losing slice 1 did not cancel its stuck lease attempt")
	}
	if failure := holding.Rollback(operation); failure != nil {
		test.Fatal(failure)
	}
	var epoch int64
	if failure := pool.QueryRow(operation, `select epoch from node_partition_leases where partition = 1`).Scan(&epoch); failure != nil || epoch != 0 {
		test.Fatalf("the cancelled attempt raised slice 1's epoch to %d (%v)", epoch, failure)
	}
	if owner.ownership.owning() {
		test.Fatal("a slice given up while its lease was pending ended up owned")
	}
}
