package engine

import (
	"context"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
)

var evaluationsDropped = promauto.NewCounter(prometheus.CounterOpts{
	Name: "twilight_evaluation_dropped_total",
	Help: "Connect hints dropped because the evaluation set was full.",
})

var evaluationsQueued = promauto.NewGauge(prometheus.GaugeOpts{Name: "twilight_evaluation_queue", Help: "Nodes waiting for evaluation."})

var transitionsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_row_transitions_total",
	Help: "campaign_nodes transitions applied, by the state entered.",
}, []string{"state"})

type keyQueue struct {
	mutex    sync.Mutex
	set      map[NodeKey]struct{}
	order    []NodeKey
	capacity int
	notify   chan struct{}
}

func newKeyQueue(capacity int) *keyQueue {
	return &keyQueue{set: map[NodeKey]struct{}{}, capacity: capacity, notify: make(chan struct{}, 1)}
}

func (queue *keyQueue) offer(key NodeKey) bool {
	queue.mutex.Lock()
	defer queue.mutex.Unlock()
	if _, queued := queue.set[key]; queued {
		return true
	}
	if len(queue.set) >= queue.capacity {
		return false
	}
	queue.set[key] = struct{}{}
	queue.order = append(queue.order, key)
	select {
	case queue.notify <- struct{}{}:
	default:
	}
	return true
}

func (queue *keyQueue) take(maximum int) []NodeKey {
	queue.mutex.Lock()
	defer queue.mutex.Unlock()
	count := min(maximum, len(queue.order))
	taken := append([]NodeKey(nil), queue.order[:count]...)
	queue.order = queue.order[count:]
	if len(queue.order) == 0 {
		queue.order = nil
	}
	for _, key := range taken {
		delete(queue.set, key)
	}
	return taken
}

func (queue *keyQueue) size() int {
	queue.mutex.Lock()
	defer queue.mutex.Unlock()
	return len(queue.set)
}

type evaluationQueue = keyQueue

func newEvaluationQueue(capacity int) *evaluationQueue {
	return newKeyQueue(capacity)
}

func (engine *Engine) hint(key NodeKey) {
	if !engine.leadership.Leading() {
		return
	}
	if !engine.evaluations.offer(key) {
		evaluationsDropped.Inc()
	}
}

func (engine *Engine) runEvaluations(operation context.Context, term int64) {
	for {
		select {
		case <-operation.Done():
			return
		case <-engine.evaluations.notify:
		case <-time.After(time.Second):
		}
		for {
			batch := engine.evaluations.take(1000)
			evaluationsQueued.Set(float64(engine.evaluations.size()))
			if len(batch) == 0 {
				break
			}
			if _, failure := engine.evaluateBatch(operation, term, batch); failure != nil && operation.Err() == nil {
				engine.Logger.Warn("an evaluation batch failed; its nodes return with the next hint or sweep", "nodes", len(batch), "error", failure)
			}
		}
	}
}

func (engine *Engine) evaluateBatch(operation context.Context, term int64, keys []NodeKey) (int, error) {
	if len(keys) == 0 {
		return 0, nil
	}
	nodes, failure := engine.Inventory.GetMany(operation, keys)
	if failure != nil {
		return 0, failure
	}
	now := time.Now()
	missed := 0
	var evaluate []NodeKey
	for _, key := range keys {
		node := nodes[key]
		if node == nil {
			continue
		}
		if session, online := engine.view.Target(key); online && engine.needsFacts(node, session, now) {
			if engine.facts.offer(key) {
				missed++
				continue
			}
		}
		evaluate = append(evaluate, key)
	}
	targets := engine.registry.targets()
	if len(evaluate) == 0 || len(targets) == 0 {
		return missed, nil
	}
	devices := make([]string, len(evaluate))
	installations := make([]string, len(evaluate))
	for index, key := range evaluate {
		devices[index], installations[index] = key.DeviceID, key.InstallationID
	}
	identifiers := make([]uuid.UUID, len(targets))
	for index, target := range targets {
		identifiers[index] = target.Campaign.ID
	}
	loaded, failure := engine.Campaigns.RowsForNodes(operation, identifiers, devices, installations)
	if failure != nil {
		return missed, failure
	}
	rows := map[NodeKey]map[uuid.UUID]campaign.Row{}
	for _, row := range loaded {
		key := NodeKey{DeviceID: row.DeviceID, InstallationID: row.InstallationID}
		if rows[key] == nil {
			rows[key] = map[uuid.UUID]campaign.Row{}
		}
		rows[key][row.CampaignID] = row
	}
	for _, key := range evaluate {
		var session *Session
		if found, online := engine.view.Target(key); online {
			session = &found
		}
		failed := false
		for _, decision := range Evaluate(targets, nodes[key], rows[key], session, now, engine.randomFloat()) {
			failed = engine.apply(operation, term, decision) != nil || failed
		}
		if failed {
			missed++
		}
	}
	return missed, nil
}

