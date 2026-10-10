package engine

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/selector"
)

func target(test *testing.T, kind campaign.Kind, source string, started time.Time, mutate func(*campaign.Campaign)) Target {
	test.Helper()
	parsed, failure := selector.Parse(source)
	if failure != nil {
		test.Fatal(failure)
	}
	created := &campaign.Campaign{
		ID: uuid.New(), Kind: kind, Status: campaign.StatusRunning, Selector: source, Salt: []byte("0123456789abcdef0123456789abcdef"),
		Action:    campaign.Action{Kind: kind, Script: "run", Version: "0.2.0", VersionKey: campaign.DefaultVersionKey, ConfigHash: "c0ffee"},
		StartedAt: &started,
	}
	created.Policy.ApplyDefaults(kind)
	if mutate != nil {
		mutate(created)
	}
	return Target{Campaign: created, Selector: parsed}
}

func testNode(index int, country, version string) *inventory.Node {
	facts, _ := json.Marshal(map[string]string{"dusk.version": version})
	return &inventory.Node{DeviceID: device(index), InstallationID: device(index + 1000), Lifecycle: "active", Country: &country, DuskVersion: &version, FactsJSON: facts}
}

var online = &Session{NamespaceID: namespace(1), Epoch: 100, Instance: "nightfall-0"}

func only(test *testing.T, decisions []Decision) Decision {
	test.Helper()
	if len(decisions) != 1 {
		test.Fatalf("%d decisions: %+v", len(decisions), decisions)
	}
	return decisions[0]
}

func TestNewMatchingNodeJoinsAndIsDispatched(test *testing.T) {
	run := target(test, campaign.KindRunScript, `country == "US"`, base, nil)
	decision := only(test, Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), nil, online, base, 0.5))
	if decision.Previous != nil || !decision.Write || !decision.Dispatch || decision.Next.State != campaign.StatePending || decision.Next.Attempt != 1 {
		test.Fatalf("%+v", decision)
	}
	if decisions := Evaluate([]Target{run}, testNode(2, "DE", "0.1.0"), nil, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("a node outside the selector joined: %+v", decisions)
	}
	if decision := only(test, Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), nil, nil, base, 0.5)); decision.Dispatch || !decision.Write {
		test.Fatalf("an offline node joins but is not dispatched: %+v", decision)
	}
}

func TestOnlyOpenPhasesJoin(test *testing.T) {
	staged := target(test, campaign.KindRunScript, `has(country)`, base, func(created *campaign.Campaign) {
		created.Policy.Phases = []campaign.Phase{{Name: "canary", Percent: 10}, {Name: "all", Percent: 100}}
	})
	joined, waiting := 0, 0
	for index := range 200 {
		node := testNode(index, "US", "0.1.0")
		phase, _ := staged.Campaign.PhaseOf(node.DeviceID, node.InstallationID)
		decisions := Evaluate([]Target{staged}, node, nil, online, base, 0.5)
		if phase == 0 {
			joined++
			if len(decisions) != 1 || decisions[0].Next.Phase != 0 {
				test.Fatalf("a canary node did not join: %+v", decisions)
			}
		} else {
			waiting++
			if len(decisions) != 0 {
				test.Fatalf("a node of a later phase joined early: %+v", decisions)
			}
		}
	}
	if joined == 0 || waiting == 0 {
		test.Fatalf("joined %d, waiting %d", joined, waiting)
	}
	staged.Campaign.CurrentPhase = 1
	for index := range 200 {
		if len(Evaluate([]Target{staged}, testNode(index, "US", "0.1.0"), nil, online, base, 0.5)) != 1 {
			test.Fatal("a node did not join once its phase opened")
		}
	}
}

func TestConvergingCampaigns(test *testing.T) {
	ensure := target(test, campaign.KindEnsureVersion, `has(country)`, base, nil)
	decision := only(test, Evaluate([]Target{ensure}, testNode(1, "US", "0.2.0"), nil, online, base, 0.5))
	if decision.Next.State != campaign.StateSucceeded || decision.Dispatch {
		test.Fatalf("a node already at the desired version: %+v", decision)
	}
	succeeded := decision.Next
	drift := only(test, Evaluate([]Target{ensure}, testNode(1, "US", "0.1.9"), map[uuid.UUID]campaign.Row{ensure.Campaign.ID: succeeded}, online, base, 0.5))
	if drift.Next.State != campaign.StatePending || !drift.Dispatch || drift.Next.Attempt != 2 {
		test.Fatalf("drift: %+v", drift)
	}
	pending := campaign.NewRow(ensure.Campaign.ID, device(1), device(1001), 0, campaign.StatePending)
	satisfied := only(test, Evaluate([]Target{ensure}, testNode(1, "US", "0.2.0"), map[uuid.UUID]campaign.Row{ensure.Campaign.ID: pending}, online, base, 0.5))
	if satisfied.Next.State != campaign.StateSucceeded || satisfied.Dispatch {
		test.Fatalf("reported == desired must succeed a pending row: %+v", satisfied)
	}
	verifying := pending
	verifying.State, verifying.NamespaceID = campaign.StateVerifying, namespace(1)
	node := testNode(1, "US", "0.1.9")
	node.FactsNamespaceID = new(namespace(1))
	if decisions := Evaluate([]Target{ensure}, node, map[uuid.UUID]campaign.Row{ensure.Campaign.ID: verifying}, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("verification judged in the session the process ran in: %+v", decisions)
	}
	node.FactsNamespaceID = new(namespace(2))
	failed := only(test, Evaluate([]Target{ensure}, node, map[uuid.UUID]campaign.Row{ensure.Campaign.ID: verifying}, online, base, 0.5))
	if failed.Next.State != campaign.StateBackoff || failed.Next.Failures != 1 {
		test.Fatalf("still different in the next session: %+v", failed)
	}
}

