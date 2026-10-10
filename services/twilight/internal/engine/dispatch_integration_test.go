//go:build integration

package engine

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

type recordingIntended struct {
	mutex   sync.Mutex
	events  []string
	failure error
}

func (fake *recordingIntended) ProduceIntendedProcess(_ context.Context, intended kafka.IntendedProcess) error {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	if fake.failure != nil {
		return fake.failure
	}
	fake.events = append(fake.events, "record "+intended.Key())
	return nil
}

func (fake *recordingIntended) ClearIntendedProcess(_ context.Context, key string) error {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	if fake.failure != nil {
		return fake.failure
	}
	fake.events = append(fake.events, "tombstone "+key)
	return nil
}

func (fake *recordingIntended) taken() []string {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	events := fake.events
	fake.events = nil
	return events
}

type recordingDawn struct {
	mutex       sync.Mutex
	dispatches  [][]dawn.Work
	intended    *recordingIntended
	unpublished []campaign.Pid
}

func (fake *recordingDawn) Dispatch(_ context.Context, node dawn.NodeRef, work []dawn.Work) ([]campaign.Pid, error) {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	if fake.intended != nil {
		fake.intended.mutex.Lock()
		for _, item := range work {
			if !slices.Contains(fake.intended.events, "record "+kafka.IntendedProcessKey(node.DeviceID, node.InstallationID, item.Pid.String())) {
				fake.unpublished = append(fake.unpublished, item.Pid)
			}
		}
		fake.intended.mutex.Unlock()
	}
	fake.dispatches = append(fake.dispatches, work)
	accepted := make([]campaign.Pid, len(work))
	for index, item := range work {
		accepted[index] = item.Pid
	}
	return accepted, nil
}

func (fake *recordingDawn) Reap(context.Context, dawn.NodeRef, []campaign.Pid) error {
	return nil
}

func (fake *recordingDawn) Facts(context.Context, dawn.NodeRef, campaign.Pid, []string) (dawn.Facts, error) {
	return dawn.Facts{}, nil
}

func (fake *recordingDawn) Logs(context.Context, dawn.NodeRef, campaign.Pid, string, int, *string) (string, error) {
	return "", nil
}

func (fake *recordingDawn) Files(context.Context, dawn.NodeRef, campaign.Pid, string, *string) (string, error) {
	return "", nil
}

