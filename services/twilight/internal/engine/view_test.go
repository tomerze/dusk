package engine

import (
	"fmt"
	"testing"
	"time"

	"dusk/services/twilight/internal/kafka"
)

var base = time.Date(2026, 10, 7, 12, 0, 0, 0, time.UTC)

func at(seconds int) string {
	return kafka.FormatTime(base.Add(time.Duration(seconds) * time.Second))
}

func device(index int) string {
	return fmt.Sprintf("%032x", index)
}

func namespace(index int) string {
	return fmt.Sprintf("%016x", index)
}

func connection(event string, node, space int, epoch int64, instance string, seconds int) kafka.Connection {
	message := kafka.Connection{
		Envelope:       kafka.Envelope{Schema: "dusk.connections/v1", Time: at(seconds)},
		Event:          event,
		DeviceID:       device(node),
		InstallationID: device(node + 1000),
		NamespaceID:    namespace(space),
		Epoch:          epoch,
		Instance:       instance,
		InnerAddress:   instance + ".nightfall-inner:8444",
		ConnectedAt:    at(seconds),
	}
	if event == "disconnected" {
		reason := "node_closed"
		message.DisconnectReason = &reason
	}
	return message
}

func key(node int) NodeKey {
	return NodeKey{DeviceID: device(node), InstallationID: device(node + 1000)}
}

type censusSession struct {
	node, space int
	epoch       int64
	connected   int
}

func census(instance string, generation, snapshot int64, started, seconds int, chunks ...[]censusSession) []kafka.CensusRecord {
	var records []kafka.CensusRecord
	for index, chunk := range chunks {
		record := kafka.CensusRecord{Envelope: kafka.Envelope{Time: at(seconds)}, Record: "chunk", Instance: instance, Generation: generation, Index: index}
		for _, session := range chunk {
			record.Sessions = append(record.Sessions, kafka.CensusSession{
				DeviceID: device(session.node), InstallationID: device(session.node + 1000), NamespaceID: namespace(session.space),
				Epoch: session.epoch, ConnectedAt: at(session.connected), LastSeen: at(seconds),
			})
		}
		records = append(records, record)
	}
	return append(records, kafka.CensusRecord{
		Envelope: kafka.Envelope{Time: at(seconds)}, Record: "header", Instance: instance, InnerAddress: instance + ".nightfall-inner:8444",
		Generation: generation, SnapshotEpoch: snapshot, StartedAt: at(started), Full: true, ChunkCount: len(chunks), HeartbeatSeconds: 15,
	})
}

func heartbeat(instance string, generation int64, started, seconds int) kafka.CensusRecord {
	return kafka.CensusRecord{Envelope: kafka.Envelope{Time: at(seconds)}, Record: "header", Instance: instance, Generation: generation, StartedAt: at(started), HeartbeatSeconds: 15}
}

func apply(view *View, records []kafka.CensusRecord) []Change {
	var changes []Change
	for _, record := range records {
		changes = append(changes, view.ApplyCensus(record)...)
	}
	return changes
}

func TestConnectionsDiscardOlderEpochsPerNamespace(test *testing.T) {
	view := NewView()
	changes := view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 0))
	if len(changes) != 1 || !changes[0].Online || changes[0].WasOnline || changes[0].NewNamespace != namespace(1) {
		test.Fatalf("connect: %+v", changes)
	}
	if changes := view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 1)); len(changes) != 0 {
		test.Fatal("a repeated connected was applied")
	}
	if changes := view.ApplyConnection(connection("connected", 1, 1, 99, "nightfall-1", 1)); len(changes) != 0 {
		test.Fatal("an older epoch replaced a newer one")
	}
	if changes := view.ApplyConnection(connection("disconnected", 1, 1, 99, "nightfall-1", 2)); len(changes) != 0 || !view.Online(key(1)) {
		test.Fatal("an older disconnect closed a newer session")
	}
	changes = view.ApplyConnection(connection("connected", 1, 2, 50, "nightfall-1", 3))
	if len(changes) != 1 || !changes[0].WasOnline || changes[0].NewNamespace != namespace(2) || len(changes[0].Sessions) != 2 {
		test.Fatalf("second namespace: %+v", changes)
	}
	target, online := view.Target(key(1))
	if !online || target.Epoch != 100 || target.Instance != "nightfall-0" {
		test.Fatalf("target %+v", target)
	}
	changes = view.ApplyConnection(connection("disconnected", 1, 1, 100, "nightfall-0", 4))
	if len(changes) != 1 || !changes[0].Online || changes[0].Reason != ReasonDisconnected {
		test.Fatalf("one of two namespaces closed: %+v", changes)
	}
	changes = view.ApplyConnection(connection("disconnected", 1, 2, 50, "nightfall-1", 5))
	if len(changes) != 1 || changes[0].Online || !changes[0].WasOnline || view.OnlineCount() != 0 {
		test.Fatalf("last namespace closed: %+v", changes)
	}
	if changes := view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 6)); len(changes) != 0 {
		test.Fatal("a late connected for a closed session reopened it")
	}
	if changes := view.ApplyConnection(connection("connected", 1, 1, 101, "nightfall-2", 7)); len(changes) != 1 || !changes[0].Online {
		test.Fatal("a reconnect with a newer epoch was discarded")
	}
	if changes := view.ApplyConnection(connection("disconnected", 2, 9, 5, "nightfall-0", 8)); len(changes) != 0 {
		test.Fatal("a disconnect for an unknown session produced a change")
	}
	if changes := view.ApplyConnection(connection("connected", 2, 9, 5, "nightfall-0", 9)); len(changes) != 0 {
		test.Fatal("a connected older than a disconnect seen first was applied")
	}
}

