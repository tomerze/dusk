package engine

import (
	"context"
	"errors"
	"math/rand/v2"
	"net/http"
	"sort"
	"sync"
	"time"

	"github.com/google/uuid"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/inventory"
)

var dispatchedProcesses = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_dispatch_processes_total",
	Help: "Campaign processes twilight handed to dawn, by action kind and outcome.",
}, []string{"kind", "outcome"})

type batch struct {
	pids     map[campaign.Pid]bool
	deadline time.Time
}

type dispatcher struct {
	engine     *Engine
	mutex      sync.Mutex
	ready      map[NodeKey]map[uuid.UUID]bool
	queues     map[uuid.UUID][]NodeKey
	inFlight   map[NodeKey]*batch
	reaps      *keyQueue
	reapTokens *TokenBucket
	wake       chan struct{}
	workers    chan struct{}
}

func newDispatcher(engine *Engine) *dispatcher {
	reapsPerSecond := engine.Config.Engine.ReapsPerSecond
	return &dispatcher{
		engine:     engine,
		ready:      map[NodeKey]map[uuid.UUID]bool{},
		queues:     map[uuid.UUID][]NodeKey{},
		inFlight:   map[NodeKey]*batch{},
		reaps:      newKeyQueue(engine.Config.Engine.EvaluationQueue),
		reapTokens: NewTokenBucket(float64(reapsPerSecond), reapsPerSecond, time.Now()),
		wake:       make(chan struct{}, 1),
		workers:    make(chan struct{}, engine.Config.Engine.DispatchWorkers),
	}
}

func (current *dispatcher) reset() {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	current.ready = map[NodeKey]map[uuid.UUID]bool{}
	current.queues = map[uuid.UUID][]NodeKey{}
	current.inFlight = map[NodeKey]*batch{}
	current.reaps.take(current.reaps.size())
}

func (current *dispatcher) queueLimit(identifier uuid.UUID) int {
	target := current.engine.registry.campaignOf(identifier)
	if target == nil {
		return 0
	}
	return max(1000, int(2*target.Policy.Rate.PerSecond*float64(current.engine.Config.Engine.SweepIntervalSeconds)))
}

func (current *dispatcher) offer(identifier uuid.UUID, key NodeKey) {
	limit := current.queueLimit(identifier)
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if current.ready[key][identifier] {
		return
	}
	if len(current.queues[identifier]) >= limit {
		return
	}
	if current.ready[key] == nil {
		current.ready[key] = map[uuid.UUID]bool{}
	}
	current.ready[key][identifier] = true
	current.queues[identifier] = append(current.queues[identifier], key)
	select {
	case current.wake <- struct{}{}:
	default:
	}
}

func (current *dispatcher) inFlightFor(key NodeKey) bool {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	_, busy := current.inFlight[key]
	return busy
}

func (current *dispatcher) complete(key NodeKey, pid campaign.Pid) {
	current.mutex.Lock()
	entry := current.inFlight[key]
	done := false
	if entry != nil {
		delete(entry.pids, pid)
		if len(entry.pids) == 0 {
			delete(current.inFlight, key)
			done = true
		}
	}
	current.mutex.Unlock()
	if done {
		current.engine.hint(key)
	}
}

func (current *dispatcher) expire(now time.Time) {
	current.mutex.Lock()
	var expired []NodeKey
	for key, entry := range current.inFlight {
		if now.After(entry.deadline) {
			delete(current.inFlight, key)
			expired = append(expired, key)
		}
	}
	current.mutex.Unlock()
	for _, key := range expired {
		current.engine.hint(key)
	}
}

type pick struct {
	key       NodeKey
	campaigns []uuid.UUID
	reap      bool
	found     bool
}

