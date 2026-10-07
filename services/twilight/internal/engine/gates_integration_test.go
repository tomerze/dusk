//go:build integration

package engine

import (
	"context"
	"fmt"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/testsupport"
)

func TestGateTalliesCountEveryRowByItsOutcome(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	created := time.Now().Add(-2 * time.Hour)
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "canary", Percent: 10, BakeSeconds: 900}, {Name: "all", Percent: 100, BakeSeconds: 900}},
		NodeTimeoutSeconds: 60, Gates: campaign.Gates{MinSample: 1, MaxFailureRate: 0.5, MaxSilentRate: 0.5, SilentWindowSeconds: 600, Breakdown: []string{"os_build"}}}
	target, failure := store.Create(operation, campaign.Definition{Name: "gate math", Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", created)
	if failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: created}); failure != nil {
		test.Fatal(failure)
	}
	now := time.Now()
	early, late := now.Add(-time.Hour), now.Add(-time.Minute)
	rows := []struct {
		phase      int
		state      campaign.NodeState
		lastStatus string
		dispatched time.Time
		silent     *bool
		build      string
	}{
		{0, campaign.StateSucceeded, "succeeded", early, nil, "a"},
		{0, campaign.StateSucceeded, "resolved", early, nil, "a"},
		{0, campaign.StateFailed, "resolved", early, nil, "a"},
		{0, campaign.StateFailed, "failed", early, nil, "b"},
		{0, campaign.StateBackoff, "failed", early, nil, "b"},
		{0, campaign.StateUnknown, "ended", early, nil, "b"},
		{0, campaign.StateFailed, "denied", early, nil, "b"},
		{0, campaign.StatePending, "unreachable", early, nil, "b"},
		{0, campaign.StateSucceeded, "succeeded", late, new(true), "a"},
		{0, campaign.StateFailed, "resolved", late, nil, "a"},
		{1, campaign.StateFailed, "failed", early, nil, "a"},
	}
	for index, row := range rows {
		finished := row.dispatched.Add(time.Second)
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, last_status, dispatched_at, finished_at, silent, breakdown, revision)
			values ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9, jsonb_build_object('os_build', $10::text), 1)`,
			target.ID, fmt.Sprintf("%032x", index), fmt.Sprintf("%032x", 1000+index), row.phase, row.state, row.lastStatus, row.dispatched, finished, row.silent, row.build); failure != nil {
			test.Fatal(failure)
		}
	}
	overall, groups, phaseRows, failure := Tallies(operation, pool, target, now)
	if failure != nil {
		test.Fatal(failure)
	}
	if want := (Tally{Succeeded: 3, Failed: 5, Eligible: 2}); overall != want || phaseRows != 10 {
		test.Fatalf("overall %+v of %d rows, want %+v of 10: a row resolved failed counts in F and one resolved succeeded in S, a denied row in neither", overall, phaseRows, want)
	}
	if groups["os_build"]["a"] != (Tally{Succeeded: 3, Failed: 2, Eligible: 2}) || groups["os_build"]["b"] != (Tally{Failed: 3}) {
		test.Fatalf("groups %+v", groups)
	}
	report := Judge(target.Policy.Gates, nil, overall, groups, phaseRows, false)
	if report.Verdict != VerdictFail || report.FailingGroup != "os_build=b" {
		test.Fatalf("the gate over these tallies: %+v", report)
	}
	resumed := now.Add(-30 * time.Minute)
	if _, failure := pool.Exec(operation, `update campaigns set gate_override_after = $2 where id = $1`, target.ID, resumed); failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Get(operation, target.ID); failure != nil {
		test.Fatal(failure)
	}
	overall, _, phaseRows, failure = Tallies(operation, pool, target, now)
	if failure != nil {
		test.Fatal(failure)
	}
	if want := (Tally{Succeeded: 1, Failed: 1}); overall != want || phaseRows != 10 {
		test.Fatalf("after a gate override: %+v of %d rows, want %+v from the rows dispatched since the resume, of all 10 rows of the phase", overall, phaseRows, want)
	}
	advanced, failure := store.AdvancePhase(operation, target.ID, 0, now, nil)
	if failure != nil || !advanced {
		test.Fatalf("advance %v %v", advanced, failure)
	}
	if target, failure = store.Get(operation, target.ID); failure != nil || target.GateOverrideAfter != nil {
		test.Fatalf("the gate override outlived its phase: %+v %v", target, failure)
	}
	overall, _, phaseRows, failure = Tallies(operation, pool, target, now)
	if failure != nil {
		test.Fatal(failure)
	}
	if want := (Tally{Succeeded: 3, Failed: 6, Eligible: 2}); overall != want || phaseRows != 11 {
		test.Fatalf("the next phase's cumulative gate: %+v of %d rows, want every row of both phases %+v of 11", overall, phaseRows, want)
	}
}