func TestFullCensusIsAuthoritativeForItsInstance(test *testing.T) {
	view := NewView()
	view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 0))
	view.ApplyConnection(connection("connected", 2, 2, 300, "nightfall-0", 0))
	view.ApplyConnection(connection("connected", 3, 3, 150, "nightfall-1", 0))
	records := census("nightfall-0", 10, 200, -100, 10, []censusSession{{4, 4, 120, -50}}, []censusSession{{3, 3, 140, -60}})
	for _, record := range records[:2] {
		if changes := view.ApplyCensus(record); len(changes) != 0 {
			test.Fatalf("chunks applied before their header: %+v", changes)
		}
	}
	changes := view.ApplyCensus(records[2])
	if view.Online(key(1)) {
		test.Fatal("a session absent from the census with an epoch below the snapshot stayed online")
	}
	if !view.Online(key(2)) {
		test.Fatal("a session learned after the snapshot went offline")
	}
	if !view.Online(key(4)) {
		test.Fatal("a census session did not come online")
	}
	sessions := view.Sessions(key(3))
	if len(sessions) != 1 || sessions[0].Epoch != 150 || sessions[0].Instance != "nightfall-1" {
		test.Fatalf("an older census epoch replaced a newer session: %+v", sessions)
	}
	reasons := map[NodeKey]Change{}
	for _, change := range changes {
		reasons[change.Key] = change
	}
	if change := reasons[key(1)]; change.Online || !change.WasOnline || change.Reason != ReasonCensus {
		test.Fatalf("change for the dropped session: %+v", change)
	}
	if change := reasons[key(4)]; !change.Online || change.WasOnline {
		test.Fatalf("change for the new session: %+v", change)
	}
	if again := apply(view, census("nightfall-0", 9, 150, -100, 11, []censusSession{{1, 1, 100, -100}})); len(again) != 0 || view.Online(key(1)) {
		test.Fatal("an older generation was applied")
	}
}

func TestHeaderBeforeChunksWaitsForEveryChunk(test *testing.T) {
	view := NewView()
	records := census("nightfall-0", 20, 500, 0, 30, []censusSession{{1, 1, 100, 0}}, []censusSession{{2, 2, 200, 0}})
	header := records[2]
	if changes := view.ApplyCensus(header); len(changes) != 0 {
		test.Fatal("a header without chunks applied")
	}
	view.ApplyCensus(records[0])
	if view.Online(key(1)) {
		test.Fatal("a generation applied with a chunk missing")
	}
	view.ApplyCensus(records[1])
	if !view.Online(key(1)) || !view.Online(key(2)) {
		test.Fatal("a complete generation did not apply")
	}
}

func TestCensusDoesNotResurrectClosedSessions(test *testing.T) {
	view := NewView()
	view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 0))
	view.ApplyConnection(connection("disconnected", 1, 1, 100, "nightfall-0", 5))
	apply(view, census("nightfall-0", 10, 120, -100, 4, []censusSession{{1, 1, 100, 0}}))
	if view.Online(key(1)) {
		test.Fatal("a census taken before a disconnect resurrected the session")
	}
}

