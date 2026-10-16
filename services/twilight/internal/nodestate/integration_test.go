//go:build integration

package nodestate_test

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/nodestate"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

const (
	revokedDevice       = "10000000000000000000000000000001"
	quarantinedDevice   = "10000000000000000000000000000002"
	activeDevice        = "10000000000000000000000000000003"
	strangerDevice      = "10000000000000000000000000000004"
	withdrawnDevice     = "20000000000000000000000000000001"
	clearedDevice       = "20000000000000000000000000000002"
	anyInstallation     = "a0000000000000000000000000000001"
	operatorPrincipal   = "operator@example.org"
	quarantinePrincipal = "campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd"
)

type fixture struct {
	test      *testing.T
	pool      *pgxpool.Pool
	store     *inventory.Store
	producer  *kafka.Producer
	validator *kafka.Validator
	options   []kgo.Opt
	topic     string
}

func newFixture(test *testing.T, operation context.Context) *fixture {
	pool, _ := testsupport.Database(test)
	settings := config.Default().Kafka
	settings.Brokers = []string{testsupport.Brokers(test)}
	settings.AllowPlaintext = true
	options, failure := kafka.Options(settings, "test")
	if failure != nil {
		test.Fatal(failure)
	}
	client, failure := kgo.NewClient(options...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(client.Close)
	topic := "dusk.node-state." + strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	if _, failure := kadm.NewClient(client).CreateTopic(operation, 3, 1, map[string]*string{"cleanup.policy": new("compact")}, topic); failure != nil {
		test.Fatal(failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	return &fixture{test: test, pool: pool, store: inventory.NewStore(pool), producer: kafka.NewNodeStateProducer(client, validator, topic),
		validator: validator, options: options, topic: topic}
}

func (fixture *fixture) rebuild() nodestate.Rebuild {
	return nodestate.Rebuild{Options: fixture.options, Producer: fixture.producer, Inventory: fixture.store, Validator: fixture.validator,
		Topic: fixture.topic, Actor: "rebuild-test", Workers: 4, Logger: testsupport.Logger()}
}

func (fixture *fixture) node(operation context.Context, device string) {
	if _, failure := fixture.pool.Exec(operation, `insert into nodes (device_id, installation_id) values ($1, $2)`, device, anyInstallation); failure != nil {
		fixture.test.Fatal(failure)
	}
}

func (fixture *fixture) setLifecycle(operation context.Context, device, lifecycle, actor string, at time.Time) {
	key := inventory.Key{DeviceID: device, InstallationID: anyInstallation}
	if _, failure := fixture.store.SetLifecycle(operation, key, lifecycle, "fixture", actor, at, func(string) error { return nil }); failure != nil {
		fixture.test.Fatal(failure)
	}
}

func (fixture *fixture) setDeviceLifecycle(operation context.Context, device, lifecycle string, at time.Time) {
	changed := inventory.DeviceLifecycle{DeviceID: device, Lifecycle: lifecycle, Reason: "fixture", ChangedAt: at, Actor: operatorPrincipal}
	if _, failure := fixture.store.SetDeviceLifecycle(operation, changed, func(string) error { return nil }); failure != nil {
		fixture.test.Fatal(failure)
	}
}

func (fixture *fixture) read(operation context.Context) nodestate.Snapshot {
	snapshot, failure := nodestate.Read(operation, fixture.options, fixture.topic, fixture.validator, testsupport.Logger(), true, time.Minute)
	if failure != nil {
		fixture.test.Fatal(failure)
	}
	return snapshot
}

func (fixture *fixture) lifecycle(operation context.Context, device string) (string, string) {
	var lifecycle, actor string
	if failure := fixture.pool.QueryRow(operation, `select lifecycle, coalesce(lifecycle_actor, '') from nodes where device_id = $1`, device).Scan(&lifecycle, &actor); failure != nil {
		fixture.test.Fatal(failure)
	}
	return lifecycle, actor
}

func lifecycleOf(snapshot nodestate.Snapshot, key string) string {
	if entry := snapshot.States[key]; entry != nil {
		if entry.Lifecycle() == "" {
			return "tombstone"
		}
		return entry.Lifecycle()
	}
	return "absent"
}

func TestRebuildRepublishesTheInventoryAndKeepsEveryLaterTopicState(test *testing.T) {
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	fixture := newFixture(test, operation)
	revokedAt := time.Date(2026, 10, 1, 9, 0, 0, 0, time.UTC)
	for _, device := range []string{revokedDevice, quarantinedDevice, activeDevice} {
		fixture.node(operation, device)
	}
	fixture.setLifecycle(operation, revokedDevice, "revoked", operatorPrincipal, revokedAt)
	fixture.setLifecycle(operation, quarantinedDevice, "quarantined", quarantinePrincipal, revokedAt)
	fixture.setLifecycle(operation, activeDevice, "active", operatorPrincipal, revokedAt)
	fixture.setDeviceLifecycle(operation, withdrawnDevice, "revoked", revokedAt)
	fixture.setDeviceLifecycle(operation, clearedDevice, "revoked", revokedAt)
	fixture.setDeviceLifecycle(operation, clearedDevice, "active", revokedAt.Add(time.Hour))

	empty := fixture.read(operation)
	if len(empty.Incomplete()) != 3 {
		test.Fatalf("an empty topic of 3 partitions has %v incomplete", empty.Incomplete())
	}
	report, failure := fixture.rebuild().Run(operation, true)
	if failure != nil {
		test.Fatal(failure)
	}
	if !report.Rebuilt || report.Published != 3 || report.WrittenBack != 0 || report.Unknown != 0 || len(report.Incomplete) != 3 {
		test.Fatalf("rebuild of an empty topic: %+v", report)
	}
	rebuilt := fixture.read(operation)
	if missing := rebuilt.Incomplete(); len(missing) != 0 {
		test.Fatalf("partitions %v hold no completion record after the rebuild", missing)
	}
	revoked := rebuilt.States[nodestate.Key(revokedDevice, anyInstallation)]
	if revoked == nil || revoked.Lifecycle() != "revoked" || revoked.Actor() != operatorPrincipal || !revoked.Time().Equal(revokedAt) {
		test.Fatalf("the revoked installation on the topic: %+v", revoked)
	}
	for key, want := range map[string]string{
		nodestate.Key(quarantinedDevice, anyInstallation): "quarantined",
		nodestate.Key(withdrawnDevice, ""):                "revoked",
		nodestate.Key(activeDevice, anyInstallation):      "absent",
		nodestate.Key(clearedDevice, ""):                  "absent",
	} {
		if got := lifecycleOf(rebuilt, key); got != want {
			test.Errorf("%s: %s on the topic, want %s", key, got, want)
		}
	}

	again, failure := fixture.rebuild().Run(operation, true)
	if failure != nil {
		test.Fatal(failure)
	}
	if again.Rebuilt {
		test.Fatalf("a complete topic was rebuilt again: %+v", again)
	}

	later := revokedAt.Add(30 * time.Minute)
	reason := "after the restore point"
	for _, state := range []kafka.NodeState{
		kafka.NewInstallationState(activeDevice, anyInstallation, "revoked", &reason, "admin@example.org", later),
		kafka.NewInstallationState(quarantinedDevice, anyInstallation, "revoked", &reason, "admin@example.org", later),
		kafka.NewInstallationState(strangerDevice, anyInstallation, "revoked", &reason, "admin@example.org", later),
		kafka.NewDeviceState(clearedDevice, "revoked", &reason, "admin@example.org", revokedAt),
	} {
		if failure := fixture.producer.ProduceNodeState(operation, state); failure != nil {
			test.Fatal(failure)
		}
	}
	reconciled, failure := fixture.rebuild().Run(operation, false)
	if failure != nil {
		test.Fatal(failure)
	}
	if reconciled.WrittenBack != 2 || reconciled.Unknown != 1 || reconciled.Published != 1 || reconciled.InSync != 2 {
		test.Fatalf("rebuild against a later topic: %+v", reconciled)
	}
	for _, device := range []string{activeDevice, quarantinedDevice} {
		if lifecycle, actor := fixture.lifecycle(operation, device); lifecycle != "revoked" || actor != "admin@example.org" {
			test.Errorf("%s: %s by %q in the inventory, want the topic's later revocation", device, lifecycle, actor)
		}
	}
	final := fixture.read(operation)
	for key, want := range map[string]string{
		nodestate.Key(quarantinedDevice, anyInstallation): "revoked",
		nodestate.Key(strangerDevice, anyInstallation):    "revoked",
		nodestate.Key(revokedDevice, anyInstallation):     "revoked",
		nodestate.Key(clearedDevice, ""):                  "tombstone",
	} {
		if got := lifecycleOf(final, key); got != want {
			test.Errorf("%s: %s on the topic, want %s", key, got, want)
		}
	}
}

func TestARebuildFailsAndWritesNoCompletionRecordWhenAPartitionStalls(test *testing.T) {
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	fixture := newFixture(test, operation)
	fixture.node(operation, revokedDevice)
	fixture.setLifecycle(operation, revokedDevice, "revoked", operatorPrincipal, time.Now())
	transactional, failure := kgo.NewClient(append(append([]kgo.Opt(nil), fixture.options...), kgo.TransactionalID("stall-"+fixture.topic))...)
	if failure != nil {
		test.Fatal(failure)
	}
	defer transactional.Close()
	if failure := transactional.BeginTransaction(); failure != nil {
		test.Fatal(failure)
	}
	stalled := &kgo.Record{Topic: fixture.topic, Key: []byte(nodestate.Key(strangerDevice, "")), Value: []byte(`{}`)}
	if failure := transactional.ProduceSync(operation, stalled).FirstErr(); failure != nil {
		test.Fatal(failure)
	}
	rebuild := fixture.rebuild()
	rebuild.Stall = 5 * time.Second
	for _, onlyIfIncomplete := range []bool{true, false} {
		report, failure := rebuild.Run(operation, onlyIfIncomplete)
		if !errors.Is(failure, nodestate.ErrUnread) || report.Rebuilt || !strings.Contains(failure.Error(), fmt.Sprintf("%d (end offset", stalled.Partition)) {
			test.Fatalf("only if incomplete %v: %+v, %v", onlyIfIncomplete, report, failure)
		}
	}
	plain, failure := kgo.NewClient(append(append([]kgo.Opt(nil), fixture.options...), kgo.ConsumeTopics(fixture.topic), kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))...)
	if failure != nil {
		test.Fatal(failure)
	}
	defer plain.Close()
	polling, stop := context.WithTimeout(operation, 5*time.Second)
	fetches := plain.PollFetches(polling)
	stop()
	fetches.EachRecord(func(record *kgo.Record) {
		if strings.HasPrefix(string(record.Key), kafka.NodeStateCompletePrefix) {
			test.Errorf("a completion record %s was written over a partial read", record.Key)
		}
	})
	if failure := transactional.EndTransaction(operation, kgo.TryAbort); failure != nil {
		test.Fatal(failure)
	}
	report, failure := rebuild.Run(operation, true)
	if failure != nil || !report.Rebuilt {
		test.Fatalf("after the transaction ended: %+v, %v", report, failure)
	}
	if missing := fixture.read(operation).Incomplete(); len(missing) != 0 {
		test.Fatalf("partitions %v hold no completion record", missing)
	}
}

func TestCheckReadsTheTopicOnlyWhenItIsNotTheOneLastFoundComplete(test *testing.T) {
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	fixture := newFixture(test, operation)
	partitioner := kgo.StickyKeyPartitioner(nil).ForTopic("")
	partitionOf := func(key string, partitions int) int32 {
		return int32(partitioner.Partition(&kgo.Record{Key: []byte(key)}, partitions))
	}
	var devices []string
	moving := 0
	for index := range 16 {
		device := fmt.Sprintf("3%031x", index)
		devices = append(devices, device)
		fixture.node(operation, device)
		fixture.setLifecycle(operation, device, "revoked", operatorPrincipal, time.Now())
		if key := nodestate.Key(device, anyInstallation); partitionOf(key, 3) != partitionOf(key, 4) {
			moving++
		}
	}
	if moving == 0 {
		test.Fatal("no key moves when a fourth partition is added")
	}
	rebuild := fixture.rebuild()
	report, failure := rebuild.Check(operation)
	if failure != nil || !report.Rebuilt || report.Published != len(devices) {
		test.Fatalf("the first check of an empty topic: %+v, %v", report, failure)
	}
	report, failure = rebuild.Check(operation)
	if failure != nil || !report.Skipped || report.Checked {
		test.Fatalf("a check of the topic just rebuilt: %+v, %v", report, failure)
	}
	client, failure := kgo.NewClient(fixture.options...)
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	if _, failure := kadm.NewClient(client).CreatePartitions(operation, 1, fixture.topic); failure != nil {
		test.Fatal(failure)
	}
	deadline := time.Now().Add(time.Minute)
	for {
		report, failure = rebuild.Check(operation)
		if failure == nil || time.Now().After(deadline) {
			break
		}
		time.Sleep(time.Second)
	}
	if failure != nil || !report.Rebuilt || report.Partitions != 4 || len(report.Incomplete) != 1 || report.Incomplete[0] != 3 ||
		report.Published != 0 || report.Moved != moving {
		test.Fatalf("a check after a partition was added, with %d keys moving: %+v, %v", moving, report, failure)
	}
	snapshot := fixture.read(operation)
	if missing := snapshot.Incomplete(); len(missing) != 0 {
		test.Fatalf("partitions %v hold no completion record after the partition was added", missing)
	}
	for _, device := range devices {
		key := nodestate.Key(device, anyInstallation)
		entry := snapshot.States[key]
		if entry == nil || entry.Lifecycle() != "revoked" || entry.Partition() != partitionOf(key, 4) {
			test.Fatalf("%s is not revoked on the partition it hashes to: %+v", key, entry)
		}
	}
	report, failure = rebuild.Check(operation)
	if failure != nil || !report.Skipped {
		test.Fatalf("a check of the four partitions just rebuilt: %+v, %v", report, failure)
	}
}
