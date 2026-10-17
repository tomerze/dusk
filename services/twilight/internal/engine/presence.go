package engine

import (
	"context"
	"hash/fnv"
	"slices"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/inventory"
)

var (
	presenceWrites = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_presence_writes_total",
		Help: "node_presence rows written.",
	})
	silentEventsDropped = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_silent_events_dropped_total",
		Help: "Connects and disconnects not recorded for the silent-rate gate because too many were waiting to be written.",
	})
)

type presenceUpdate struct {
	online         bool
	sessions       []Session
	connectedAt    *time.Time
	disconnectedAt *time.Time
	lastSeen       *time.Time
	sightedAt      time.Time
	becameOnline   bool
	connected      bool
}

type silentEvent struct {
	key   NodeKey
	at    time.Time
	epoch int64
}

type presenceWriter struct {
	engine     *Engine
	mutex      sync.Mutex
	pending    map[NodeKey]*presenceUpdate
	offline    []silentEvent
	online     []silentEvent
	reconciles map[int32]bool
	wake       chan struct{}
}

func newPresenceWriter(engine *Engine) *presenceWriter {
	return &presenceWriter{engine: engine, pending: map[NodeKey]*presenceUpdate{}, reconciles: map[int32]bool{}, wake: make(chan struct{}, 1)}
}

func (writer *presenceWriter) requestReconcile(partition int32) {
	writer.mutex.Lock()
	writer.reconciles[partition] = true
	writer.mutex.Unlock()
	select {
	case writer.wake <- struct{}{}:
	default:
	}
}

const maximumSilentEvents = 200000

func (writer *presenceWriter) add(changes []Change) {
	writer.mutex.Lock()
	defer writer.mutex.Unlock()
	for _, change := range changes {
		update := writer.pending[change.Key]
		if update == nil {
			update = &presenceUpdate{}
			writer.pending[change.Key] = update
		}
		update.online = change.Online
		update.sessions = change.Sessions
		at := change.At
		if change.Online && !change.WasOnline {
			connectedAt := at
			if len(change.Sessions) > 0 && !change.Sessions[0].ConnectedAt.IsZero() {
				connectedAt = change.Sessions[0].ConnectedAt
			}
			update.connectedAt = &connectedAt
			update.becameOnline = true
			update.sightedAt = at
			writer.online = appendSilent(writer.online, silentEvent{key: change.Key, at: at, epoch: change.Epoch})
		}
		if change.Online && change.NewNamespace != "" {
			update.becameOnline = true
			update.sightedAt = at
		}
		if change.Online && change.Reason == ReasonConnected {
			update.connected = true
		}
		if !change.Online && change.WasOnline {
			update.disconnectedAt = &at
			writer.offline = appendSilent(writer.offline, silentEvent{key: change.Key, at: at, epoch: change.Epoch})
		}
		for _, session := range change.Sessions {
			lastSeen := session.LastSeen
			if update.lastSeen == nil || lastSeen.After(*update.lastSeen) {
				update.lastSeen = &lastSeen
			}
		}
	}
	select {
	case writer.wake <- struct{}{}:
	default:
	}
}

func appendSilent(events []silentEvent, event silentEvent) []silentEvent {
	if len(events) >= maximumSilentEvents {
		silentEventsDropped.Inc()
		return events
	}
	return append(events, event)
}

func keepNewest(events []silentEvent) []silentEvent {
	if excess := len(events) - maximumSilentEvents; excess > 0 {
		silentEventsDropped.Add(float64(excess))
		return events[excess:]
	}
	return events
}

func presenceSessions(sessions []Session) []inventory.PresenceSession {
	converted := make([]inventory.PresenceSession, len(sessions))
	for index, session := range sessions {
		converted[index] = inventory.PresenceSession{NamespaceID: session.NamespaceID, Epoch: session.Epoch, Instance: session.Instance, InnerAddress: session.InnerAddress, ConnectedAt: session.ConnectedAt}
	}
	return converted
}