func TestRestartedInstanceDropsItsPreviousRun(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 120, 0, 10, []censusSession{{1, 1, 100, 1}, {2, 2, 110, 2}}))
	view.ApplyConnection(connection("connected", 3, 3, 130, "nightfall-0", 60))
	changes := view.ApplyCensus(heartbeat("nightfall-0", 11, 50, 61))
	if view.Online(key(1)) || view.Online(key(2)) {
		test.Fatal("sessions of the previous run survived a header with a newer started_at")
	}
	if !view.Online(key(3)) {
		test.Fatal("a session of the new run was dropped")
	}
	if len(changes) != 2 || changes[0].Reason != ReasonRestart || changes[0].Epoch != 100 || changes[1].Epoch != 110 {
		test.Fatalf("restart changes must carry the epoch of the session each one ended: %+v", changes)
	}
}

func TestCensusTimeoutDegradesAndRemovesCensusSessions(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 120, 0, 0, []censusSession{{1, 1, 100, 0}}))
	view.ApplyConnection(connection("connected", 2, 2, 200, "nightfall-0", 10))
	if changes := view.Expire(base.Add(44*time.Second), 5*time.Minute); len(changes) != 0 || view.Degraded() {
		test.Fatal("expired within three heartbeats")
	}
	changes := view.Expire(base.Add(46*time.Second), 5*time.Minute)
	if !view.Degraded() || view.Online(key(1)) || !view.Online(key(2)) {
		test.Fatalf("timeout: degraded %v, census session online %v, later session online %v", view.Degraded(), view.Online(key(1)), view.Online(key(2)))
	}
	if len(changes) != 1 || changes[0].Reason != ReasonCensusTimeout {
		test.Fatalf("timeout changes %+v", changes)
	}
	view.ApplyCensus(heartbeat("nightfall-0", 10, 0, 50))
	if view.Degraded() {
		test.Fatal("still degraded after the instance came back")
	}
}

func TestSkipDegradesUntilEveryInstanceSendsAFullCensus(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 100, 0, 0, []censusSession{{1, 1, 50, 0}}))
	apply(view, census("nightfall-1", 10, 100, 0, 0, []censusSession{{2, 2, 60, 0}}))
	view.MarkSkipped()
	if !view.Degraded() || view.TakeCaughtUp() {
		test.Fatal("not degraded after a skip")
	}
	view.ApplyCensus(heartbeat("nightfall-0", 10, 0, 5))
	apply(view, census("nightfall-0", 11, 150, 0, 300, []censusSession{{1, 1, 50, 0}}))
	if !view.Degraded() {
		test.Fatal("caught up before every instance sent a full census")
	}
	apply(view, census("nightfall-1", 11, 150, 0, 300, []censusSession{{2, 2, 60, 0}}))
	if view.Degraded() || !view.TakeCaughtUp() || view.TakeCaughtUp() {
		test.Fatal("catching up must clear degraded and be reported once")
	}
}

func TestShutdownHeaderEmptiesTheInstance(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 100, 0, 0, []censusSession{{1, 1, 50, 0}, {2, 2, 60, 0}}))
	apply(view, census("nightfall-0", 11, 100, 0, 30))
	if view.OnlineCount() != 0 {
		test.Fatalf("%d nodes online after a shutdown census", view.OnlineCount())
	}
}

func instanceNames(view *View) []string {
	var names []string
	for _, status := range view.Instances() {
		names = append(names, status.Instance)
	}
	return names
}

func TestShutdownHeaderForgetsTheInstance(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 100, 0, 0, []censusSession{{1, 1, 50, 0}}))
	apply(view, census("nightfall-1", 10, 100, 0, 0, []censusSession{{2, 2, 60, 0}}))
	apply(view, census("nightfall-1", 11, 100, 0, 30))
	if names := instanceNames(view); len(names) != 1 || names[0] != "nightfall-0" {
		test.Fatalf("an instance that shut down is still known: %v", names)
	}
	for seconds := 45; seconds <= 3600; seconds += 15 {
		view.ApplyCensus(heartbeat("nightfall-0", 10, 0, seconds))
		view.Expire(base.Add(time.Duration(seconds)*time.Second), 5*time.Minute)
	}
	if view.Degraded() || !view.Online(key(1)) || view.Online(key(2)) {
		test.Fatalf("an hour after an instance shut down: degraded %v", view.Degraded())
	}
	if oldest := view.OldestHeader(base.Add(time.Hour)); !oldest.Equal(base) {
		test.Fatalf("the replay starts from %v, not the census of the instance still running", oldest)
	}
}

