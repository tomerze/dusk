package engine

import (
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/selector"
)

type Target struct {
	Campaign *campaign.Campaign
	Selector *selector.Selector
}

type Decision struct {
	Target   Target
	Previous *campaign.Row
	Next     campaign.Row
	Write    bool
	Dispatch bool
	Conflict bool
}

func conflictGroup(target *campaign.Campaign) string {
	switch target.Kind {
	case campaign.KindEnsureVersion:
		return "ensure_version/" + target.Action.VersionKey
	case campaign.KindEnsureConfig:
		return "ensure_config"
	}
	return ""
}

func winners(targets []Target, node *inventory.Node) map[string]uuid.UUID {
	owners := map[string]uuid.UUID{}
	for _, target := range targets {
		group := conflictGroup(target.Campaign)
		if group == "" {
			continue
		}
		if _, taken := owners[group]; taken {
			continue
		}
		if target.Selector.Matches(node) {
			owners[group] = target.Campaign.ID
		}
	}
	return owners
}

func dispatchable(target Target, row campaign.Row, session *Session, now time.Time) bool {
	if target.Campaign.Status != campaign.StatusRunning {
		return false
	}
	if row.State != campaign.StatePending && row.State != campaign.StateBackoff {
		return false
	}
	if session == nil {
		return target.Campaign.Kind == campaign.KindQuarantine && row.Due(now)
	}
	return row.DueOnNewSession(campaign.Session{NamespaceID: session.NamespaceID, Epoch: session.Epoch}, now)
}

func Evaluate(targets []Target, node *inventory.Node, rows map[uuid.UUID]campaign.Row, session *Session, now time.Time, random float64) []Decision {
	owners := winners(targets, node)
	var decisions []Decision
	for _, target := range targets {
		current := target.Campaign
		running := current.Status == campaign.StatusRunning
		matches := target.Selector.Matches(node)
		phase, _ := current.PhaseOf(node.DeviceID, node.InstallationID)
		open := phase <= current.CurrentPhase
		converging := current.Kind.Converging()
		satisfied, known := current.Action.Satisfied(node)
		loser := converging && owners[conflictGroup(current)] != current.ID
		row, exists := rows[current.ID]
		if !exists {
			if !running || !matches || !open {
				continue
			}
			next := campaign.NewRow(current.ID, node.DeviceID, node.InstallationID, phase, campaign.StatePending)
			decision := Decision{Target: target, Write: true}
			switch {
			case loser:
				next.State = campaign.StateConflict
				decision.Conflict = true
			case converging && known && satisfied:
				next, _ = next.Satisfied(now)
			default:
				decision.Dispatch = dispatchable(target, next, session, now)
			}
			decision.Next = next
			decisions = append(decisions, decision)
			continue
		}
		next, changed := row, false
		conflict := false
		switch {
		case row.State == campaign.StateConflict:
			if running && !loser && matches {
				next, changed = row.Unconflict()
			}
		case row.State == campaign.StateVerifying:
			if converging {
				namespace := ""
				if node.FactsNamespaceID != nil {
					namespace = *node.FactsNamespaceID
				}
				next, changed = row.Verify(known && satisfied, namespace, current.Policy, now, random)
			}
		case row.State.Terminal():
			if row.State == campaign.StateSucceeded && converging && running && matches && known && !satisfied && !loser {
				next, changed = row.Drifted()
			}
		default:
			switch {
			case converging && known && satisfied:
				next, changed = row.Satisfied(now)
			case row.State == campaign.StateExcluded:
				if matches {
					next, changed = row.Include()
				}
			case !matches:
				next, changed = row.Exclude()
			case loser:
				next, changed = row.Conflict()
				conflict = changed
			}
		}
		dispatch := dispatchable(target, next, session, now)
		if !changed && !dispatch {
			continue
		}
		previous := row
		decisions = append(decisions, Decision{Target: target, Previous: &previous, Next: next, Write: changed, Dispatch: dispatch, Conflict: conflict})
	}
	return decisions
}