func TestConflictingEnsureCampaigns(test *testing.T) {
	first := target(test, campaign.KindEnsureVersion, `has(country)`, base, nil)
	second := target(test, campaign.KindEnsureVersion, `country == "US"`, base.Add(time.Hour), nil)
	other := target(test, campaign.KindEnsureVersion, `country == "US"`, base.Add(2*time.Hour), func(created *campaign.Campaign) { created.Action.VersionKey = "app.version" })
	decisions := Evaluate([]Target{first, second, other}, testNode(1, "US", "0.1.0"), nil, online, base, 0.5)
	if len(decisions) != 3 {
		test.Fatalf("%+v", decisions)
	}
	if decisions[0].Next.State != campaign.StatePending || !decisions[0].Dispatch {
		test.Fatalf("the earliest campaign must own the node: %+v", decisions[0])
	}
	if decisions[1].Next.State != campaign.StateConflict || !decisions[1].Conflict || decisions[1].Dispatch {
		test.Fatalf("the later campaign must conflict: %+v", decisions[1])
	}
	if decisions[2].Next.State != campaign.StatePending {
		test.Fatalf("a different version key does not conflict: %+v", decisions[2])
	}
	conflict := decisions[1].Next
	cleared := only(test, Evaluate([]Target{second}, testNode(1, "US", "0.1.0"), map[uuid.UUID]campaign.Row{second.Campaign.ID: conflict}, online, base, 0.5))
	if cleared.Next.State != campaign.StatePending || !cleared.Dispatch {
		test.Fatalf("a conflict clears once the owner is gone: %+v", cleared)
	}
}

func TestMembershipIsSticky(test *testing.T) {
	run := target(test, campaign.KindRunScript, `country == "US"`, base, nil)
	pending := campaign.NewRow(run.Campaign.ID, device(1), device(1001), 0, campaign.StatePending)
	excluded := only(test, Evaluate([]Target{run}, testNode(1, "DE", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: pending}, online, base, 0.5))
	if excluded.Next.State != campaign.StateExcluded || excluded.Dispatch {
		test.Fatalf("a node that stopped matching: %+v", excluded)
	}
	back := only(test, Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: excluded.Next}, online, base, 0.5))
	if back.Next.State != campaign.StatePending || !back.Dispatch {
		test.Fatalf("a node that matches again: %+v", back)
	}
	finished := base
	succeeded := pending
	succeeded.State, succeeded.FinishedAt = campaign.StateSucceeded, &finished
	if decisions := Evaluate([]Target{run}, testNode(1, "DE", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: succeeded}, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("a terminal row changed because of the selector: %+v", decisions)
	}
	delivered := pending
	delivered.State = campaign.StateDelivered
	if decisions := Evaluate([]Target{run}, testNode(1, "DE", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: delivered}, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("an in-flight row was excluded: %+v", decisions)
	}
}

func TestPausedCampaignsNeitherJoinNorDispatch(test *testing.T) {
	run := target(test, campaign.KindRunScript, `country == "US"`, base, func(created *campaign.Campaign) { created.Status = campaign.StatusPaused })
	if decisions := Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), nil, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("a paused campaign took a node: %+v", decisions)
	}
	pending := campaign.NewRow(run.Campaign.ID, device(1), device(1001), 0, campaign.StatePending)
	if decisions := Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: pending}, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("a paused campaign dispatched: %+v", decisions)
	}
}

func TestQuarantineReachesOfflineNodes(test *testing.T) {
	quarantine := target(test, campaign.KindQuarantine, `country == "US"`, base, func(created *campaign.Campaign) { created.Action.Script = "" })
	decision := only(test, Evaluate([]Target{quarantine}, testNode(1, "US", "0.1.0"), nil, nil, base, 0.5))
	if !decision.Dispatch {
		test.Fatalf("a quarantine must reach an offline node: %+v", decision)
	}
}

func TestUndeliveredRowsWaitForBackoffOrANewSession(test *testing.T) {
	run := target(test, campaign.KindRunScript, `country == "US"`, base, nil)
	row := campaign.NewRow(run.Campaign.ID, device(1), device(1001), 0, campaign.StatePending)
	row, _ = row.Dispatch(run.Campaign.ID, campaign.Session{NamespaceID: namespace(1), Epoch: 100}, base, base.Add(time.Hour), nil)
	row, _ = row.Undelivered("unreachable", "", base, time.Minute)
	if decisions := Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: row}, online, base, 0.5); len(decisions) != 0 {
		test.Fatalf("dispatched during its backoff on the same session: %+v", decisions)
	}
	reconnected := &Session{NamespaceID: namespace(2), Epoch: 101}
	decision := only(test, Evaluate([]Target{run}, testNode(1, "US", "0.1.0"), map[uuid.UUID]campaign.Row{run.Campaign.ID: row}, reconnected, base, 0.5))
	if !decision.Dispatch || decision.Write {
		test.Fatalf("a reconnect must make the row due at once: %+v", decision)
	}
}
