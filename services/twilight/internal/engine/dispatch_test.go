package engine

import (
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
)

func TestBusyDawnBacksOffFurtherEachTime(test *testing.T) {
	if first, second, capped := busyDelay(1, 1), busyDelay(2, 1), busyDelay(40, 1); first != 10*time.Second || second != 20*time.Second || capped != 5*time.Minute {
		test.Fatalf("busy delays %s %s %s", first, second, capped)
	}
	if busyDelay(3, 0.5) != 20*time.Second {
		test.Fatal("the delay is not drawn uniformly below its ceiling")
	}
}

func TestReapsTakeTokensFromTheirOwnBucket(test *testing.T) {
	settings := config.Default()
	settings.Engine.ReapsPerSecond = 2
	engine := &Engine{Dependencies: Dependencies{Config: settings}, leadership: &Leadership{}, evaluations: newEvaluationQueue(10)}
	engine.registry = newRegistry(engine)
	engine.dispatcher = newDispatcher(engine)
	now := time.Now().Add(time.Hour)
	keys := []NodeKey{{DeviceID: "a", InstallationID: "1"}, {DeviceID: "b", InstallationID: "2"}, {DeviceID: "c", InstallationID: "3"}}
	for _, key := range keys {
		engine.dispatcher.offerReap(key)
	}
	for index := range 2 {
		chosen, _ := engine.dispatcher.next(now)
		if !chosen.found || !chosen.reap || chosen.key != keys[index] {
			test.Fatalf("pick %d: %+v", index, chosen)
		}
	}
	chosen, wait := engine.dispatcher.next(now)
	if chosen.found || wait < 400*time.Millisecond || wait > time.Second {
		test.Fatalf("a third reap within the second went out (%+v) or waits %s", chosen, wait)
	}
	chosen, _ = engine.dispatcher.next(now.Add(time.Second))
	if !chosen.found || !chosen.reap || chosen.key != keys[2] {
		test.Fatalf("the third reap after its token came: %+v", chosen)
	}
	engine.dispatcher.offerReap(keys[2])
	if chosen, _ := engine.dispatcher.next(now.Add(2 * time.Second)); chosen.found {
		test.Fatalf("a reap went to a node with a dispatch in flight: %+v", chosen)
	}
}

func TestANodeDueInSeveralCampaignsGoesOutInOnePick(test *testing.T) {
	engine := &Engine{Dependencies: Dependencies{Config: config.Default()}, leadership: &Leadership{}, evaluations: newEvaluationQueue(10)}
	engine.registry = newRegistry(engine)
	engine.dispatcher = newDispatcher(engine)
	start := time.Now()
	identifiers := []uuid.UUID{uuid.MustParse("0192f3a4-0000-7000-8000-000000000001"), uuid.MustParse("0192f3a4-0000-7000-8000-000000000002"), uuid.MustParse("0192f3a4-0000-7000-8000-000000000003")}
	for _, identifier := range identifiers {
		engine.registry.runtimes[identifier] = &runtime{campaign: &campaign.Campaign{ID: identifier, Status: campaign.StatusRunning}, bucket: NewTokenBucket(1, 5, start), counters: map[int]map[campaign.NodeState]int64{}}
	}
	shared, other := NodeKey{DeviceID: "a", InstallationID: "1"}, NodeKey{DeviceID: "b", InstallationID: "2"}
	for _, identifier := range identifiers {
		engine.dispatcher.offer(identifier, shared)
	}
	engine.dispatcher.offer(identifiers[2], other)
	now := start.Add(time.Hour)
	chosen, _ := engine.dispatcher.next(now)
	slices.SortFunc(chosen.campaigns, func(left, right uuid.UUID) int { return strings.Compare(left.String(), right.String()) })
	if !chosen.found || chosen.key != shared || !slices.Equal(chosen.campaigns, identifiers) || !engine.dispatcher.inFlightFor(shared) {
		test.Fatalf("one node due in three campaigns: %+v", chosen)
	}
	for _, identifier := range identifiers {
		if tokens := engine.registry.runtimes[identifier].bucket.tokens; tokens != 4 {
			test.Fatalf("campaign %s holds %v tokens after one process; each campaign gives one", identifier, tokens)
		}
	}
	chosen, _ = engine.dispatcher.next(now)
	if !chosen.found || chosen.key != other || len(chosen.campaigns) != 1 || chosen.campaigns[0] != identifiers[2] {
		test.Fatalf("the second node: %+v", chosen)
	}
	if chosen, _ := engine.dispatcher.next(now); chosen.found {
		test.Fatalf("a node went out twice: %+v", chosen)
	}
}

func TestQueuesOfCampaignsNoLongerTrackedAreDropped(test *testing.T) {
	engine := &Engine{Dependencies: Dependencies{Config: config.Default()}, leadership: &Leadership{}, evaluations: newEvaluationQueue(10)}
	engine.registry = newRegistry(engine)
	engine.dispatcher = newDispatcher(engine)
	finished, running := uuid.MustParse("0192f3a4-0000-7000-8000-00000000000a"), uuid.MustParse("0192f3a4-0000-7000-8000-00000000000b")
	start := time.Now()
	for _, identifier := range []uuid.UUID{finished, running} {
		engine.registry.runtimes[identifier] = &runtime{campaign: &campaign.Campaign{ID: identifier, Status: campaign.StatusPaused}, bucket: NewTokenBucket(1, 1, start), counters: map[int]map[campaign.NodeState]int64{}}
	}
	keys := []NodeKey{{DeviceID: "a", InstallationID: "1"}, {DeviceID: "b", InstallationID: "2"}}
	for _, key := range keys {
		engine.dispatcher.offer(finished, key)
	}
	engine.dispatcher.offer(running, keys[0])
	delete(engine.registry.runtimes, finished)
	engine.dispatcher.next(start)
	engine.dispatcher.mutex.Lock()
	defer engine.dispatcher.mutex.Unlock()
	if _, kept := engine.dispatcher.queues[finished]; kept || engine.dispatcher.ready[keys[0]][finished] || engine.dispatcher.ready[keys[1]] != nil {
		test.Fatalf("a campaign the registry no longer tracks kept its queue: %v %v", engine.dispatcher.queues, engine.dispatcher.ready)
	}
	if len(engine.dispatcher.queues[running]) != 1 || !engine.dispatcher.ready[keys[0]][running] {
		test.Fatalf("a paused campaign lost its queue: %v %v", engine.dispatcher.queues, engine.dispatcher.ready)
	}
}

func TestDawnIsGivenTheWholeProcessTimeout(test *testing.T) {
	target := &campaign.Campaign{ID: uuid.MustParse("0192f3a4-0000-7000-8000-00000000000c"), Kind: campaign.KindRunScript,
		Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps", CollectFiles: []string{"/var/log/a", "/var/log/b"}, StreamLogs: &campaign.StreamLogs{Level: "info", DurationSeconds: 45}},
		Policy: campaign.Policy{NodeTimeoutSeconds: 60}}
	work := workFor(target, campaign.Row{Pid: 12808937078074471924, Attempt: 1})
	if work.TimeoutSeconds != 60*3+45 || time.Duration(work.TimeoutSeconds)*time.Second != target.ProcessTimeout() {
		test.Fatalf("dawn was given %d s for work whose row waits %s", work.TimeoutSeconds, target.ProcessTimeout())
	}
}
