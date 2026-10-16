package nodestate

import (
	"testing"
	"time"

	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/inventory"
)

const (
	device       = "7c1a5e9f3b2d4a6e8f0c1b3d5e7a9c2f"
	installation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
)

func installationState(lifecycle string, changed *time.Time) inventory.LifecycleState {
	return inventory.LifecycleState{DeviceID: device, InstallationID: installation, Lifecycle: lifecycle, ChangedAt: changed}
}

func deviceState(lifecycle string, changed *time.Time) inventory.LifecycleState {
	return inventory.LifecycleState{DeviceID: device, Lifecycle: lifecycle, ChangedAt: changed}
}

func TestDecideKeepsTheLaterOfTopicAndInventoryAndNeverDropsAnUnknownNodesState(test *testing.T) {
	earlier := time.Date(2026, 10, 1, 9, 0, 0, 0, time.UTC)
	later := earlier.Add(time.Hour)
	for _, check := range []struct {
		name    string
		topic   *Entry
		current inventory.LifecycleState
		want    action
	}{
		{"nothing anywhere", nil, installationState("active", &earlier), inSync},
		{"a revocation missing from the topic", nil, installationState("revoked", &earlier), publish},
		{"a device revocation missing from the topic", nil, deviceState("revoked", &earlier), publish},
		{"the same revocation on both", NewEntry("revoked", earlier), installationState("revoked", &earlier), inSync},
		{"active and enrolled are the default", NewEntry("active", later), installationState("enrolled", &earlier), inSync},
		{"a tombstone is the default", NewEntry("", later), deviceState("", nil), inSync},
		{"the inventory quarantined later", NewEntry("revoked", earlier), installationState("quarantined", &later), publish},
		{"the topic revoked later than the inventory quarantined", NewEntry("revoked", later), installationState("quarantined", &earlier), writeBack},
		{"the topic revoked a node the inventory holds active", NewEntry("revoked", later), installationState("active", &earlier), writeBack},
		{"the inventory released later", NewEntry("quarantined", earlier), installationState("active", &later), publish},
		{"the topic revoked a node the inventory does not hold", NewEntry("revoked", later), installationState("", nil), unknownNode},
		{"the topic revoked a device the inventory never changed", NewEntry("revoked", earlier), deviceState("", nil), writeBack},
		{"the inventory cleared a device later", NewEntry("revoked", earlier), deviceState("active", &later), publish},
		{"the topic cleared a device later", NewEntry("", later), deviceState("revoked", &earlier), writeBack},
		{"a tie goes to the inventory", NewEntry("retired", earlier), installationState("revoked", &earlier), publish},
	} {
		if got := decide(check.topic, check.current); got != check.want {
			test.Errorf("%s: %d, want %d", check.name, got, check.want)
		}
	}
}

func TestCompleteKeysLandOneOnEveryPartition(test *testing.T) {
	partitioner := kgo.StickyKeyPartitioner(nil).ForTopic("")
	for _, partitions := range []int32{1, 3, 6, 48} {
		keys := CompleteKeys(partitions)
		if int32(len(keys)) != partitions {
			test.Fatalf("%d partitions: %d keys", partitions, len(keys))
		}
		seen := map[string]bool{}
		for partition, key := range keys {
			if seen[key] {
				test.Fatalf("%d partitions: %s twice", partitions, key)
			}
			seen[key] = true
			if got := partitioner.Partition(&kgo.Record{Key: []byte(key)}, int(partitions)); got != partition {
				test.Errorf("%d partitions: %s lands on %d, not %d", partitions, key, got, partition)
			}
		}
	}
	if keys := CompleteKeys(1); keys[0] != "complete/0" {
		test.Errorf("one partition: %v", keys)
	}
	again := CompleteKeys(6)
	for index, key := range CompleteKeys(6) {
		if again[index] != key {
			test.Errorf("the keys differ between two runs: %v and %v", again, CompleteKeys(6))
		}
	}
}

func TestKeysAndScopesRoundTrip(test *testing.T) {
	if gotDevice, gotInstallation := scope(Key(device, installation)); gotDevice != device || gotInstallation != installation {
		test.Errorf("installation key: %s %s", gotDevice, gotInstallation)
	}
	if gotDevice, gotInstallation := scope(Key(device, "")); gotDevice != device || gotInstallation != "" {
		test.Errorf("device key: %s %s", gotDevice, gotInstallation)
	}
}

func TestTheSnapshotKeepsTheLaterCopyAcrossPartitionsAndTheLastReadWithinOne(t *testing.T) {
	snapshot := Snapshot{Partitions: 4, States: map[string]*Entry{}}
	key := Key(device, installation)
	current := partitionOf(key, 4)
	other := (current + 1) % 4
	snapshot.keep(key, &Entry{lifecycle: lifecycleCode("revoked"), time: 500, partition: current})
	snapshot.keep(key, &Entry{lifecycle: lifecycleCode("revoked"), time: 500, partition: other})
	if snapshot.States[key].Partition() != current {
		t.Fatal("of two copies with one time, the one off the key's partition replaced the one on it")
	}
	snapshot.keep(key, &Entry{lifecycle: lifecycleCode("revoked"), time: 2000, partition: 1})
	snapshot.keep(key, &Entry{lifecycle: lifecycleCode("active"), time: 1000, partition: 0})
	if got := snapshot.States[key].Lifecycle(); got != "revoked" {
		t.Fatalf("an older copy on another partition replaced the later one: %q", got)
	}
	snapshot.keep(key, &Entry{lifecycle: lifecycleCode("quarantined"), time: 1500, partition: 1})
	if got := snapshot.States[key].Lifecycle(); got != "quarantined" {
		t.Fatalf("the last record on the key's own partition did not win: %q", got)
	}
}