func (engine *Engine) logTransition(previous *campaign.Row, next campaign.Row, reason string) {
	from := ""
	if previous != nil {
		from = string(previous.State)
	}
	transitionsTotal.WithLabelValues(string(next.State)).Inc()
	engine.Logger.Info("campaign node transition", "campaign_id", next.CampaignID, "device_id", next.DeviceID, "installation_id", next.InstallationID,
		"pid", next.Pid.String(), "attempt", next.Attempt, "from", from, "to", next.State, "reason", reason, "last_status", next.LastStatus)
}

func (engine *Engine) write(operation context.Context, term int64, previous *campaign.Row, next campaign.Row, reason string) (campaign.Row, bool, error) {
	var written campaign.Row
	var applied bool
	var failure error
	if previous == nil {
		written, applied, failure = engine.Campaigns.InsertRow(operation, term, next)
	} else {
		written, applied, failure = engine.Campaigns.UpdateRow(operation, term, *previous, next)
	}
	if failure != nil {
		if operation.Err() == nil {
			engine.Logger.Warn("a campaign node write failed", "campaign_id", next.CampaignID, "device_id", next.DeviceID, "installation_id", next.InstallationID, "error", failure)
		}
		return next, false, failure
	}
	if !applied {
		return next, false, nil
	}
	engine.registry.count(next.CampaignID, previous, written)
	engine.logTransition(previous, written, reason)
	return written, true, nil
}

func (engine *Engine) apply(operation context.Context, term int64, decision Decision) error {
	row := decision.Next
	if decision.Write {
		written, applied, failure := engine.write(operation, term, decision.Previous, decision.Next, "evaluate")
		if !applied {
			return failure
		}
		row = written
		if decision.Conflict {
			engine.raiseConflict(operation, decision.Target.Campaign, row)
		}
	}
	if decision.Dispatch {
		engine.dispatcher.offer(decision.Target.Campaign.ID, NodeKey{DeviceID: row.DeviceID, InstallationID: row.InstallationID})
	}
	return nil
}

func (engine *Engine) raiseConflict(operation context.Context, conflicted *campaign.Campaign, row campaign.Row) {
	existing := engine.registry.get(conflicted.ID)
	if existing == nil {
		return
	}
	engine.registry.mutex.Lock()
	first := !existing.conflictRaised
	existing.conflictRaised = true
	engine.registry.mutex.Unlock()
	if !first {
		return
	}
	detail := map[string]any{"campaign_id": conflicted.ID.String(), "device_id": row.DeviceID, "installation_id": row.InstallationID,
		"message": "an earlier campaign of the same kind owns some of this campaign's nodes; their rows are in conflict"}
	if failure := engine.Campaigns.AddEvent(operation, conflicted.ID, "conflict", "twilight", detail); failure != nil {
		engine.Logger.Warn("the conflict event was not written", "campaign_id", conflicted.ID, "error", failure)
	}
	if _, failure := engine.Alerts.Raise(operation, alerts.Raised{Severity: alerts.Medium, Kind: alerts.KindCampaignConflict, Fingerprint: "campaign_conflict:" + conflicted.ID.String(), Detail: detail}); failure != nil {
		engine.Logger.Warn("the conflict alert was not raised", "campaign_id", conflicted.ID, "error", failure)
	}
}
