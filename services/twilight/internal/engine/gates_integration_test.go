//go:build integration

package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
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

func TestGatesKeepWatchingAPausedCampaign(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Logger: testsupport.Logger()})
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	created := time.Now().Add(-2 * time.Hour)
	for _, onFailure := range []campaign.GateAction{campaign.GateActionPause, campaign.GateActionAbort} {
		policy := campaign.Policy{Phases: []campaign.Phase{{Name: "canary", Percent: 10, BakeSeconds: 60}, {Name: "all", Percent: 100, BakeSeconds: 60}},
			NodeTimeoutSeconds: 60, Gates: campaign.Gates{MinSample: 1, MaxFailureRate: 0.1, MaxSilentRate: 1}, Abort: campaign.Abort{OnGateFailure: onFailure}}
		target, failure := store.Create(operation, campaign.Definition{Name: "paused " + string(onFailure), Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", created)
		if failure != nil {
			test.Fatal(failure)
		}
		if _, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: created}); failure != nil {
			test.Fatal(failure)
		}
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, last_status, dispatched_at, finished_at, revision)
			values ($1, $2, $3, 0, 'failed', 1, 'failed', $4, $4, 1)`, target.ID, strings.Repeat("a", 32), strings.Repeat("b", 32), created); failure != nil {
			test.Fatal(failure)
		}
		paused, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionPause, Actor: "operator@example.org", Reason: "maintenance window", Now: created.Add(time.Minute)})
		if failure != nil {
			test.Fatal(failure)
		}
		for range 2 {
			if failure := engine.gate(operation, term, paused); failure != nil {
				test.Fatal(failure)
			}
		}
		after, failure := store.Get(operation, target.ID)
		if failure != nil {
			test.Fatal(failure)
		}
		events, failure := store.Events(operation, target.ID, 0, 100)
		if failure != nil {
			test.Fatal(failure)
		}
		gateEvents := 0
		for _, event := range events {
			if event.Kind == "gate_failed" || event.Kind == "failed" {
				gateEvents++
			}
		}
		switch onFailure {
		case campaign.GateActionPause:
			if after.Status != campaign.StatusPaused || after.PauseKind == nil || *after.PauseKind != campaign.PauseGate || gateEvents != 1 || after.CurrentPhase != 0 {
				test.Fatalf("a paused campaign whose gate fails stays paused and needs an override to resume, once: %+v, %d gate events", after, gateEvents)
			}
			if _, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionResume, Actor: "operator@example.org", Now: time.Now()}); !errors.Is(failure, campaign.ErrGateOverrideRequired) {
				test.Fatalf("resumed without a gate override: %v", failure)
			}
		case campaign.GateActionAbort:
			if after.Status != campaign.StatusFailed || gateEvents != 1 {
				test.Fatalf("a paused campaign whose policy aborts on a failing gate: %+v, %d gate events", after, gateEvents)
			}
		}
	}
}

func TestAGateThatPausesOrFailsACampaignRaisesAnAlert(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	alertStore := testsupport.AlertStore(test, pool)
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Alerts: alertStore, Logger: testsupport.Logger()})
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	created := time.Now().Add(-2 * time.Hour)
	tenant := "acme"
	for _, onFailure := range []campaign.GateAction{campaign.GateActionPause, campaign.GateActionAbort} {
		policy := campaign.Policy{Phases: []campaign.Phase{{Name: "canary", Percent: 10, BakeSeconds: 60}, {Name: "all", Percent: 100, BakeSeconds: 60}},
			NodeTimeoutSeconds: 60, Gates: campaign.Gates{MinSample: 1, MaxFailureRate: 0.1, MaxSilentRate: 1}, Abort: campaign.Abort{OnGateFailure: onFailure}}
		target, failure := store.Create(operation, campaign.Definition{Name: "gated " + string(onFailure), Tenant: &tenant, Selector: "has(device_id)",
			Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", created)
		if failure != nil {
			test.Fatal(failure)
		}
		started, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: created})
		if failure != nil {
			test.Fatal(failure)
		}
		if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, last_status, dispatched_at, finished_at, revision)
			values ($1, $2, $3, 0, 'failed', 1, 'failed', $4, $4, 1)`, target.ID, strings.Repeat("a", 32), strings.Repeat("b", 32), created); failure != nil {
			test.Fatal(failure)
		}
		if failure := engine.gate(operation, term, started); failure != nil {
			test.Fatal(failure)
		}
		kind := alerts.KindCampaignPausedByGate
		if onFailure == campaign.GateActionAbort {
			kind = alerts.KindCampaignFailedByPolicy
		}
		listed, failure := alertStore.List(operation, true, "", 100)
		if failure != nil {
			test.Fatal(failure)
		}
		var raised *alerts.Alert
		for index := range listed.Alerts {
			if listed.Alerts[index].Fingerprint == kind+":"+target.ID.String() {
				raised = &listed.Alerts[index]
			}
		}
		if raised == nil || raised.Severity != alerts.High || raised.Tenant == nil || *raised.Tenant != "acme" {
			test.Fatalf("%s: no %s alert among %+v", onFailure, kind, listed.Alerts)
		}
		var detail map[string]any
		_ = json.Unmarshal(raised.Detail, &detail)
		if detail["campaign_name"] != "gated "+string(onFailure) || detail["phase_name"] != "canary" || !strings.Contains(detail["reason"].(string), "failure rate 1.00") {
			test.Fatalf("%s: detail %v", onFailure, detail)
		}
		if onFailure == campaign.GateActionAbort {
			continue
		}
		if _, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionResume, Actor: "operator@example.org", OverrideGate: true,
			Reason: "the failing node was a lab machine", Now: time.Now()}); failure != nil {
			test.Fatal(failure)
		}
		resolved, failure := alertStore.Get(operation, raised.ID)
		if failure != nil || resolved.ResolvedAt == nil || *resolved.ResolvedBy != "operator@example.org" {
			test.Fatalf("resuming did not resolve the gate alert: %+v %v", resolved, failure)
		}
	}
}