func (current *dispatcher) next(now time.Time) (pick, time.Duration) {
	identifiers := current.engine.registry.activeIDs()
	sort.Slice(identifiers, func(left, right int) bool { return identifiers[left].String() < identifiers[right].String() })
	wait := time.Second
	current.mutex.Lock()
	defer current.mutex.Unlock()
	active := make(map[uuid.UUID]bool, len(identifiers))
	for _, identifier := range identifiers {
		active[identifier] = true
	}
	for identifier, queue := range current.queues {
		if active[identifier] {
			continue
		}
		for _, key := range queue {
			delete(current.ready[key], identifier)
			if len(current.ready[key]) == 0 {
				delete(current.ready, key)
			}
		}
		delete(current.queues, identifier)
	}
	for _, identifier := range identifiers {
		for len(current.queues[identifier]) > 0 {
			available, until := current.engine.registry.tokenAvailable(identifier, now)
			if !available {
				wait = min(wait, max(until, 5*time.Millisecond))
				break
			}
			key := current.queues[identifier][0]
			current.queues[identifier] = current.queues[identifier][1:]
			delete(current.ready[key], identifier)
			if _, busy := current.inFlight[key]; busy {
				continue
			}
			if !current.engine.registry.takeToken(identifier, now) {
				continue
			}
			chosen := pick{key: key, campaigns: []uuid.UUID{identifier}, found: true}
			for other := range current.ready[key] {
				if current.engine.registry.takeToken(other, now) {
					chosen.campaigns = append(chosen.campaigns, other)
					delete(current.ready[key], other)
					queue := current.queues[other]
					for index, queued := range queue {
						if queued == key {
							current.queues[other] = append(queue[:index:index], queue[index+1:]...)
							break
						}
					}
				}
			}
			if len(current.ready[key]) == 0 {
				delete(current.ready, key)
			}
			current.inFlight[key] = &batch{pids: map[campaign.Pid]bool{}, deadline: now.Add(time.Minute)}
			return chosen, 0
		}
		if len(current.queues[identifier]) == 0 {
			delete(current.queues, identifier)
		}
	}
	if current.reaps.size() > 0 {
		if !current.reapTokens.Available(now) {
			return pick{}, min(wait, max(current.reapTokens.Wait(now), 5*time.Millisecond))
		}
		for _, key := range current.reaps.take(1) {
			if _, busy := current.inFlight[key]; busy {
				continue
			}
			current.reapTokens.Take(now)
			current.inFlight[key] = &batch{pids: map[campaign.Pid]bool{}, deadline: now.Add(time.Minute)}
			return pick{key: key, reap: true, found: true}, 0
		}
		wait = 0
	}
	return pick{}, wait
}

func (current *dispatcher) run(operation context.Context, term int64) {
	var running sync.WaitGroup
	defer running.Wait()
	for operation.Err() == nil {
		chosen, wait := current.next(time.Now())
		if !chosen.found {
			if wait == 0 {
				continue
			}
			select {
			case <-operation.Done():
				return
			case <-current.wake:
			case <-time.After(wait):
			}
			continue
		}
		select {
		case current.workers <- struct{}{}:
		case <-operation.Done():
			return
		}
		running.Add(1)
		go func() {
			defer running.Done()
			defer func() { <-current.workers }()
			if chosen.reap {
				current.reap(operation, chosen.key)
				return
			}
			current.dispatch(operation, term, chosen)
		}()
	}
}

func kindOrder(kind campaign.Kind) int {
	switch kind {
	case campaign.KindQuarantine:
		return 0
	case campaign.KindRunScript:
		return 1
	case campaign.KindEnsureConfig:
		return 2
	}
	return 3
}

type prepared struct {
	campaign *campaign.Campaign
	row      campaign.Row
	intended campaign.IntendedProcess
}

func workFor(target *campaign.Campaign, row campaign.Row) dawn.Work {
	identifier, attempt := target.ID.String(), row.Attempt
	work := dawn.Work{
		Pid:            row.Pid,
		CampaignID:     &identifier,
		Attempt:        &attempt,
		Kind:           string(target.Kind),
		TimeoutSeconds: int(target.ProcessTimeout() / time.Second),
		CollectFiles:   target.Action.CollectFiles,
	}
	if target.Action.Script != "" {
		script := target.Action.Script
		work.Script = &script
	}
	switch target.Kind {
	case campaign.KindEnsureVersion:
		versionKey, version := target.Action.VersionKey, target.Action.Version
		work.VersionKey, work.DesiredVersion = &versionKey, &version
	case campaign.KindEnsureConfig:
		configHash := target.Action.ConfigHash
		work.ConfigHash = &configHash
	}
	if target.Action.StreamLogs != nil {
		work.StreamLogs = &dawn.StreamLogs{Level: target.Action.StreamLogs.Level, DurationSeconds: target.Action.StreamLogs.DurationSeconds}
	}
	return work
}

func (engine *Engine) campaignCommands(target *campaign.Campaign) int {
	commands := 1 + len(target.Action.CollectFiles)
	if target.Action.StreamLogs != nil {
		commands++
	}
	return commands
}

func (current *dispatcher) release(key NodeKey) {
	current.mutex.Lock()
	delete(current.inFlight, key)
	current.mutex.Unlock()
}

