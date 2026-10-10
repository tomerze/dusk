package engine

import (
	"context"
	"hash/fnv"
	"math/rand/v2"
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
	engine  *Engine
	mutex   sync.Mutex
	pending map[NodeKey]*presenceUpdate
	offline []silentEvent
	online  []silentEvent
	wake    chan struct{}
}

func newPresenceWriter(engine *Engine) *presenceWriter {
	return &presenceWriter{engine: engine, pending: map[NodeKey]*presenceUpdate{}, wake: make(chan struct{}, 1)}
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

func (writer *presenceWriter) run(operation context.Context, term int64) {
	backoff := time.Second
	for {
		failure := writer.reconcile(operation)
		if failure == nil || operation.Err() != nil {
			break
		}
		writer.engine.Logger.Error("diffing the online view against node_presence failed; retrying", "term", term, "error", failure)
		if !sleep(operation, time.Duration(rand.Int64N(int64(backoff)))+1) {
			return
		}
		backoff = min(2*backoff, time.Minute)
	}
	interval := time.Duration(writer.engine.Config.Engine.PresenceFlushMillis) * time.Millisecond
	for sleep(operation, interval) {
		if failure := writer.flush(operation, term); failure != nil && operation.Err() == nil {
			writer.engine.Logger.Warn("presence flush failed; it will be retried", "error", failure)
		}
	}
}

func (writer *presenceWriter) reconcile(operation context.Context) error {
	view := writer.engine.view
	seen := map[NodeKey]bool{}
	var changes []Change
	failure := writer.engine.Inventory.ScanPresence(operation, func(stored inventory.Presence) error {
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
	})
	if failure != nil {
		return failure
	}
	view.ForEachOnline(func(key NodeKey, sessions []Session) {
		if !seen[key] {
			changes = append(changes, Change{Key: key, Online: true, Reason: ReasonCensus, Epoch: sessions[0].Epoch, At: time.Now(), Sessions: sessions, NewNamespace: sessions[0].NamespaceID})
		}
	})
	writer.engine.Logger.Info("presence differences found at election", "count", len(changes))
	writer.add(changes)
	return nil
}

func (writer *presenceWriter) flush(operation context.Context, term int64) error {
	writer.mutex.Lock()
	pending := writer.pending
	offline, online := writer.offline, writer.online
	writer.pending = map[NodeKey]*presenceUpdate{}
	writer.offline, writer.online = nil, nil
	writer.mutex.Unlock()
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
		rows = append(rows, inventory.Presence{Key: key, Online: update.online, LastSeenAt: update.lastSeen, ConnectedAt: update.connectedAt,
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
	if failure := writer.engine.Inventory.WritePresence(operation, rows); failure != nil {
		return requeue(failure)
	}
	presenceWrites.Add(float64(len(rows)))
	campaigns := writer.engine.registry.activeIDs()
	if len(campaigns) > 0 {
		if failure := writer.silent(operation, term, campaigns, offline, online); failure != nil {
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

func (writer *presenceWriter) silent(operation context.Context, term int64, campaigns []uuid.UUID, offline, online []silentEvent) error {
	pool := writer.engine.Pool
	for start := 0; start < len(offline); start += 5000 {
		devices, installations, times, epochs := columnsOf(offline[start:min(len(offline), start+5000)])
		if _, failure := pool.Exec(operation, `update campaign_nodes c set event_at = gone.at, revision = c.revision + 1, updated_at = now()
			from unnest($1::text[], $2::text[], $3::timestamptz[], $4::bigint[]) as gone(device_id, installation_id, at, epoch)
			where c.campaign_id = any($5) and c.device_id = gone.device_id and c.installation_id = gone.installation_id
			and c.state = 'succeeded' and c.event_at is null and coalesce(c.epoch, 0) <= gone.epoch and c.finished_at <= gone.at
			and exists (select 1 from leadership where id = 1 and term = $6)`, devices, installations, times, epochs, campaigns, term); failure != nil {
			return failure
		}
	}
	for start := 0; start < len(online); start += 5000 {
		devices, installations, times, _ := columnsOf(online[start:min(len(online), start+5000)])
		if _, failure := pool.Exec(operation, `update campaign_nodes c set back_at = back.at,
			silent = extract(epoch from back.at - c.event_at) > coalesce((k.policy->'gates'->>'silent_window_seconds')::double precision, 0),
			revision = c.revision + 1, updated_at = now()
			from unnest($1::text[], $2::text[], $3::timestamptz[]) as back(device_id, installation_id, at), campaigns k
			where k.id = c.campaign_id and c.campaign_id = any($4) and c.device_id = back.device_id and c.installation_id = back.installation_id
			and c.state = 'succeeded' and c.event_at is not null and c.back_at is null and back.at >= c.event_at
			and exists (select 1 from leadership where id = 1 and term = $5)`, devices, installations, times, campaigns, term); failure != nil {
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

func (writer *presenceWriter) runLastSeen(operation context.Context, term int64) {
	const buckets = 30
	interval := time.Duration(writer.engine.Config.Engine.LastSeenBucketSeconds) * time.Second
	bucket := uint32(0)
	for sleep(operation, interval) {
		current := bucket
		keys, times := writer.engine.view.LastSeen(func(key NodeKey) bool { return bucketOf(key, buckets) == current })
		if len(keys) > 0 {
			if failure := writer.engine.Inventory.UpdateLastSeen(operation, keys, times); failure != nil && operation.Err() == nil {
				writer.engine.Logger.Warn("last_seen_at sweep failed", "bucket", bucket, "error", failure)
			}
		}
		bucket = (bucket + 1) % buckets
	}
}
