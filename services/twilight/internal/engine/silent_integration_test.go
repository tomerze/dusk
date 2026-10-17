//go:build integration

package engine

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/testsupport"
)

func TestACanaryWhoseNodesWentSilentFailsItsGateBeforeAnyOwnerMarksThem(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Alerts: testsupport.AlertStore(test, pool), Logger: testsupport.Logger()})
	created := time.Now().Add(-2 * time.Hour)
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "canary", Percent: 10, BakeSeconds: 600}, {Name: "all", Percent: 100, BakeSeconds: 600}},
		NodeTimeoutSeconds: 60, Gates: campaign.Gates{MinSample: 2, MaxFailureRate: 0.5, MaxSilentRate: 0.4, SilentWindowSeconds: 600}}
	target, failure := store.Create(operation, campaign.Definition{Name: "silent canary", Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy},
		"operator@example.org", created)
	if failure != nil {
		test.Fatal(failure)
	}
	started, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: created})
	if failure != nil {
		test.Fatal(failure)
	}
	succeeded, departed := created.Add(time.Minute), created.Add(2*time.Minute)
	for index, gone := range []bool{true, true, false} {
		var eventAt any
		if gone {
			eventAt = departed
		}
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, last_status, dispatched_at, finished_at, event_at, revision)
			values ($1, $2, $3, 0, 'succeeded', 1, 'succeeded', $4, $4, $5, 1)`, target.ID, fmt.Sprintf("%032x", 0x51000+index), strings.Repeat("b", 32), succeeded, eventAt); failure != nil {
			test.Fatal(failure)
		}
	}
	overall, _, _, failure := Tallies(operation, pool, started, time.Now())
	if failure != nil || overall.Eligible != 3 || overall.Silent != 2 {
		test.Fatalf("the tallies count %+v (%v); two of three nodes left past their silent window and must count as silent before anyone marks them", overall, failure)
	}
	if failure := engine.gate(operation, started); failure != nil {
		test.Fatal(failure)
	}
	after, failure := store.Get(operation, target.ID)
	if failure != nil || after.CurrentPhase != 0 || after.Status != campaign.StatusPaused {
		test.Fatalf("a canary two thirds of whose nodes went silent is in phase %d, %s (%v); its gate must fail", after.CurrentPhase, after.Status, failure)
	}
	var marked int
	if failure := pool.QueryRow(operation, `select count(*) from campaign_nodes where campaign_id = $1 and silent is not null`, target.ID).Scan(&marked); failure != nil || marked != 0 {
		test.Fatalf("%d rows were marked (%v); the gate must not depend on the marks", marked, failure)
	}
}