func (writer *presenceWriter) run(operation context.Context) {
	interval := time.Duration(writer.engine.Config.Engine.PresenceFlushMillis) * time.Millisecond
	for sleep(operation, interval) {
		writer.reconcileRequested(operation)
		if failure := writer.flush(operation); failure != nil && operation.Err() == nil {
			writer.engine.Logger.Warn("presence flush failed; it will be retried", "error", failure)
		}
	}
}

func (writer *presenceWriter) reconcileRequested(operation context.Context) {
	writer.mutex.Lock()
	partitions := make([]int32, 0, len(writer.reconciles))
	for partition := range writer.reconciles {
		partitions = append(partitions, partition)
	}
	writer.reconciles = map[int32]bool{}
	writer.mutex.Unlock()
	partitions = slices.DeleteFunc(partitions, func(partition int32) bool {
		_, owned := writer.engine.ownership.leaseOf(partition)
		return !owned
	})
	if len(partitions) == 0 {
		return
	}
	if failure := writer.reconcile(operation, partitions); failure != nil {
		if operation.Err() == nil {
			writer.engine.Logger.Warn("diffing the online view against node_presence for slices taken over failed; they are diffed again on the next flush", "partitions", partitions, "error", failure)
		}
		writer.mutex.Lock()
		for _, partition := range partitions {
			writer.reconciles[partition] = true
		}
		writer.mutex.Unlock()
	}
}

func (writer *presenceWriter) reconcile(operation context.Context, partitions []int32) error {
	view := writer.engine.view
	seen := map[NodeKey]bool{}
	var changes []Change
	visit := func(stored inventory.Presence) error {
		seen[stored.Key] = true
		sessions := view.Sessions(stored.Key)
		online := len(sessions) > 0
		if online == stored.Online && len(sessions) == len(stored.Sessions) {
			same := true
			for index := range sessions {
				same = same && sessions[index].NamespaceID == stored.Sessions[index].NamespaceID && sessions[index].Epoch == stored.Sessions[index].Epoch
			}
			if same {
				return nil
			}
		}
		change := Change{Key: stored.Key, WasOnline: stored.Online, Online: online, Reason: ReasonCensus, At: time.Now(), Sessions: sessions}
		for _, session := range stored.Sessions {
			change.Epoch = max(change.Epoch, session.Epoch)
		}
		for _, session := range sessions {
			change.Epoch = max(change.Epoch, session.Epoch)
		}
		changes = append(changes, change)
		return nil
	}
	for _, partition := range partitions {
		if failure := writer.engine.Inventory.ScanPresence(operation, partition, visit); failure != nil {
			return failure
		}
	}
	view.ForEachOnline(func(key NodeKey, sessions []Session) {
		if seen[key] {
			return
		}
		if keyPartition, known := writer.engine.ownership.partitionOf(key); known && slices.Contains(partitions, keyPartition) {
			changes = append(changes, Change{Key: key, Online: true, Reason: ReasonCensus, Epoch: sessions[0].Epoch, At: time.Now(), Sessions: sessions, NewNamespace: sessions[0].NamespaceID})
		}
	})
	writer.engine.Logger.Info("presence differences found in slices taken over", "partitions", partitions, "count", len(changes))
	writer.add(changes)
	return nil
}

