//go:build integration

package engine

import (
	"context"
	"fmt"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

func TestASweepPassTakesOnlyItsPartitionsShareOfTheCandidates(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	owner := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Logger: testsupport.Logger()})
	ownEverySlice(test, owner)
	target := startedRunScript(test, store, "swept by shares")
	var devices, installations []string
	now := time.Now()
	for index := range 1500 {
		key := NodeKey{DeviceID: fmt.Sprintf("%032x", 0x5f0000+index), InstallationID: fmt.Sprintf("%032x", 0x1f0000+index)}
		devices, installations = append(devices, key.DeviceID), append(installations, key.InstallationID)
		owner.view.ApplyConnection(kafka.Connection{Envelope: kafka.Envelope{Time: kafka.FormatTime(now)}, Event: "connected", DeviceID: key.DeviceID, InstallationID: key.InstallationID,
			NamespaceID: fmt.Sprintf("%016x", index), Epoch: 1, Instance: "nightfall-0", InnerAddress: "nightfall-0.nightfall-inner:8444", ConnectedAt: kafka.FormatTime(now)})
	}
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id) select * from unnest($1::text[], $2::text[])`, devices, installations); failure != nil {
		test.Fatal(failure)
	}
	target, failure := store.Get(operation, target.ID)
	if failure != nil {
		test.Fatal(failure)
	}
	total := max(1000, int(2*target.Policy.Rate.PerSecond*float64(owner.Config.Engine.SweepIntervalSeconds)))
	share := (total + testsupport.NodePartitions - 1) / testsupport.NodePartitions
	for partition := range int32(testsupport.NodePartitions) {
		counted, failure := owner.sweepPartition(operation, target, partition, true, share)
		if failure != nil {
			test.Fatal(failure)
		}
		if counted.scanned > share || counted.candidates > share || counted.candidates == 0 {
			test.Fatalf("partition %d scanned %d nodes and took %d candidates in one pass; its share of the campaign's %d is %d", partition, counted.scanned, counted.candidates, total, share)
		}
	}
}
