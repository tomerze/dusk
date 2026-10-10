package engine

import (
	"context"
	"sort"
	"sync"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/selector"
)

type runtime struct {
	campaign       *campaign.Campaign
	selector       *selector.Selector
	bucket         *TokenBucket
	counters       map[int]map[campaign.NodeState]int64
	dirty          bool
	lastHold       string
	conflictRaised bool
}

func (current *runtime) target() Target {
	return Target{Campaign: current.campaign, Selector: current.selector}
}

type registry struct {
	engine   *Engine
	mutex    sync.Mutex
	runtimes map[uuid.UUID]*runtime
	term     int64
}

func newRegistry(engine *Engine) *registry {
	return &registry{engine: engine, runtimes: map[uuid.UUID]*runtime{}}
}

func (current *registry) reset() {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	current.runtimes = map[uuid.UUID]*runtime{}
	current.term = 0
}

func (current *registry) load(operation context.Context, term int64) error {
	current.mutex.Lock()
	current.term = term
	current.runtimes = map[uuid.UUID]*runtime{}
	current.mutex.Unlock()
	return current.refresh(operation)
}

func (current *registry) countersFor(operation context.Context, identifier uuid.UUID) (map[int]map[campaign.NodeState]int64, error) {
	counted, failure := current.engine.Campaigns.CountRows(operation, identifier)
	if failure != nil {
		return nil, failure
	}
	counters := map[int]map[campaign.NodeState]int64{}
	for _, counter := range counted {
		if counters[counter.Phase] == nil {
			counters[counter.Phase] = map[campaign.NodeState]int64{}
		}
		counters[counter.Phase][counter.State] = counter.Count
	}
	return counters, nil
}

func (current *registry) refresh(operation context.Context) error {
	active, failure := current.engine.Campaigns.Active(operation)
	if failure != nil {
		return failure
	}
	now := time.Now()
	seen := map[uuid.UUID]bool{}
	var sweeps []uuid.UUID
	for _, loaded := range active {
		seen[loaded.ID] = true
		current.mutex.Lock()
		existing := current.runtimes[loaded.ID]
		current.mutex.Unlock()
		if existing == nil {
			parsed, failure := selector.Parse(loaded.Selector)
			if failure != nil {
				current.engine.Logger.Error("a running campaign's selector no longer parses; it is skipped", "campaign_id", loaded.ID, "error", failure)
				continue
			}
			counters, failure := current.countersFor(operation, loaded.ID)
			if failure != nil {
				return failure
			}
			created := &runtime{campaign: loaded, selector: parsed, bucket: NewTokenBucket(loaded.Policy.Rate.PerSecond, loaded.Policy.Rate.Burst, now), counters: counters, dirty: true}
			current.mutex.Lock()
			current.runtimes[loaded.ID] = created
			current.mutex.Unlock()
			current.engine.Logger.Info("campaign tracked by the leader", "campaign_id", loaded.ID, "status", loaded.Status, "phase", loaded.CurrentPhase)
			if loaded.Status == campaign.StatusRunning {
				sweeps = append(sweeps, loaded.ID)
			}
			continue
		}
		current.mutex.Lock()
		previous := existing.campaign
		existing.campaign = loaded
		current.mutex.Unlock()
		resumed := previous.Status != campaign.StatusRunning && loaded.Status == campaign.StatusRunning
		advanced := loaded.CurrentPhase != previous.CurrentPhase
		if resumed || (advanced && loaded.Status == campaign.StatusRunning) {
			sweeps = append(sweeps, loaded.ID)
		}
	}
	current.mutex.Lock()
	var gone []*runtime
	for identifier, existing := range current.runtimes {
		if !seen[identifier] {
			gone = append(gone, existing)
			delete(current.runtimes, identifier)
		}
	}
	current.mutex.Unlock()
	for _, finished := range gone {
		counters, failure := current.engine.Campaigns.CountRows(operation, finished.campaign.ID)
		if failure == nil {
			failure = current.engine.Campaigns.WriteCounters(operation, finished.campaign.ID, counters)
		}
		if failure != nil {
			current.engine.Logger.Warn("final counters of a finished campaign were not written", "campaign_id", finished.campaign.ID, "error", failure)
		}
		current.engine.Logger.Info("campaign no longer tracked by the leader", "campaign_id", finished.campaign.ID)
	}
	for _, identifier := range sweeps {
		current.engine.requestSweep(sweepRequest{campaign: identifier})
	}
	return nil
}

func (current *registry) run(operation context.Context, term int64) {
	for sleep(operation, 2*time.Second) {
		if failure := current.refresh(operation); failure != nil && operation.Err() == nil {
			current.engine.Logger.Warn("refreshing the campaigns failed", "term", term, "error", failure)
		}
	}
}