func (writer *presenceWriter) flush(operation context.Context) error {
	writer.mutex.Lock()
	pending := writer.pending
	offline, online := writer.offline, writer.online
	writer.pending = map[NodeKey]*presenceUpdate{}
	writer.offline, writer.online = nil, nil
	writer.mutex.Unlock()
	ownership := writer.engine.ownership
	for key := range pending {
		if !ownership.owns(key) {
			delete(pending, key)
		}
	}
	offline = slices.DeleteFunc(offline, func(event silentEvent) bool { return !ownership.owns(event.key) })
	online = slices.DeleteFunc(online, func(event silentEvent) bool { return !ownership.owns(event.key) })
	if len(pending) == 0 && len(offline) == 0 && len(online) == 0 {
		return nil
	}
	var sightings []inventory.Sighting
	rows := make([]inventory.Presence, 0, len(pending))
	var arrived []NodeKey
	for key, update := range pending {
		if update.becameOnline {
			sightings = append(sightings, inventory.Sighting{Key: key, At: update.sightedAt})
		}
		if update.becameOnline || update.connected {
			arrived = append(arrived, key)
		}
		partition, _ := ownership.partitionOf(key)
		rows = append(rows, inventory.Presence{Key: key, Partition: partition, Online: update.online, LastSeenAt: update.lastSeen, ConnectedAt: update.connectedAt,
			DisconnectedAt: update.disconnectedAt, Sessions: presenceSessions(update.sessions)})
	}
	requeueSilent := func() {
		writer.offline = keepNewest(append(offline, writer.offline...))
		writer.online = keepNewest(append(online, writer.online...))
	}
	requeue := func(failure error) error {
		writer.mutex.Lock()
		for key, update := range pending {
			if _, newer := writer.pending[key]; !newer {
				writer.pending[key] = update
			}
		}
		requeueSilent()
		writer.mutex.Unlock()
		return failure
	}
	if len(sightings) > 0 {
		activated, failure := writer.engine.Inventory.RecordConnections(operation, sightings)
		if failure != nil {
			return requeue(failure)
		}
		for _, key := range activated {
			writer.engine.Logger.Info("node active after its first connection", "device_id", key.DeviceID, "installation_id", key.InstallationID)
		}
	}
	held := map[int32]int64{}
	for _, lease := range ownership.leases() {
		held[lease.Partition] = lease.Epoch
	}
	if failure := writer.engine.Inventory.WritePresence(operation, rows, held); failure != nil {
		return requeue(failure)
	}
	presenceWrites.Add(float64(len(rows)))
	campaigns := writer.engine.registry.activeIDs()
	if len(campaigns) > 0 {
		if failure := writer.silent(operation, campaigns, offline, online); failure != nil {
			writer.engine.Logger.Warn("silent-rate presence updates failed; they are retried with the next flush", "error", failure)
			writer.mutex.Lock()
			requeueSilent()
			writer.mutex.Unlock()
		}
	}
	for _, key := range arrived {
		writer.engine.hint(key)
	}
	return nil
}

func columnsOf(events []silentEvent) ([]string, []string, []time.Time, []int64) {
	devices := make([]string, len(events))
	installations := make([]string, len(events))
	times := make([]time.Time, len(events))
	epochs := make([]int64, len(events))
	for index, event := range events {
		devices[index], installations[index], times[index], epochs[index] = event.key.DeviceID, event.key.InstallationID, event.at, event.epoch
	}
	return devices, installations, times, epochs
}

func (writer *presenceWriter) silent(operation context.Context, campaigns []uuid.UUID, offline, online []silentEvent) error {
	leases := writer.engine.ownership.leases()
	for start := 0; start < len(offline); start += 5000 {
		devices, installations, times, epochs := columnsOf(offline[start:min(len(offline), start+5000)])
		if failure := writer.engine.Campaigns.RecordGone(operation, leases, campaigns, devices, installations, times, epochs); failure != nil {
			return failure
		}
	}
	for start := 0; start < len(online); start += 5000 {
		devices, installations, times, _ := columnsOf(online[start:min(len(online), start+5000)])
		if failure := writer.engine.Campaigns.RecordBack(operation, leases, campaigns, devices, installations, times); failure != nil {
			return failure
		}
	}
	return nil
}

func bucketOf(key NodeKey, buckets uint32) uint32 {
	hash := fnv.New32a()
	hash.Write([]byte(key.String()))
	return hash.Sum32() % buckets
}

func (writer *presenceWriter) runLastSeen(operation context.Context) {
	const buckets = 30
	interval := time.Duration(writer.engine.Config.Engine.LastSeenBucketSeconds) * time.Second
	bucket := uint32(0)
	for sleep(operation, interval) {
		current := bucket
		keys, times := writer.engine.view.LastSeen(func(key NodeKey) bool { return bucketOf(key, buckets) == current && writer.engine.ownership.owns(key) })
		if len(keys) > 0 {
			if failure := writer.engine.Inventory.UpdateLastSeen(operation, keys, times); failure != nil && operation.Err() == nil {
				writer.engine.Logger.Warn("last_seen_at sweep failed", "bucket", bucket, "error", failure)
			}
		}
		bucket = (bucket + 1) % buckets
	}
}
