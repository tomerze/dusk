//go:build integration

package campaign_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

const (
	device       = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	installation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
)

func definition() campaign.Definition {
	return campaign.Definition{
		Name:     "upgrade",
		Selector: `country == "US"`,
		Action:   campaign.Action{Kind: campaign.KindEnsureVersion, Version: "0.2.0", Script: "upgrade"},
		Policy:   campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, Gates: campaign.Gates{MinSample: 1}},
	}
}

func TestCampaignLifecycleInPostgres(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	operation := context.Background()
	now := time.Now().UTC().Truncate(time.Microsecond)
	created, failure := store.Create(operation, definition(), "operator@example.org", now)
	if failure != nil {
		test.Fatal(failure)
	}
	if created.Status != campaign.StatusDraft || len(created.Salt) != 32 || created.Policy.Retry.MaxAttempts != 3 || created.Action.VersionKey != "dusk.version" {
		test.Fatalf("created %+v", created)
	}
	edited := definition()
	edited.Name = "upgrade to 0.2.0"
	if _, failure := store.UpdateDefinition(operation, created.ID, edited, created.Version+1, "operator@example.org", now); !errors.Is(failure, campaign.ErrVersionConflict) {
		test.Fatalf("stale version: %v", failure)
	}
	updated, failure := store.UpdateDefinition(operation, created.ID, edited, created.Version, "operator@example.org", now)
	if failure != nil || updated.Name != "upgrade to 0.2.0" || updated.Version != created.Version+1 {
		test.Fatalf("updated %+v %v", updated, failure)
	}
	started, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: now})
	if failure != nil || started.Status != campaign.StatusRunning || started.SelectorSQL != "(n.country = $1::text)" || len(started.SelectorParameters) != 1 || started.PhaseStartedAt == nil {
		test.Fatalf("started %+v %v", started, failure)
	}
	if _, failure := store.UpdateDefinition(operation, created.ID, edited, started.Version, "operator@example.org", now); !errors.Is(failure, campaign.ErrNotDraft) {
		test.Fatalf("edited a running campaign: %v", failure)
	}
	paused, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionPause, PauseKind: campaign.PauseGate, Reason: "failure rate 0.62 in os_build=22631.4317, 41 of 66", Actor: "twilight", Now: now.Add(time.Minute)})
	if failure != nil || paused.Status != campaign.StatusPaused || *paused.PauseKind != campaign.PauseGate {
		test.Fatalf("paused %+v %v", paused, failure)
	}
	if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionResume, Actor: "operator@example.org", Now: now.Add(2 * time.Minute)}); !errors.Is(failure, campaign.ErrGateOverrideRequired) {
		test.Fatalf("resumed a gate pause without an override: %v", failure)
	}
	resumed, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionResume, OverrideGate: true, Reason: "one bad build, pulled", Actor: "operator@example.org", Now: now.Add(3 * time.Minute)})
	if failure != nil || resumed.Status != campaign.StatusRunning || resumed.GateOverrideAfter == nil || resumed.PhasePausedSeconds != 120 || resumed.PausedAt != nil {
		test.Fatalf("resumed %+v %v", resumed, failure)
	}
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	row := campaign.NewRow(created.ID, device, installation, 0, campaign.StatePending)
	inserted, applied, failure := store.InsertRow(operation, term, row)
	if failure != nil || !applied {
		test.Fatalf("insert %v %v", applied, failure)
	}
	if _, applied, _ := store.InsertRow(operation, term, row); applied {
		test.Fatal("inserted a row twice")
	}
	if _, applied, _ := store.InsertRow(operation, term+1, campaign.NewRow(created.ID, device, "b"+installation[1:], 0, campaign.StatePending)); applied {
		test.Fatal("a stale term inserted a row")
	}
	next, _ := inserted.Dispatch(created.ID, campaign.Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 9}, now, now.Add(time.Hour), map[string]string{"os_build": "13"})
	attempt := 1
	intended := campaign.IntendedProcess{Pid: next.Pid, CampaignID: &created.ID, Attempt: &attempt, DeviceID: device, InstallationID: installation, ActionKind: "ensure_version",
		Principal: "operator@example.org", Subject: created.Subject(), ExpiresAt: now.Add(2 * time.Hour), MaxCommands: 3, DefaultShellCommands: 8, At: now}
	if _, applied, failure := store.Dispatching(operation, term+1, inserted, next, intended); failure != nil || applied {
		test.Fatalf("a stale term dispatched: %v %v", applied, failure)
	}
	dispatching, applied, failure := store.Dispatching(operation, term, inserted, next, intended)
	if failure != nil || !applied || dispatching.Revision != 2 {
		test.Fatalf("dispatching %+v %v %v", dispatching, applied, failure)
	}
	if _, applied, _ := store.UpdateRow(operation, term, inserted, next); applied {
		test.Fatal("a stale revision applied")
	}
	stored, failure := store.Row(operation, created.ID, device, installation)
	if failure != nil || stored.State != campaign.StateDispatching || stored.Pid != campaign.DerivePid(created.ID, device, installation, 1) || stored.Breakdown["os_build"] != "13" || stored.Epoch != 9 {
		test.Fatalf("stored %+v %v", stored, failure)
	}
	var largest string
	if failure := pool.QueryRow(operation, `update campaign_nodes set pid = 18446744073709551615 where campaign_id = $1 and device_id = $2 and installation_id = $3 returning pid::text`,
		created.ID, device, installation).Scan(&largest); failure != nil || largest != "18446744073709551615" {
		test.Fatalf("the largest pid does not round-trip: %q %v", largest, failure)
	}
	if read, failure := store.Row(operation, created.ID, device, installation); failure != nil || read.Pid != campaign.Pid(18446744073709551615) {
		test.Fatalf("the largest pid reads back as %d (%v)", read.Pid, failure)
	}
	if _, failure := pool.Exec(operation, `update campaign_nodes set pid = $4::text::numeric where campaign_id = $1 and device_id = $2 and installation_id = $3`,
		created.ID, device, installation, stored.Pid.String()); failure != nil {
		test.Fatal(failure)
	}
	undelivered, _ := stored.Undelivered("unreachable", "", now, 0)
	undelivered, _, _ = store.UpdateRow(operation, term, stored, undelivered)
	again, _ := undelivered.Dispatch(created.ID, campaign.Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 10}, now, now.Add(time.Hour), nil)
	intended.ExpiresAt, intended.At = now.Add(3*time.Hour), now.Add(time.Minute)
	if _, applied, failure := store.Dispatching(operation, term, undelivered, again, intended); failure != nil || !applied {
		test.Fatalf("resend %v %v", applied, failure)
	}
	var recorded, maxCommands, defaultShellCommands int
	var expires, lastDispatched time.Time
	if failure := pool.QueryRow(operation, `select count(*), max(expires_at), max(last_dispatched_at), max(max_commands), max(default_shell_commands) from intended_processes where pid = $1::text::numeric`, next.Pid.String()).
		Scan(&recorded, &expires, &lastDispatched, &maxCommands, &defaultShellCommands); failure != nil || recorded != 1 || !expires.Equal(now.Add(3*time.Hour)) || !lastDispatched.Equal(now.Add(time.Minute)) {
		test.Fatalf("the intended process after a resend: %d %v %v %v", recorded, expires, lastDispatched, failure)
	}
	if maxCommands != 6 || defaultShellCommands != 16 {
		test.Fatalf("a resend must add its own command budget: max_commands %d, default_shell_commands %d", maxCommands, defaultShellCommands)
	}
	pending := campaign.NewRow(created.ID, device, "c"+installation[1:], 0, campaign.StatePending)
	if _, applied, _ := store.InsertRow(operation, term, pending); !applied {
		test.Fatal("second row")
	}
	counts, failure := store.CountRows(operation, created.ID)
	if failure != nil || len(counts) != 2 {
		test.Fatalf("counts %+v %v", counts, failure)
	}
	if failure := store.WriteCounters(operation, created.ID, counts); failure != nil {
		test.Fatal(failure)
	}
	stored2, _ := store.Counters(operation, []uuid.UUID{created.ID})
	if len(stored2[created.ID]) != 2 {
		test.Fatalf("stored counters %+v", stored2)
	}
	aborted, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionAbort, Reason: "wrong version", Actor: "operator@example.org", Now: now.Add(time.Hour)})
	if failure != nil || aborted.Status != campaign.StatusAborted || aborted.FinishedAt == nil {
		test.Fatalf("aborted %+v %v", aborted, failure)
	}
	cancelled, _ := store.Row(operation, created.ID, device, "c"+installation[1:])
	inFlight, _ := store.Row(operation, created.ID, device, installation)
	if cancelled.State != campaign.StateCancelled || inFlight.State != campaign.StateDispatching {
		test.Fatalf("abort cancelled %s and left %s", cancelled.State, inFlight.State)
	}
	if failure := store.RecordDispatch(operation, created.ID, now); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionArchive, Actor: "operator@example.org", Now: now.Add(time.Hour)}); !errors.Is(failure, campaign.ErrNotArchivable) {
		test.Fatalf("archived too early: %v", failure)
	}
	archived, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionArchive, Actor: "operator@example.org", Now: now.Add(2 * time.Hour)})
	if failure != nil || archived.Status != campaign.StatusArchived {
		test.Fatalf("archived %+v %v", archived, failure)
	}
	if _, failure := store.Row(operation, created.ID, device, installation); failure == nil {
		test.Fatal("rows survived archiving")
	}
	events, failure := store.Events(operation, created.ID, 0, 100)
	if failure == nil && len(events) != 0 {
		test.Fatalf("events survived archiving: %d", len(events))
	}
	page, failure := store.List(operation, []campaign.Status{campaign.StatusArchived}, "", 10)
	if failure != nil || len(page.Campaigns) != 1 {
		test.Fatalf("list %+v %v", page, failure)
	}
}

func TestOverlapCountsSharedNodes(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	operation := context.Background()
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id, country) values ($1, $2, 'US'), ($1, $3, 'DE')`, device, installation, "d"+installation[1:]); failure != nil {
		test.Fatal(failure)
	}
	now := time.Now()
	first, _ := store.Create(operation, definition(), "a", now)
	if _, failure := store.Transition(operation, first.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "a", Now: now}); failure != nil {
		test.Fatal(failure)
	}
	other := definition()
	other.Selector = `country in ["US", "DE"]`
	second, _ := store.Create(operation, other, "a", now)
	count, overlapping, failure := store.Overlap(operation, second)
	if failure != nil || count != 1 || len(overlapping) != 1 || overlapping[0] != first.ID {
		test.Fatalf("overlap %d %v %v", count, overlapping, failure)
	}
}