func TestDeadInstanceIsForgottenAfterACensusInterval(test *testing.T) {
	view := NewView()
	apply(view, census("nightfall-0", 10, 100, 0, 0, []censusSession{{1, 1, 50, 0}}))
	apply(view, census("nightfall-1", 10, 100, 0, 0, []censusSession{{2, 2, 60, 0}}))
	view.ApplyConnection(connection("connected", 3, 3, 200, "nightfall-1", 10))
	for seconds := 15; seconds <= 60; seconds += 15 {
		view.ApplyCensus(heartbeat("nightfall-0", 10, 0, seconds))
	}
	view.Expire(base.Add(60*time.Second), 5*time.Minute)
	if !view.Degraded() || view.Online(key(2)) || !view.Online(key(3)) {
		test.Fatalf("census timeout: degraded %v, census session %v, later session %v", view.Degraded(), view.Online(key(2)), view.Online(key(3)))
	}
	view.MarkSkipped()
	apply(view, census("nightfall-0", 11, 150, 0, 300, []censusSession{{1, 1, 50, 0}}))
	if !view.TakeCaughtUp() {
		test.Fatal("a skip waited for a full census from an instance that is dead")
	}
	view.ApplyCensus(heartbeat("nightfall-0", 11, 0, 345))
	changes := view.Expire(base.Add(346*time.Second), 5*time.Minute)
	if view.Degraded() || view.Online(key(3)) || len(instanceNames(view)) != 1 {
		test.Fatalf("a census interval after the timeout: degraded %v, sessions left %v, instances %v", view.Degraded(), view.Online(key(3)), instanceNames(view))
	}
	if len(changes) != 1 || changes[0].Key != key(3) || changes[0].Epoch != 200 || changes[0].Reason != ReasonCensusTimeout {
		test.Fatalf("forgetting the instance: %+v", changes)
	}
}

func TestHeartbeatHeaderCompletesItsGeneration(test *testing.T) {
	view := NewView()
	records := census("nightfall-0", 10, 100, 0, 0, []censusSession{{1, 1, 50, 0}})
	view.ApplyCensus(records[0])
	beat := heartbeat("nightfall-0", 10, 0, 15)
	beat.ChunkCount = 1
	beat.SnapshotEpoch = 100
	if changes := view.ApplyCensus(beat); len(changes) != 1 || !view.Online(key(1)) {
		test.Fatalf("a generation whose full header was compacted away was not applied from its heartbeat: %+v", changes)
	}
	view.ApplyCensus(heartbeat("nightfall-0", 10, 0, 285))
	if oldest := view.OldestHeader(base.Add(290 * time.Second)); !oldest.Equal(base.Add(15 * time.Second)) {
		test.Fatalf("the replay starts from %v, not from the header that applied the census", oldest)
	}
}

func TestIdleNodesLeaveTheViewAfterTheReplayWindow(test *testing.T) {
	view := NewView()
	view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 0))
	view.ApplyConnection(connection("disconnected", 1, 1, 100, "nightfall-0", 10))
	view.ApplyConnection(connection("disconnected", 2, 2, 100, "nightfall-0", 20))
	view.ApplyConnection(connection("connected", 3, 3, 100, "nightfall-0", 0))
	view.ApplyConnection(connection("disconnected", 3, 3, 100, "nightfall-0", 30))
	view.ApplyConnection(connection("connected", 3, 4, 101, "nightfall-0", 40))
	horizon := 6 * time.Minute
	if evicted := view.Evict(base.Add(5*time.Minute), horizon); evicted != 0 {
		test.Fatalf("%d nodes evicted inside the window", evicted)
	}
	if changes := view.ApplyConnection(connection("connected", 1, 1, 100, "nightfall-0", 300)); len(changes) != 0 {
		test.Fatal("a replayed connect inside the window reopened a closed session")
	}
	if evicted := view.Evict(base.Add(7*time.Minute), horizon); evicted != 2 || len(view.nodes) != 1 || !view.Online(key(3)) {
		test.Fatalf("evicted %d, %d nodes left", evicted, len(view.nodes))
	}
	if len(view.idle) != 0 {
		test.Fatalf("%d idle entries kept", len(view.idle))
	}
}

func TestConnectionsSkipWhenTooFarBehind(test *testing.T) {
	cases := []struct {
		lag    int64
		age    time.Duration
		behind bool
	}{
		{200000, time.Second, false},
		{200001, time.Second, true},
		{1, 60 * time.Second, false},
		{1, 61 * time.Second, true},
		{0, time.Hour, false},
	}
	for _, testCase := range cases {
		if behind := fallenBehind(testCase.lag, base.Add(-testCase.age), base, 200000, 60*time.Second); behind != testCase.behind {
			test.Errorf("lag %d, newest record %s old: behind %v", testCase.lag, testCase.age, behind)
		}
	}
}