func TestOneDispatchCarriesEveryDueCampaignInBatchOrder(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	fake := &recordingDawn{}
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Dawn: fake, Intended: &recordingIntended{}, Logger: testsupport.Logger()})
	key := NodeKey{DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"}
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, NodeTimeoutSeconds: 30, Gates: campaign.Gates{MinSample: 1}}
	started := time.Now().Add(-time.Hour)
	definitions := []struct {
		name   string
		action campaign.Action
	}{
		{"converge the version", campaign.Action{Kind: campaign.KindEnsureVersion, Version: "0.2.0", Script: "upgrade"}},
		{"early script", campaign.Action{Kind: campaign.KindRunScript, Script: "ps", CollectFiles: []string{"/var/log/syslog", "/var/log/dusk.log"}, StreamLogs: &campaign.StreamLogs{Level: "info", DurationSeconds: 20}}},
		{"converge the config", campaign.Action{Kind: campaign.KindEnsureConfig, ConfigHash: "c0ffee", Script: "apply"}},
		{"late script", campaign.Action{Kind: campaign.KindRunScript, Script: "date"}},
		{"isolate", campaign.Action{Kind: campaign.KindQuarantine, Script: "isolate"}},
	}
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	names := map[uuid.UUID]string{}
	for index, definition := range definitions {
		created, failure := store.Create(operation, campaign.Definition{Name: definition.name, Selector: "has(device_id)", Action: definition.action, Policy: policy}, "operator@example.org", started)
		if failure != nil {
			test.Fatal(failure)
		}
		if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: started.Add(time.Duration(index) * time.Minute)}); failure != nil {
			test.Fatal(failure)
		}
		if _, applied, failure := store.InsertRow(operation, term, campaign.NewRow(created.ID, key.DeviceID, key.InstallationID, 0, campaign.StatePending)); failure != nil || !applied {
			test.Fatalf("row of %s: %v %v", definition.name, applied, failure)
		}
		names[created.ID] = definition.name
	}
	if failure := engine.registry.load(operation, term); failure != nil {
		test.Fatal(failure)
	}
	now := time.Now()
	engine.view.ApplyConnection(kafka.Connection{Envelope: kafka.Envelope{Time: kafka.FormatTime(now)}, Event: "connected", DeviceID: key.DeviceID, InstallationID: key.InstallationID,
		NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 7, Instance: "nightfall-0", InnerAddress: "nightfall-0.nightfall-inner:8444", ConnectedAt: kafka.FormatTime(now)})
	for identifier := range names {
		engine.dispatcher.offer(identifier, key)
	}
	chosen, _ := engine.dispatcher.next(now.Add(time.Hour))
	if !chosen.found || len(chosen.campaigns) != len(definitions) {
		test.Fatalf("the node due in %d campaigns went out with %+v", len(definitions), chosen)
	}
	engine.dispatcher.dispatch(operation, term, chosen)
	if len(fake.dispatches) != 1 {
		test.Fatalf("%d dispatch calls for one node", len(fake.dispatches))
	}
	work := fake.dispatches[0]
	var order []string
	for _, item := range work {
		identifier := uuid.MustParse(*item.CampaignID)
		order = append(order, names[identifier])
	}
	if want := []string{"isolate", "early script", "late script", "converge the config", "converge the version"}; !slices.Equal(order, want) {
		test.Fatalf("batch order %v, want %v", order, want)
	}
	cumulative := time.Duration(0)
	for _, item := range work {
		identifier := uuid.MustParse(*item.CampaignID)
		target := engine.registry.campaignOf(identifier)
		if time.Duration(item.TimeoutSeconds)*time.Second != target.ProcessTimeout() {
			test.Fatalf("%s: dawn was given %d s for work whose row waits %s", names[identifier], item.TimeoutSeconds, target.ProcessTimeout())
		}
		cumulative += target.ProcessTimeout()
		row, failure := store.Row(operation, identifier, key.DeviceID, key.InstallationID)
		if failure != nil || row.State != campaign.StateDispatched || row.DispatchedAt == nil || row.DeadlineAt == nil {
			test.Fatalf("%s: row %+v %v", names[identifier], row, failure)
		}
		if deadline := row.DeadlineAt.Sub(*row.DispatchedAt); deadline < cumulative+time.Minute-time.Millisecond || deadline > cumulative+time.Minute+time.Millisecond {
			test.Fatalf("%s: deadline %s after dispatch, want the batch's timeouts so far (%s) plus a minute", names[identifier], deadline, cumulative)
		}
	}
	if early := work[1]; early.TimeoutSeconds != 30*3+20 {
		test.Fatalf("a script with two files and a 20 s log stream was given %d s", early.TimeoutSeconds)
	}
	for identifier := range names {
		if tokens := engine.registry.get(identifier).bucket.tokens; tokens != 9 {
			test.Fatalf("%s holds %v tokens after one process", names[identifier], tokens)
		}
	}
}

type refusingDawn struct {
	recordingDawn
	refusal error
}

func (fake *refusingDawn) Dispatch(context.Context, dawn.NodeRef, []dawn.Work) ([]campaign.Pid, error) {
	return nil, fake.refusal
}

