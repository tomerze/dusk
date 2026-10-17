//go:build integration

package campaign_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/testsupport"
)

func TestFinishingACampaignRecountsEveryPartitionsCountersOnceAndForAll(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	operation := context.Background()
	created, failure := store.Create(operation, definition(), "operator@example.org", time.Now())
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: time.Now()}); failure != nil {
		test.Fatal(failure)
	}
	for index := range 12 {
		state := campaign.StatePending
		if index%4 == 0 {
			state = campaign.StateSucceeded
		}
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state) values ($1, $2, $3, 0, $4)`,
			created.ID, device, fmt.Sprintf("%032x", 0xd1000+index), state); failure != nil {
			test.Fatal(failure)
		}
	}
	stale := []campaign.Counter{{Partition: 0, Phase: 0, State: campaign.StatePending, Count: 99}}
	if failure := store.WriteCounters(operation, created.ID, []int32{0}, stale); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionAbort, Actor: "operator@example.org", Reason: "test", Now: time.Now()}); failure != nil {
		test.Fatal(failure)
	}
	totals := func() map[campaign.NodeState]int64 {
		summed, failure := store.Counters(operation, []uuid.UUID{created.ID})
		if failure != nil {
			test.Fatal(failure)
		}
		totals := map[campaign.NodeState]int64{}
		for _, counter := range summed[created.ID] {
			totals[counter.State] += counter.Count
		}
		return totals
	}
	if counted := totals(); counted[campaign.StateCancelled] != 9 || counted[campaign.StateSucceeded] != 3 || counted[campaign.StatePending] != 0 {
		test.Fatalf("the counters after the abort %v; want every partition recounted: 9 cancelled and 3 succeeded", counted)
	}
	if failure := store.WriteCounters(operation, created.ID, []int32{0, 1, 2}, stale); failure != nil {
		test.Fatal(failure)
	}
	if counted := totals(); counted[campaign.StateCancelled] != 9 || counted[campaign.StatePending] != 0 {
		test.Fatalf("an owner that had not noticed the abort overwrote the final counters: %v", counted)
	}
}
