package engine

import (
	"sort"
	"sync"
	"time"

	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

type NodeKey = inventory.Key

type Session struct {
	NamespaceID  string
	Epoch        int64
	Instance     string
	InnerAddress string
	ConnectedAt  time.Time
	LastSeen     time.Time
}

const (
	ReasonConnected     = "connected"
	ReasonDisconnected  = "disconnected"
	ReasonCensus        = "census"
	ReasonRestart       = "census_restart"
	ReasonCensusTimeout = "census_timeout"
)

type Change struct {
	Key          NodeKey
	WasOnline    bool
	Online       bool
	Reason       string
	Epoch        int64
	At           time.Time
	NewNamespace string
	Sessions     []Session
}

type instanceState struct {
	startedAt         time.Time
	lastHeader        time.Time
	appliedHeaderAt   time.Time
	heartbeatSeconds  int
	snapshotEpoch     int64
	appliedGeneration int64
	pendingGeneration int64
	pendingHeader     *kafka.CensusRecord
	chunks            map[int][]kafka.CensusSession
	timedOut          bool
	needsFull         bool
	sessions          map[NodeKey]map[string]struct{}
}

type nodeState struct {
	sessions  map[string]*Session
	closed    map[string]int64
	idleSince time.Time
}

const closedNamespacesPerNode = 8

type idleNode struct {
	key   NodeKey
	since time.Time
}

type View struct {
	mutex      sync.RWMutex
	nodes      map[NodeKey]*nodeState
	instances  map[string]*instanceState
	idle       []idleNode
	skipped    bool
	caughtUp   bool
	onlineSize int
}

func NewView() *View {
	return &View{nodes: map[NodeKey]*nodeState{}, instances: map[string]*instanceState{}}
}

func (view *View) instance(name string) *instanceState {
	state, found := view.instances[name]
	if !found {
		state = &instanceState{chunks: map[int][]kafka.CensusSession{}, sessions: map[NodeKey]map[string]struct{}{}}
		view.instances[name] = state
	}
	return state
}

func (view *View) node(key NodeKey) *nodeState {
	state, found := view.nodes[key]
	if !found {
		state = &nodeState{sessions: map[string]*Session{}, closed: map[string]int64{}}
		view.nodes[key] = state
	}
	return state
}

func copySessions(state *nodeState) []Session {
	if state == nil || len(state.sessions) == 0 {
		return nil
	}
	sessions := make([]Session, 0, len(state.sessions))
	for _, session := range state.sessions {
		sessions = append(sessions, *session)
	}
	sort.Slice(sessions, func(left, right int) bool { return sessions[left].Epoch > sessions[right].Epoch })
	return sessions
}

func (view *View) attribute(key NodeKey, namespace, instance string) {
	index := view.instance(instance).sessions
	if index[key] == nil {
		index[key] = map[string]struct{}{}
	}
	index[key][namespace] = struct{}{}
}

func (view *View) detach(key NodeKey, namespace, instance string) {
	state, found := view.instances[instance]
	if !found {
		return
	}
	if namespaces, found := state.sessions[key]; found {
		delete(namespaces, namespace)
		if len(namespaces) == 0 {
			delete(state.sessions, key)
		}
	}
}

func (state *nodeState) close(namespace string, epoch int64) {
	if previous, found := state.closed[namespace]; found && previous >= epoch {
		return
	}
	state.closed[namespace] = epoch
	for len(state.closed) > closedNamespacesPerNode {
		oldest, oldestEpoch := "", int64(-1)
		for name, closedEpoch := range state.closed {
			if oldestEpoch < 0 || closedEpoch < oldestEpoch {
				oldest, oldestEpoch = name, closedEpoch
			}
		}
		delete(state.closed, oldest)
	}
}

func (view *View) set(key NodeKey, session Session) bool {
	state := view.node(key)
	wasOnline := len(state.sessions) > 0
	if existing, found := state.sessions[session.NamespaceID]; found && existing.Instance != session.Instance {
		view.detach(key, session.NamespaceID, existing.Instance)
	}
	stored := session
	state.sessions[session.NamespaceID] = &stored
	view.attribute(key, session.NamespaceID, session.Instance)
	if !wasOnline {
		view.onlineSize++
	}
	return wasOnline
}

func (view *View) markIdle(key NodeKey, state *nodeState, at time.Time) {
	if len(state.sessions) > 0 {
		return
	}
	state.idleSince = at
	view.idle = append(view.idle, idleNode{key: key, since: at})
}

func (view *View) remove(key NodeKey, namespace string, epoch int64, at time.Time) (bool, bool) {
	state, found := view.nodes[key]
	if !found {
		return false, false
	}
	existing, found := state.sessions[namespace]
	if !found {
		return false, false
	}
	wasOnline := len(state.sessions) > 0
	view.detach(key, namespace, existing.Instance)
	delete(state.sessions, namespace)
	state.close(namespace, max(epoch, existing.Epoch))
	if wasOnline && len(state.sessions) == 0 {
		view.onlineSize--
		view.markIdle(key, state, at)
	}
	return wasOnline, len(state.sessions) > 0
}

func (view *View) ApplyConnection(connection kafka.Connection) []Change {
	at, failure := kafka.ParseTime(connection.Time)
	if failure != nil {
		at = time.Now()
	}
	key := NodeKey{DeviceID: connection.DeviceID, InstallationID: connection.InstallationID}
	view.mutex.Lock()
	defer view.mutex.Unlock()
	state := view.node(key)
	existing := state.sessions[connection.NamespaceID]
	switch connection.Event {
	case "connected":
		if closedEpoch, closed := state.closed[connection.NamespaceID]; closed && closedEpoch >= connection.Epoch {
			return nil
		}
		if existing != nil && existing.Epoch >= connection.Epoch {
			return nil
		}
		connectedAt, failure := kafka.ParseTime(connection.ConnectedAt)
		if failure != nil {
			connectedAt = at
		}
		wasOnline := view.set(key, Session{
			NamespaceID:  connection.NamespaceID,
			Epoch:        connection.Epoch,
			Instance:     connection.Instance,
			InnerAddress: connection.InnerAddress,
			ConnectedAt:  connectedAt,
			LastSeen:     at,
		})
		change := Change{Key: key, WasOnline: wasOnline, Online: true, Reason: ReasonConnected, Epoch: connection.Epoch, At: at, Sessions: copySessions(state)}
		if existing == nil {
			change.NewNamespace = connection.NamespaceID
		}
		return []Change{change}
	case "disconnected":
		if existing == nil {
			state.close(connection.NamespaceID, connection.Epoch)
			view.markIdle(key, state, at)
			return nil
		}
		if existing.Epoch > connection.Epoch {
			return nil
		}
		wasOnline, online := view.remove(key, connection.NamespaceID, connection.Epoch, at)
		return []Change{{Key: key, WasOnline: wasOnline, Online: online, Reason: ReasonDisconnected, Epoch: connection.Epoch, At: at, Sessions: copySessions(state)}}
	}
	return nil
}

func (view *View) ApplyCensus(record kafka.CensusRecord) []Change {
	at, failure := kafka.ParseTime(record.Time)
	if failure != nil {
		at = time.Now()
	}
	view.mutex.Lock()
	defer view.mutex.Unlock()
	state := view.instance(record.Instance)
	switch record.Record {
	case "chunk":
		if record.Generation <= state.appliedGeneration && state.appliedGeneration != 0 {
			return nil
		}
		if record.Generation != state.pendingGeneration {
			if record.Generation < state.pendingGeneration {
				return nil
			}
			state.pendingGeneration = record.Generation
			state.chunks = map[int][]kafka.CensusSession{}
			if state.pendingHeader != nil && state.pendingHeader.Generation != record.Generation {
				state.pendingHeader = nil
			}
		}
		state.chunks[record.Index] = record.Sessions
		return view.applyIfComplete(record.Instance, state, at)
	case "header":
		var changes []Change
		startedAt, failure := kafka.ParseTime(record.StartedAt)
		if failure == nil {
			if !state.startedAt.IsZero() && startedAt.After(state.startedAt) {
				changes = append(changes, view.dropBefore(record.Instance, state, startedAt, at)...)
			}
			if startedAt.After(state.startedAt) {
				state.startedAt = startedAt
			}
		}
		if at.After(state.lastHeader) {
			state.lastHeader = at
		}
		state.heartbeatSeconds = record.HeartbeatSeconds
		state.timedOut = false
		if record.Generation <= state.appliedGeneration && state.appliedGeneration != 0 {
			return changes
		}
		if record.Generation != state.pendingGeneration {
			if record.Generation < state.pendingGeneration {
				return changes
			}
			state.pendingGeneration = record.Generation
			state.chunks = map[int][]kafka.CensusSession{}
		}
		header := record
		state.pendingHeader = &header
		changes = append(changes, view.applyIfComplete(record.Instance, state, at)...)
		if record.Full && record.ChunkCount == 0 && state.appliedGeneration == record.Generation && len(state.sessions) == 0 {
			delete(view.instances, record.Instance)
			view.settleSkip()
		}
		return changes
	}
	return nil
}

func (view *View) applyIfComplete(instance string, state *instanceState, at time.Time) []Change {
	header := state.pendingHeader
	if header == nil || header.Generation != state.pendingGeneration {
		return nil
	}
	for index := range header.ChunkCount {
		if _, present := state.chunks[index]; !present {
			return nil
		}
	}
	census := map[NodeKey]map[string]kafka.CensusSession{}
	for index := range header.ChunkCount {
		for _, session := range state.chunks[index] {
			key := NodeKey{DeviceID: session.DeviceID, InstallationID: session.InstallationID}
			if census[key] == nil {
				census[key] = map[string]kafka.CensusSession{}
			}
			census[key][session.NamespaceID] = session
		}
	}
	touched := map[NodeKey]int64{}
	wasOnline := map[NodeKey]bool{}
	remember := func(key NodeKey, epoch int64) {
		if _, seen := touched[key]; !seen {
			wasOnline[key] = view.nodes[key] != nil && len(view.nodes[key].sessions) > 0
		}
		touched[key] = max(touched[key], epoch)
	}
	for key, namespaces := range state.sessions {
		for namespace := range namespaces {
			if _, listed := census[key][namespace]; listed {
				continue
			}
			node := view.nodes[key]
			if node == nil || node.sessions[namespace] == nil || node.sessions[namespace].Epoch > header.SnapshotEpoch {
				continue
			}
			epoch := node.sessions[namespace].Epoch
			remember(key, epoch)
			view.remove(key, namespace, epoch, at)
		}
	}
	for key, namespaces := range census {
		for namespace, listed := range namespaces {
			node := view.nodes[key]
			if node != nil {
				if closedEpoch, closed := node.closed[namespace]; closed && closedEpoch >= listed.Epoch {
					continue
				}
				if existing := node.sessions[namespace]; existing != nil {
					if existing.Epoch > listed.Epoch {
						continue
					}
					if existing.Epoch == listed.Epoch && existing.Instance == instance {
						if lastSeen, failure := kafka.ParseTime(listed.LastSeen); failure == nil && lastSeen.After(existing.LastSeen) {
							existing.LastSeen = lastSeen
						}
						continue
					}
				}
			}
			connectedAt, _ := kafka.ParseTime(listed.ConnectedAt)
			lastSeen, _ := kafka.ParseTime(listed.LastSeen)
			remember(key, listed.Epoch)
			view.set(key, Session{NamespaceID: namespace, Epoch: listed.Epoch, Instance: instance, InnerAddress: header.InnerAddress, ConnectedAt: connectedAt, LastSeen: lastSeen})
		}
	}
	state.appliedGeneration = header.Generation
	state.snapshotEpoch = header.SnapshotEpoch
	if headerAt, failure := kafka.ParseTime(header.Time); failure == nil {
		state.appliedHeaderAt = headerAt
	} else {
		state.appliedHeaderAt = at
	}
	state.pendingHeader = nil
	state.chunks = map[int][]kafka.CensusSession{}
	state.needsFull = false
	view.settleSkip()
	return view.changesFor(touched, wasOnline, ReasonCensus, at)
}

func (view *View) settleSkip() {
	if !view.skipped {
		return
	}
	for _, other := range view.instances {
		if other.needsFull {
			return
		}
	}
	view.skipped = false
	view.caughtUp = true
}

func (view *View) changesFor(touched map[NodeKey]int64, wasOnline map[NodeKey]bool, reason string, at time.Time) []Change {
	changes := make([]Change, 0, len(touched))
	for key, epoch := range touched {
		node := view.nodes[key]
		online := node != nil && len(node.sessions) > 0
		changes = append(changes, Change{Key: key, WasOnline: wasOnline[key], Online: online, Reason: reason, Epoch: epoch, At: at, Sessions: copySessions(node)})
	}
	sort.Slice(changes, func(left, right int) bool { return changes[left].Key.String() < changes[right].Key.String() })
	return changes
}

func (view *View) removeAttributed(state *instanceState, keep func(*Session) bool, at time.Time) (map[NodeKey]int64, map[NodeKey]bool) {
	touched := map[NodeKey]int64{}
	wasOnline := map[NodeKey]bool{}
	for key, namespaces := range state.sessions {
		for namespace := range namespaces {
			node := view.nodes[key]
			if node == nil || node.sessions[namespace] == nil || keep(node.sessions[namespace]) {
				continue
			}
			epoch := node.sessions[namespace].Epoch
			touched[key] = max(touched[key], epoch)
			wasOnline[key] = true
			view.remove(key, namespace, epoch, at)
		}
	}
	return touched, wasOnline
}

func (view *View) dropBefore(instance string, state *instanceState, startedAt, at time.Time) []Change {
	touched, wasOnline := view.removeAttributed(state, func(session *Session) bool { return !session.ConnectedAt.Before(startedAt) }, at)
	return view.changesFor(touched, wasOnline, ReasonRestart, at)
}

func (state *instanceState) dead(now time.Time) bool {
	return !state.lastHeader.IsZero() && state.heartbeatSeconds > 0 && now.Sub(state.lastHeader) > 3*time.Duration(state.heartbeatSeconds)*time.Second
}

func (view *View) Expire(now time.Time, forget time.Duration) []Change {
	view.mutex.Lock()
	defer view.mutex.Unlock()
	var changes []Change
	for name, state := range view.instances {
		if !state.dead(now) {
			continue
		}
		if !state.timedOut {
			state.timedOut = true
			state.needsFull = false
			touched, wasOnline := view.removeAttributed(state, func(session *Session) bool { return session.Epoch > state.snapshotEpoch }, now)
			changes = append(changes, view.changesFor(touched, wasOnline, ReasonCensusTimeout, now)...)
		}
		if now.Sub(state.lastHeader) > 3*time.Duration(state.heartbeatSeconds)*time.Second+forget {
			touched, wasOnline := view.removeAttributed(state, func(*Session) bool { return false }, now)
			changes = append(changes, view.changesFor(touched, wasOnline, ReasonCensusTimeout, now)...)
			delete(view.instances, name)
		}
	}
	view.settleSkip()
	return changes
}

func (view *View) Evict(now time.Time, horizon time.Duration) int {
	view.mutex.Lock()
	defer view.mutex.Unlock()
	evicted, index := 0, 0
	for ; index < len(view.idle) && now.Sub(view.idle[index].since) > horizon; index++ {
		entry := view.idle[index]
		if state := view.nodes[entry.key]; state != nil && len(state.sessions) == 0 && state.idleSince.Equal(entry.since) {
			delete(view.nodes, entry.key)
			evicted++
		}
	}
	view.idle = append([]idleNode(nil), view.idle[index:]...)
	return evicted
}

func (view *View) MarkSkipped() {
	view.mutex.Lock()
	defer view.mutex.Unlock()
	view.skipped = true
	view.caughtUp = false
	for _, state := range view.instances {
		state.needsFull = !state.timedOut
	}
	view.settleSkip()
}

func (view *View) TakeCaughtUp() bool {
	view.mutex.Lock()
	defer view.mutex.Unlock()
	caughtUp := view.caughtUp
	view.caughtUp = false
	return caughtUp
}

func (view *View) Degraded() bool {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	if view.skipped {
		return true
	}
	for _, state := range view.instances {
		if state.timedOut {
			return true
		}
	}
	return false
}

func (view *View) OldestHeader(now time.Time) time.Time {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	var oldest time.Time
	for _, state := range view.instances {
		if state.appliedGeneration == 0 || state.appliedHeaderAt.IsZero() || state.dead(now) {
			continue
		}
		if oldest.IsZero() || state.appliedHeaderAt.Before(oldest) {
			oldest = state.appliedHeaderAt
		}
	}
	return oldest
}

func (view *View) Target(key NodeKey) (Session, bool) {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	state := view.nodes[key]
	if state == nil || len(state.sessions) == 0 {
		return Session{}, false
	}
	var best *Session
	for _, session := range state.sessions {
		if best == nil || session.Epoch > best.Epoch {
			best = session
		}
	}
	return *best, true
}

func (view *View) Online(key NodeKey) bool {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	state := view.nodes[key]
	return state != nil && len(state.sessions) > 0
}

func (view *View) Sessions(key NodeKey) []Session {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	return copySessions(view.nodes[key])
}

func (view *View) OnlineCount() int {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	return view.onlineSize
}

func (view *View) LastSeen(within func(NodeKey) bool) ([]NodeKey, []time.Time) {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	var keys []NodeKey
	var times []time.Time
	for key, state := range view.nodes {
		if len(state.sessions) == 0 || !within(key) {
			continue
		}
		var latest time.Time
		for _, session := range state.sessions {
			if session.LastSeen.After(latest) {
				latest = session.LastSeen
			}
		}
		keys = append(keys, key)
		times = append(times, latest)
	}
	return keys, times
}

func (view *View) ForEachOnline(visit func(NodeKey, []Session)) {
	view.mutex.RLock()
	keys := make([]NodeKey, 0, view.onlineSize)
	for key, state := range view.nodes {
		if len(state.sessions) > 0 {
			keys = append(keys, key)
		}
	}
	view.mutex.RUnlock()
	for _, key := range keys {
		if sessions := view.Sessions(key); len(sessions) > 0 {
			visit(key, sessions)
		}
	}
}

type InstanceStatus struct {
	Instance          string    `json:"instance"`
	StartedAt         time.Time `json:"started_at"`
	LastHeader        time.Time `json:"last_header"`
	AppliedGeneration int64     `json:"applied_generation"`
	TimedOut          bool      `json:"timed_out"`
	Sessions          int       `json:"sessions"`
}

func (view *View) Instances() []InstanceStatus {
	view.mutex.RLock()
	defer view.mutex.RUnlock()
	statuses := make([]InstanceStatus, 0, len(view.instances))
	for name, state := range view.instances {
		count := 0
		for _, namespaces := range state.sessions {
			count += len(namespaces)
		}
		statuses = append(statuses, InstanceStatus{Instance: name, StartedAt: state.startedAt, LastHeader: state.lastHeader, AppliedGeneration: state.appliedGeneration, TimedOut: state.timedOut, Sessions: count})
	}
	sort.Slice(statuses, func(left, right int) bool { return statuses[left].Instance < statuses[right].Instance })
	return statuses
}