func TestADispatchDawnRefusesFailsItsRowAndPausesTheCampaign(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, NodeTimeoutSeconds: 30, Gates: campaign.Gates{MinSample: 1}}
	cases := []struct {
		name      string
		refusal   error
		unreached int
		state     campaign.NodeState
		status    campaign.Status
	}{
		{"forbidden", &dawn.Error{Outcome: dawn.Rejected, Status: 403, Message: "the principal has no dispatcher role"}, 0, campaign.StateFailed, campaign.StatusPaused},
		{"unauthorized", &dawn.Error{Outcome: dawn.Rejected, Status: 401, Message: "no client certificate"}, 0, campaign.StateFailed, campaign.StatusPaused},
		{"rejected once", &dawn.Error{Outcome: dawn.Rejected, Status: 422, Message: "unprocessable"}, 0, campaign.StatePending, campaign.StatusRunning},
		{"rejected a third time", &dawn.Error{Outcome: dawn.Rejected, Status: 422, Message: "unprocessable"}, rejectedDispatches - 1, campaign.StateFailed, campaign.StatusPaused},
	}
	for index, scenario := range cases {
		fake := &refusingDawn{refusal: scenario.refusal}
		engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Dawn: fake, Intended: &recordingIntended{}, Logger: testsupport.Logger()})
		key := NodeKey{DeviceID: fmt.Sprintf("%032x", 0xe000+index), InstallationID: fmt.Sprintf("%032x", 0xf000+index)}
		created, failure := store.Create(operation, campaign.Definition{Name: scenario.name, Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", time.Now())
		if failure != nil {
			test.Fatal(failure)
		}
		if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: time.Now()}); failure != nil {
			test.Fatal(failure)
		}
		if _, applied, failure := store.InsertRow(operation, term, campaign.NewRow(created.ID, key.DeviceID, key.InstallationID, 0, campaign.StatePending)); failure != nil || !applied {
			test.Fatalf("%s: row %v %v", scenario.name, applied, failure)
		}
		if _, failure := pool.Exec(operation, `update campaign_nodes set unreached = $2 where campaign_id = $1`, created.ID, scenario.unreached); failure != nil {
			test.Fatal(failure)
		}
		if failure := engine.registry.load(operation, term); failure != nil {
			test.Fatal(failure)
		}
		now := time.Now()
		engine.view.ApplyConnection(kafka.Connection{Envelope: kafka.Envelope{Time: kafka.FormatTime(now)}, Event: "connected", DeviceID: key.DeviceID, InstallationID: key.InstallationID,
			NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 7, Instance: "nightfall-0", InnerAddress: "nightfall-0.nightfall-inner:8444", ConnectedAt: kafka.FormatTime(now)})
		engine.dispatcher.offer(created.ID, key)
		chosen, _ := engine.dispatcher.next(now.Add(time.Hour))
		if !chosen.found {
			test.Fatalf("%s: nothing went out", scenario.name)
		}
		engine.dispatcher.dispatch(operation, term, chosen)
		after, failure := store.Row(operation, created.ID, key.DeviceID, key.InstallationID)
		if failure != nil || after.State != scenario.state {
			test.Fatalf("%s: row %+v %v, want %s", scenario.name, after, failure, scenario.state)
		}
		if scenario.state == campaign.StateFailed && (after.LastStatus != string(campaign.ResultDenied) || after.LastError == "") {
			test.Fatalf("%s: a refused row must say so: %+v", scenario.name, after)
		}
		loaded, failure := store.Get(operation, created.ID)
		if failure != nil || loaded.Status != scenario.status {
			test.Fatalf("%s: campaign %+v %v, want %s", scenario.name, loaded, failure, scenario.status)
		}
		if scenario.status == campaign.StatusPaused && (loaded.PauseKind == nil || *loaded.PauseKind != campaign.PausePermission) {
			test.Fatalf("%s: paused as %v, want permission", scenario.name, loaded.PauseKind)
		}
		if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionAbort, Actor: "operator@example.org", Now: time.Now()}); failure != nil {
			test.Fatal(failure)
		}
	}
}