func (current *registry) targets() []Target {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	runtimes := make([]*runtime, 0, len(current.runtimes))
	for _, existing := range current.runtimes {
		runtimes = append(runtimes, existing)
	}
	sort.Slice(runtimes, func(left, right int) bool {
		leftStart, rightStart := runtimes[left].campaign.StartedAt, runtimes[right].campaign.StartedAt
		if leftStart != nil && rightStart != nil && !leftStart.Equal(*rightStart) {
			return leftStart.Before(*rightStart)
		}
		return runtimes[left].campaign.ID.String() < runtimes[right].campaign.ID.String()
	})
	targets := make([]Target, len(runtimes))
	for index, existing := range runtimes {
		targets[index] = existing.target()
	}
	return targets
}

func (current *registry) get(identifier uuid.UUID) *runtime {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	return current.runtimes[identifier]
}

func (current *registry) campaignOf(identifier uuid.UUID) *campaign.Campaign {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if existing := current.runtimes[identifier]; existing != nil {
		return existing.campaign
	}
	return nil
}

func (current *registry) activeIDs() []uuid.UUID {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	identifiers := make([]uuid.UUID, 0, len(current.runtimes))
	for identifier := range current.runtimes {
		identifiers = append(identifiers, identifier)
	}
	return identifiers
}

func (current *registry) setStatus(identifier uuid.UUID, updated *campaign.Campaign) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if existing := current.runtimes[identifier]; existing != nil && updated != nil {
		existing.campaign = updated
	}
}

func (current *registry) count(identifier uuid.UUID, previous *campaign.Row, next campaign.Row) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	existing := current.runtimes[identifier]
	if existing == nil {
		return
	}
	if previous != nil {
		if existing.counters[previous.Phase] != nil {
			existing.counters[previous.Phase][previous.State]--
		}
	}
	if existing.counters[next.Phase] == nil {
		existing.counters[next.Phase] = map[campaign.NodeState]int64{}
	}
	existing.counters[next.Phase][next.State]++
	existing.dirty = true
}

func (current *registry) takeToken(identifier uuid.UUID, now time.Time) bool {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	existing := current.runtimes[identifier]
	if existing == nil || existing.campaign.Status != campaign.StatusRunning {
		return false
	}
	return existing.bucket.Take(now)
}

func (current *registry) tokenAvailable(identifier uuid.UUID, now time.Time) (bool, time.Duration) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	existing := current.runtimes[identifier]
	if existing == nil || existing.campaign.Status != campaign.StatusRunning {
		return false, time.Hour
	}
	return existing.bucket.Available(now), existing.bucket.Wait(now)
}

func (current *registry) returnTokens(identifier uuid.UUID, count int) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if existing := current.runtimes[identifier]; existing != nil {
		existing.bucket.Return(count)
	}
}

func (current *registry) runFlush(operation context.Context, term int64) {
	interval := time.Duration(current.engine.Config.Engine.CountersFlushSeconds) * time.Second
	rebuildEvery := max(1, int(time.Minute/interval))
	for tick := 1; sleep(operation, interval); tick++ {
		if tick%rebuildEvery == 0 {
			current.rebuild(operation)
		}
		current.flush(operation)
	}
	flushing, cancel := context.WithTimeout(context.WithoutCancel(operation), 5*time.Second)
	current.flush(flushing)
	cancel()
}

func (current *registry) rebuild(operation context.Context) {
	for _, identifier := range current.activeIDs() {
		counters, failure := current.countersFor(operation, identifier)
		if failure != nil {
			if operation.Err() == nil {
				current.engine.Logger.Warn("campaign counters were not rebuilt", "campaign_id", identifier, "error", failure)
			}
			continue
		}
		current.mutex.Lock()
		if existing := current.runtimes[identifier]; existing != nil {
			existing.counters = counters
			existing.dirty = true
		}
		current.mutex.Unlock()
	}
}

func (current *registry) flush(operation context.Context) {
	type pending struct {
		identifier uuid.UUID
		counters   []campaign.Counter
	}
	var writes []pending
	current.mutex.Lock()
	for identifier, existing := range current.runtimes {
		if !existing.dirty {
			continue
		}
		var counters []campaign.Counter
		for phase, states := range existing.counters {
			for state, count := range states {
				if count > 0 {
					counters = append(counters, campaign.Counter{Phase: phase, State: state, Count: count})
				}
			}
		}
		existing.dirty = false
		writes = append(writes, pending{identifier, counters})
	}
	current.mutex.Unlock()
	for _, write := range writes {
		if failure := current.engine.Campaigns.WriteCounters(operation, write.identifier, write.counters); failure != nil {
			current.mutex.Lock()
			if existing := current.runtimes[write.identifier]; existing != nil {
				existing.dirty = true
			}
			current.mutex.Unlock()
			current.engine.Logger.Warn("campaign counters were not flushed", "campaign_id", write.identifier, "error", failure)
		}
	}
}
