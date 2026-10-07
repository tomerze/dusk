package engine

import (
	"context"
	"errors"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
)

var gateVerdicts = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_gate_verdicts_total",
	Help: "Health gate verdicts, by verdict.",
}, []string{"verdict"})

func (engine *Engine) runGates(operation context.Context, term int64) {
	interval := time.Duration(engine.Config.Engine.GateIntervalSeconds) * time.Second
	for sleep(operation, interval) {
		for _, target := range engine.registry.targets() {
			if failure := engine.gate(operation, term, target.Campaign); failure != nil && operation.Err() == nil {
				engine.Logger.Warn("gate evaluation failed", "campaign_id", target.Campaign.ID, "error", failure)
			}
		}
	}
}

func (engine *Engine) markLapsed(operation context.Context, term int64, target *campaign.Campaign, now time.Time) error {
	window := time.Duration(target.Policy.Gates.SilentWindowSeconds) * time.Second
	_, failure := engine.Pool.Exec(operation, `update campaign_nodes set silent = true, revision = revision + 1, updated_at = now()
		where campaign_id = $1 and state = 'succeeded' and event_at is not null and back_at is null and silent is null and event_at < $2
		and exists (select 1 from leadership where id = 1 and term = $3)`, target.ID, now.Add(-window), term)
	return failure
}

func (engine *Engine) Report(operation context.Context, target *campaign.Campaign) (GateReport, error) {
	overall, groups, rows, failure := Tallies(operation, engine.Pool, target, time.Now())
	if failure != nil {
		return GateReport{}, failure
	}
	return Judge(target.Policy.Gates, target.Policy.Abort.MaxTotalFailures, overall, groups, rows, engine.view.Degraded()), nil
}

func (engine *Engine) transition(operation context.Context, target *campaign.Campaign, request campaign.TransitionRequest) {
	updated, failure := engine.Campaigns.Transition(operation, target.ID, request)
	if failure != nil {
		if !errors.Is(failure, campaign.ErrInvalidTransition) {
			engine.Logger.Warn("a campaign transition failed", "campaign_id", target.ID, "transition", request.Transition, "error", failure)
		}
		return
	}
	engine.registry.setStatus(target.ID, updated)
	engine.Logger.Info("campaign transition", "campaign_id", target.ID, "transition", request.Transition, "status", updated.Status, "reason", request.Reason)
}

func (engine *Engine) gate(operation context.Context, term int64, target *campaign.Campaign) error {
	now := time.Now()
	if target.Policy.Deadline != nil && !now.Before(*target.Policy.Deadline) {
		engine.transition(operation, target, campaign.TransitionRequest{Transition: campaign.TransitionComplete, Actor: "twilight", Reason: "the campaign reached its deadline", Now: now})
		return nil
	}
	if !target.Status.Active() {
		return nil
	}
	paused := target.Status == campaign.StatusPaused
	if failure := engine.markLapsed(operation, term, target, now); failure != nil {
		return failure
	}
	report, failure := engine.Report(operation, target)
	if failure != nil {
		return failure
	}
	gateVerdicts.WithLabelValues(report.Verdict).Inc()
	existing := engine.registry.get(target.ID)
	switch report.Verdict {
	case VerdictFail:
		detail := map[string]any{"group": report.FailingGroup, "overall": report.Overall}
		switch {
		case target.Policy.Abort.OnGateFailure == campaign.GateActionAbort:
			engine.transition(operation, target, campaign.TransitionRequest{Transition: campaign.TransitionFail, PauseKind: campaign.PauseGate, Actor: "twilight", Reason: report.Reason, Now: now, Detail: detail})
		case paused:
			held, failure := engine.Campaigns.HoldForGate(operation, target.ID, report.Reason, detail, now)
			if failure != nil {
				return failure
			}
			if held {
				if refreshed, failure := engine.Campaigns.Get(operation, target.ID); failure == nil {
					engine.registry.setStatus(target.ID, refreshed)
				}
				engine.Logger.Info("a paused campaign's gate failed; resuming it needs a gate override", "campaign_id", target.ID, "reason", report.Reason)
			}
		default:
			engine.transition(operation, target, campaign.TransitionRequest{Transition: campaign.TransitionPause, PauseKind: campaign.PauseGate, Actor: "twilight", Reason: report.Reason, Now: now, Detail: detail})
		}
		return nil
	case VerdictHold:
		if existing != nil {
			engine.registry.mutex.Lock()
			changed := existing.lastHold != report.Reason
			existing.lastHold = report.Reason
			engine.registry.mutex.Unlock()
			if changed {
				if failure := engine.Campaigns.AddEvent(operation, target.ID, "gate_holding", "twilight", map[string]any{"reason": report.Reason, "phase": target.CurrentPhase}); failure != nil {
					engine.Logger.Warn("the gate event was not written", "campaign_id", target.ID, "error", failure)
				}
			}
		}
		return nil
	}
	if existing != nil {
		engine.registry.mutex.Lock()
		existing.lastHold = ""
		engine.registry.mutex.Unlock()
	}
	if paused || !target.BakeComplete(now) {
		return nil
	}
	if !target.LastPhase() {
		next := target.Policy.Phases[target.CurrentPhase+1]
		advanced, failure := engine.Campaigns.AdvancePhase(operation, target.ID, target.CurrentPhase, now, map[string]any{
			"from": target.Policy.Phases[target.CurrentPhase].Name, "to": next.Name, "percent": next.Percent, "overall": report.Overall,
		})
		if failure != nil {
			return failure
		}
		if advanced {
			engine.Logger.Info("campaign phase advanced", "campaign_id", target.ID, "phase", target.CurrentPhase+1, "name", next.Name)
			if refreshed, failure := engine.Campaigns.Get(operation, target.ID); failure == nil {
				engine.registry.setStatus(target.ID, refreshed)
			}
			engine.requestSweep(sweepRequest{campaign: target.ID})
		}
		return nil
	}
	if !target.Kind.OneShot() {
		return nil
	}
	swept, failure := engine.sweptPhase(operation, target.ID)
	if failure != nil || swept < target.CurrentPhase {
		return failure
	}
	outstanding, failure := engine.Campaigns.Outstanding(operation, target.ID)
	if failure != nil || outstanding {
		return failure
	}
	engine.transition(operation, target, campaign.TransitionRequest{Transition: campaign.TransitionComplete, Actor: "twilight", Reason: "every node finished and the last phase passed its gate", Now: now})
	return nil
}