func (current *dispatcher) dispatch(operation context.Context, term int64, chosen pick) {
	engine := current.engine
	key := chosen.key
	now := time.Now()
	session, online := engine.view.Target(key)
	var items []prepared
	for _, identifier := range chosen.campaigns {
		target := engine.registry.campaignOf(identifier)
		if target == nil || target.Status != campaign.StatusRunning {
			engine.registry.returnTokens(identifier, 1)
			continue
		}
		row, failure := engine.Campaigns.Row(operation, identifier, key.DeviceID, key.InstallationID)
		if failure != nil || (row.State != campaign.StatePending && row.State != campaign.StateBackoff) || !dispatchable(Target{Campaign: target}, row, sessionPointer(session, online), now) {
			engine.registry.returnTokens(identifier, 1)
			continue
		}
		items = append(items, prepared{campaign: target, row: row})
	}
	sort.SliceStable(items, func(left, right int) bool {
		leftOrder, rightOrder := kindOrder(items[left].campaign.Kind), kindOrder(items[right].campaign.Kind)
		if leftOrder != rightOrder {
			return leftOrder < rightOrder
		}
		return items[left].campaign.StartedAt.Before(*items[right].campaign.StartedAt)
	})
	var node *inventory.Node
	if loaded, failure := engine.Inventory.Get(operation, key); failure == nil {
		node = &loaded
	}
	var cumulative time.Duration
	var work []dawn.Work
	var sent []prepared
	latest := now
	for _, item := range items {
		target := item.campaign
		if target.Kind == campaign.KindQuarantine && (target.Action.Script == "" || (!online && !target.Action.RequireScriptSuccess)) {
			next, changed := item.row.QuarantineWithoutScript(now)
			if changed {
				engine.write(operation, term, &item.row, next, "quarantine without script")
			}
			continue
		}
		if !online {
			engine.registry.returnTokens(target.ID, 1)
			continue
		}
		cumulative += target.ProcessTimeout()
		deadline := now.Add(cumulative + time.Minute)
		var breakdown map[string]string
		if node != nil {
			breakdown = node.Breakdown(target.Policy.Gates.Breakdown)
		}
		next, resend := item.row.Dispatch(target.ID, campaign.Session{NamespaceID: session.NamespaceID, Epoch: session.Epoch}, now, deadline, breakdown)
		attempt := next.Attempt
		intended := campaign.IntendedProcess{Pid: next.Pid, CampaignID: &target.ID, Attempt: &attempt, DeviceID: key.DeviceID, InstallationID: key.InstallationID,
			ActionKind: string(target.Kind), Principal: target.CreatedBy, Subject: target.Subject(), ExpiresAt: deadline.Add(5 * time.Minute),
			MaxCommands: engine.campaignCommands(target), DefaultShellCommands: engine.Config.Reconcile.DefaultShell.Commands(string(target.Kind), 0), At: now}
		written, applied, failure := engine.Campaigns.Dispatching(operation, term, item.row, next, intended)
		if failure != nil || !applied {
			if failure != nil && operation.Err() == nil {
				engine.Logger.Warn("a dispatch was not recorded", "campaign_id", target.ID, "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
			}
			engine.registry.returnTokens(target.ID, 1)
			continue
		}
		engine.registry.count(target.ID, &item.row, written)
		reason := "dispatch"
		if resend {
			reason = "resend"
		}
		engine.logTransition(&item.row, written, reason)
		work = append(work, workFor(target, written))
		sent = append(sent, prepared{campaign: target, row: written, intended: intended})
		latest = deadline
	}
	if len(work) == 0 {
		current.release(key)
		return
	}
	current.mutex.Lock()
	entry := current.inFlight[key]
	if entry == nil {
		entry = &batch{pids: map[campaign.Pid]bool{}}
		current.inFlight[key] = entry
	}
	for _, item := range sent {
		entry.pids[item.row.Pid] = true
	}
	entry.deadline = latest
	current.mutex.Unlock()
	reference := nodeReference(key, session)
	var accepted []campaign.Pid
	var failure error
	delay := time.Second
	for attempt := 1; attempt <= engine.Config.Engine.DispatchAttempts; attempt++ {
		if attempt > 1 {
			for _, item := range sent {
				item.intended.At = time.Now()
				if failure := engine.Campaigns.RecordIntendedProcess(operation, item.intended); failure != nil && operation.Err() == nil {
					engine.Logger.Warn("the command budget of a retried dispatch was not recorded", "campaign_id", item.campaign.ID, "pid", item.row.Pid, "error", failure)
				}
			}
		}
		calling, cancel := context.WithTimeout(operation, time.Duration(engine.Config.Dawn.RequestTimeoutSeconds)*time.Second)
		accepted, failure = engine.Dawn.Dispatch(calling, reference, work)
		cancel()
		if dawn.OutcomeOf(failure) != dawn.Ambiguous || attempt == engine.Config.Engine.DispatchAttempts || operation.Err() != nil {
			break
		}
		engine.Logger.Warn("dawn's answer to a dispatch was lost; retrying with the same pids", "device_id", key.DeviceID, "installation_id", key.InstallationID, "attempt", attempt, "error", failure)
		sleep(operation, time.Duration(rand.Int64N(int64(delay)))+1)
		delay *= 2
	}
	outcome := dawn.OutcomeOf(failure)
	acceptedSet := map[campaign.Pid]bool{}
	for _, pid := range accepted {
		acceptedSet[pid] = true
	}
	for _, item := range sent {
		dispatchedProcesses.WithLabelValues(string(item.campaign.Kind), outcome.String()).Inc()
		if failure := engine.Campaigns.RecordDispatch(operation, item.campaign.ID, now); failure != nil && operation.Err() == nil {
			engine.Logger.Warn("last_dispatch_at was not recorded", "campaign_id", item.campaign.ID, "error", failure)
		}
		switch {
		case outcome == dawn.Accepted && acceptedSet[item.row.Pid]:
			if next, changed := item.row.Accepted(item.row.Pid); changed {
				engine.write(operation, term, &item.row, next, "dawn accepted")
			}
		case outcome == dawn.Ambiguous:
			engine.Logger.Warn("dispatch left ambiguous; the row waits for a result or its deadline", "campaign_id", item.campaign.ID, "pid", item.row.Pid, "error", failure)
		default:
			status, delayFor := "dawn_unreachable", item.campaign.Policy.Backoff(item.row.Unreached+1, engine.randomFloat())
			message := "dawn did not accept the process"
			if failure != nil {
				message = failure.Error()
			}
			switch outcome {
			case dawn.Busy:
				status, delayFor = "dawn_busy", busyDelay(item.row.Unreached+1, engine.randomFloat())
				engine.registry.returnTokens(item.campaign.ID, 1)
			case dawn.Rejected:
				status = "dawn_rejected"
				engine.Logger.Error("dawn rejected a dispatch", "campaign_id", item.campaign.ID, "pid", item.row.Pid, "error", failure)
				var dawnError *dawn.Error
				refused := errors.As(failure, &dawnError) && (dawnError.Status == http.StatusUnauthorized || dawnError.Status == http.StatusForbidden)
				if refused || item.row.Unreached+1 >= rejectedDispatches {
					engine.refuseDispatch(operation, term, item, message)
					current.complete(key, item.row.Pid)
					continue
				}
			}
			if next, changed := item.row.Undelivered(status, message, time.Now(), delayFor); changed {
				latest := item.campaign
				if current, failure := engine.campaignFor(operation, item.campaign.ID); failure == nil {
					latest = current
				}
				engine.write(operation, term, &item.row, next.Settle(latest, time.Now()), status)
			}
			current.complete(key, item.row.Pid)
		}
	}
	if outcome == dawn.Accepted {
		pids := make([]string, len(sent))
		for index, item := range sent {
			pids[index] = item.row.Pid.String()
		}
		engine.Logger.Info("dispatched", "device_id", key.DeviceID, "installation_id", key.InstallationID, "namespace_id", session.NamespaceID, "epoch", session.Epoch, "pids", pids)
	}
}

const rejectedDispatches = 3

func (engine *Engine) refuseDispatch(operation context.Context, term int64, item prepared, message string) {
	refused := campaign.Result{Pid: item.row.Pid, Status: campaign.ResultDenied, Error: message}
	if next, _, changed := item.row.ApplyResult(item.campaign, refused, time.Now(), engine.randomFloat()); changed {
		engine.write(operation, term, &item.row, next, "dawn refused")
	}
	engine.pauseForPermission(operation, item.campaign, "dawn refused the dispatch: "+message,
		map[string]any{"pid": item.row.Pid.String(), "device_id": item.row.DeviceID, "installation_id": item.row.InstallationID})
}

func (engine *Engine) pauseForPermission(operation context.Context, target *campaign.Campaign, reason string, detail map[string]any) {
	updated, failure := engine.Campaigns.Transition(operation, target.ID, campaign.TransitionRequest{
		Transition: campaign.TransitionPause, PauseKind: campaign.PausePermission, Reason: reason, Actor: "twilight", Now: time.Now(), Detail: detail,
	})
	if failure != nil {
		if !errors.Is(failure, campaign.ErrInvalidTransition) {
			engine.Logger.Warn("pausing a campaign after a refusal failed", "campaign_id", target.ID, "error", failure)
		}
		return
	}
	engine.registry.setStatus(target.ID, updated)
	engine.Logger.Warn("campaign paused: a process was refused", "campaign_id", target.ID, "reason", reason, "detail", detail)
}

func nodeReference(key NodeKey, session Session) dawn.NodeRef {
	nightfall := session.InnerAddress
	return dawn.NodeRef{DeviceID: key.DeviceID, InstallationID: key.InstallationID, NamespaceID: session.NamespaceID, Nightfall: &nightfall}
}

func busyDelay(unreached int, random float64) time.Duration {
	ceiling := min(5*time.Minute, 10*time.Second<<min(unreached-1, 5))
	return time.Duration(random * float64(ceiling))
}

func sessionPointer(session Session, online bool) *Session {
	if !online {
		return nil
	}
	return &session
}