func TestGatesOpenTheNextPhaseOverAnEmptyOneAndWaitForEveryNodeOfASmallOne(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Logger: testsupport.Logger()})
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	created := time.Now().Add(-30 * time.Minute)
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "half", Percent: 50, BakeSeconds: 3600}, {Name: "all", Percent: 100, BakeSeconds: 3600}},
		NodeTimeoutSeconds: 60, Gates: campaign.Gates{MinSample: 5, MaxFailureRate: 0.1, MaxSilentRate: 1}}
	target, failure := store.Create(operation, campaign.Definition{Name: "two nodes", Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", created)
	if failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: created}); failure != nil {
		test.Fatal(failure)
	}
	if failure := engine.gate(operation, term, target); failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Get(operation, target.ID); failure != nil || target.CurrentPhase != 0 {
		test.Fatalf("an empty phase opened the next one before its bake: %+v %v", target, failure)
	}
	if _, failure := pool.Exec(operation, `update campaigns set phase_started_at = phase_started_at - interval '1 hour' where id = $1`, target.ID); failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Get(operation, target.ID); failure != nil {
		test.Fatal(failure)
	}
	if failure := engine.gate(operation, term, target); failure != nil {
		test.Fatal(failure)
	}
	if target, failure = store.Get(operation, target.ID); failure != nil || target.CurrentPhase != 1 {
		test.Fatalf("an empty phase did not open the next one once it had baked: %+v %v", target, failure)
	}
	device, installation := strings.Repeat("c", 32), strings.Repeat("d", 32)
	if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, dispatched_at, revision)
		values ($1, $2, $3, 1, 'dispatched', 1, now(), 1)`, target.ID, device, installation); failure != nil {
		test.Fatal(failure)
	}
	report, failure := engine.Report(operation, target)
	if failure != nil || report.Verdict != VerdictHold || report.Reason != "waiting for sample 0 of 1" {
		test.Fatalf("a one-node phase under min_sample 5 before its node's result: %+v %v", report, failure)
	}
	if _, failure := pool.Exec(operation, `update campaign_nodes set state = 'succeeded', last_status = 'succeeded', finished_at = now() - interval '1 second'
		where campaign_id = $1`, target.ID); failure != nil {
		test.Fatal(failure)
	}
	if report, failure = engine.Report(operation, target); failure != nil || report.Verdict != VerdictPass {
		test.Fatalf("a one-node phase under min_sample 5 with its node's result: %+v %v", report, failure)
	}
}