func TestDawnIsAskedOnlyForIntendedProcessesPublishedToNightfall(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	store := campaign.NewStore(pool)
	intended := &recordingIntended{failure: errors.New("the broker is down")}
	fake := &recordingDawn{intended: intended}
	engine := New(Dependencies{Config: config.Default(), Pool: pool, Campaigns: store, Inventory: inventory.NewStore(pool), Dawn: fake, Intended: intended, Logger: testsupport.Logger()})
	term, failure := store.Term(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	key := NodeKey{DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a14", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c71"}
	policy := campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, NodeTimeoutSeconds: 30, Gates: campaign.Gates{MinSample: 1}}
	created, failure := store.Create(operation, campaign.Definition{Name: "published first", Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy}, "operator@example.org", time.Now())
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: time.Now()}); failure != nil {
		test.Fatal(failure)
	}
	if _, applied, failure := store.InsertRow(operation, term, campaign.NewRow(created.ID, key.DeviceID, key.InstallationID, 0, campaign.StatePending)); failure != nil || !applied {
		test.Fatalf("row: %v %v", applied, failure)
	}
	if failure := engine.registry.load(operation, term); failure != nil {
		test.Fatal(failure)
	}
	dispatchOnce := func() {
		test.Helper()
		if _, failure := pool.Exec(operation, `update campaign_nodes set next_attempt_at = null where campaign_id = $1`, created.ID); failure != nil {
			test.Fatal(failure)
		}
		now := time.Now()
		engine.view.ApplyConnection(kafka.Connection{Envelope: kafka.Envelope{Time: kafka.FormatTime(now)}, Event: "connected", DeviceID: key.DeviceID, InstallationID: key.InstallationID,
			NamespaceID: "5d2e9a1c7b3f8e05", Epoch: 7, Instance: "nightfall-0", InnerAddress: "nightfall-0.nightfall-inner:8444", ConnectedAt: kafka.FormatTime(now)})
		engine.dispatcher.offer(created.ID, key)
		chosen, _ := engine.dispatcher.next(now.Add(time.Hour))
		if !chosen.found {
			test.Fatal("nothing went out")
		}
		engine.dispatcher.dispatch(operation, term, chosen)
	}

	dispatchOnce()
	if len(fake.dispatches) != 0 {
		test.Fatalf("dawn was asked for %d processes nightfall was never told of", len(fake.dispatches))
	}
	row, failure := store.Row(operation, created.ID, key.DeviceID, key.InstallationID)
	if failure != nil || row.State != campaign.StatePending || row.LastStatus != "intent_unpublished" || row.Pid == 0 {
		test.Fatalf("a row whose intended process was not published: %+v %v", row, failure)
	}
	pidKey := kafka.IntendedProcessKey(key.DeviceID, key.InstallationID, row.Pid.String())

	intended.mutex.Lock()
	intended.failure = nil
	intended.mutex.Unlock()
	if published := engine.publishPending(operation, term); published != 1 {
		test.Fatalf("the sweep published %d intended processes, want the one recorded before the broker went down", published)
	}
	if events := intended.taken(); !slices.Equal(events, []string{"record " + pidKey}) {
		test.Fatalf("the sweep wrote %v", events)
	}
	if published := engine.publishPending(operation, term); published != 0 {
		test.Fatalf("a second sweep published %d intended processes again", published)
	}

	dispatchOnce()
	if len(fake.dispatches) != 1 || len(fake.unpublished) != 0 {
		test.Fatalf("dispatches %d, of pids not yet published %v", len(fake.dispatches), fake.unpublished)
	}
	row, failure = store.Row(operation, created.ID, key.DeviceID, key.InstallationID)
	if failure != nil || row.Pid != fake.dispatches[0][0].Pid {
		test.Fatalf("the dispatched row %+v %v", row, failure)
	}
	pidKey = kafka.IntendedProcessKey(key.DeviceID, key.InstallationID, row.Pid.String())
	if events := intended.taken(); !slices.Equal(events, []string{"record " + pidKey}) {
		test.Fatalf("the dispatch wrote %v before asking dawn", events)
	}

	if _, failure := pool.Exec(operation, `update intended_processes set expires_at = now() - interval '1 second' where pid = $1::text::numeric`, row.Pid.String()); failure != nil {
		test.Fatal(failure)
	}
	if published := engine.publishPending(operation, term); published != 1 {
		test.Fatalf("the sweep published %d tombstones for an expired process", published)
	}
	if events := intended.taken(); !slices.Equal(events, []string{"tombstone " + pidKey}) {
		test.Fatalf("an expired process was published as %v", events)
	}
	if published := engine.publishPending(operation, term); published != 0 {
		test.Fatalf("a tombstoned process was published %d more times", published)
	}

	resend := campaign.IntendedProcess{Pid: row.Pid, CampaignID: &created.ID, Attempt: &row.Attempt, DeviceID: key.DeviceID, InstallationID: key.InstallationID,
		ActionKind: "run_script", Principal: "operator@example.org", Subject: "campaign:" + created.ID.String(), ExpiresAt: time.Now().Add(time.Hour), MaxCommands: 1, At: time.Now()}
	if failure := store.RecordIntendedProcess(operation, resend); failure != nil {
		test.Fatal(failure)
	}
	if published := engine.publishPending(operation, term); published != 1 {
		test.Fatalf("a resent process was published %d times", published)
	}
	if events := intended.taken(); !slices.Equal(events, []string{"record " + pidKey}) {
		test.Fatalf("a resent process was published as %v", events)
	}
	if recorded, failure := store.RecordReaped(operation, key.DeviceID, key.InstallationID, row.Pid, time.Now()); failure != nil || !recorded {
		test.Fatalf("reap: %v %v", recorded, failure)
	}
	if published := engine.publishPending(operation, term); published != 1 {
		test.Fatalf("a reaped process was published %d times", published)
	}
	if events := intended.taken(); !slices.Equal(events, []string{"tombstone " + pidKey}) {
		test.Fatalf("a reaped process was published as %v", events)
	}
}
