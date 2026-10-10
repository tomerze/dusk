package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
)

type sweepRequest struct {
	campaign uuid.UUID
	all      bool
}

func (engine *Engine) requestSweep(request sweepRequest) {
	select {
	case engine.sweeps <- request:
	default:
		engine.Logger.Warn("a sweep request was dropped because the sweep queue is full; the periodic sweep will cover it", "campaign_id", request.campaign)
	}
}

func (engine *Engine) runSweeps(operation context.Context, term int64) {
	interval := time.Duration(engine.Config.Engine.SweepIntervalSeconds) * time.Second
	timer := time.NewTimer(interval)
	defer timer.Stop()
	for {
		var request sweepRequest
		select {
		case <-operation.Done():
			return
		case request = <-engine.sweeps:
		case <-timer.C:
			request = sweepRequest{all: true}
			timer.Reset(interval)
		}
		targets := engine.registry.targets()
		for _, target := range targets {
			if !request.all && target.Campaign.ID != request.campaign {
				continue
			}
			if target.Campaign.Status != campaign.StatusRunning {
				continue
			}
			if failure := engine.sweep(operation, term, target.Campaign, !request.all); failure != nil && operation.Err() == nil {
				engine.Logger.Warn("sweep failed", "campaign_id", target.Campaign.ID, "error", failure)
			}
		}
		if request.all {
			engine.refreshStaleFacts(operation)
		}
	}
}

type sweepCursor struct {
	DeviceID       string `json:"device_id"`
	InstallationID string `json:"installation_id"`
	Phase          int    `json:"phase"`
	Missed         bool   `json:"missed"`
	Swept          *int   `json:"swept"`
}

func (engine *Engine) readSweepCursor(operation context.Context, identifier uuid.UUID) (sweepCursor, error) {
	var stored []byte
	if failure := engine.Pool.QueryRow(operation, `select coalesce(sweep_cursor, '{}'::jsonb) from campaigns where id = $1`, identifier).Scan(&stored); failure != nil {
		return sweepCursor{}, failure
	}
	var cursor sweepCursor
	if failure := json.Unmarshal(stored, &cursor); failure != nil {
		engine.Logger.Warn("a campaign's sweep cursor does not decode; its sweep starts over", "campaign_id", identifier, "error", failure)
		return sweepCursor{}, nil
	}
	return cursor, nil
}

func (engine *Engine) sweep(operation context.Context, term int64, target *campaign.Campaign, fresh bool) error {
	if target.SelectorSQL == "" {
		return errors.New("the campaign has no frozen selector")
	}
	limit := max(1000, int(2*target.Policy.Rate.PerSecond*float64(engine.Config.Engine.SweepIntervalSeconds)))
	threshold := target.Thresholds()[target.CurrentPhase].String()
	cursor, failure := engine.readSweepCursor(operation, target.ID)
	if failure != nil {
		return failure
	}
	if fresh || cursor.Phase != target.CurrentPhase {
		cursor = sweepCursor{Phase: target.CurrentPhase, Swept: cursor.Swept}
	}
	candidates, scanned, missed := 0, 0, 0
	now := time.Now()
	for page := 0; candidates < limit && page < 200; page++ {
		values := make([]any, 0, len(target.SelectorParameters)+6)
		for _, parameter := range target.SelectorParameters {
			values = append(values, parameter)
		}
		offset := len(values)
		values = append(values, target.ID, now, target.Salt, threshold, cursor.DeviceID, cursor.InstallationID)
		query := fmt.Sprintf(`select n.device_id, n.installation_id from nodes n
			left join campaign_nodes c on c.campaign_id = $%d and c.device_id = n.device_id and c.installation_id = n.installation_id
			where %s
			and (c.device_id is null or c.state in ('excluded', 'conflict') or (c.state = 'pending' and (c.next_attempt_at is null or c.next_attempt_at <= $%d))
				or (c.state = 'backoff' and c.next_attempt_at <= $%d))
			and phase_bucket(n.device_id, n.installation_id, $%d) < $%d::numeric
			and (n.device_id, n.installation_id) > ($%d, $%d)
			order by n.device_id, n.installation_id limit 1000`, offset+1, target.SelectorSQL, offset+2, offset+2, offset+3, offset+4, offset+5, offset+6)
		rows, failure := engine.Pool.Query(operation, query, values...)
		if failure != nil {
			return failure
		}
		keys, failure := collectKeys(rows)
		if failure != nil {
			return failure
		}
		scanned += len(keys)
		reachable := make([]NodeKey, 0, len(keys))
		for _, key := range keys {
			if target.Kind == campaign.KindQuarantine || engine.view.Online(key) {
				reachable = append(reachable, key)
			}
		}
		unevaluated, failure := engine.evaluateBatch(operation, term, reachable)
		if failure != nil {
			return failure
		}
		candidates += len(reachable)
		missed += unevaluated
		cursor.Missed = cursor.Missed || unevaluated > 0
		finished := len(keys) < 1000
		if finished {
			if !cursor.Missed {
				swept := cursor.Phase
				cursor.Swept = &swept
			}
			cursor = sweepCursor{Phase: target.CurrentPhase, Swept: cursor.Swept}
		} else {
			last := keys[len(keys)-1]
			cursor.DeviceID, cursor.InstallationID = last.DeviceID, last.InstallationID
		}
		encoded, failure := json.Marshal(cursor)
		if failure != nil {
			return failure
		}
		tag, failure := engine.Pool.Exec(operation, `update campaigns set sweep_cursor = $2::jsonb where id = $1 and exists (select 1 from leadership where id = 1 and term = $3)`, target.ID, encoded, term)
		if failure != nil {
			return failure
		}
		if tag.RowsAffected() == 0 {
			return fmt.Errorf("the sweep cursor was not stored: term %d no longer leads, or the campaign is gone", term)
		}
		if finished {
			engine.Logger.Info("sweep pass finished", "campaign_id", target.ID, "phase", target.CurrentPhase, "complete", cursor.Swept != nil && *cursor.Swept == target.CurrentPhase)
			break
		}
	}
	engine.Logger.Info("sweep finished", "campaign_id", target.ID, "phase", target.CurrentPhase, "candidates", candidates, "scanned", scanned, "not_evaluated", missed)
	return nil
}

func (engine *Engine) sweptPhase(operation context.Context, identifier uuid.UUID) (int, error) {
	cursor, failure := engine.readSweepCursor(operation, identifier)
	if failure != nil || cursor.Swept == nil {
		return -1, failure
	}
	return *cursor.Swept, nil
}

func collectKeys(rows interface {
	Next() bool
	Scan(...any) error
	Close()
	Err() error
}) ([]NodeKey, error) {
	defer rows.Close()
	var keys []NodeKey
	for rows.Next() {
		var key NodeKey
		if failure := rows.Scan(&key.DeviceID, &key.InstallationID); failure != nil {
			return nil, failure
		}
		keys = append(keys, key)
	}
	return keys, rows.Err()
}

func (engine *Engine) refreshStaleFacts(operation context.Context) {
	maximumAge := time.Duration(engine.Config.Engine.FactsMaxAgeSeconds) * time.Second
	rows, failure := engine.Pool.Query(operation, `select device_id, installation_id from nodes where facts_read_at is null or facts_read_at < $1 limit 50000`, time.Now().Add(-maximumAge))
	if failure != nil {
		engine.Logger.Warn("reading nodes with stale facts failed", "error", failure)
		return
	}
	defer rows.Close()
	queued := 0
	for rows.Next() {
		var key NodeKey
		if failure := rows.Scan(&key.DeviceID, &key.InstallationID); failure != nil {
			return
		}
		if engine.view.Online(key) && engine.facts.offer(key) {
			queued++
		}
	}
	if queued > 0 {
		engine.Logger.Info("facts refresh queued for nodes with stale facts", "nodes", queued)
	}
}

func (engine *Engine) runDueScan(operation context.Context, term int64) {
	for sleep(operation, 15*time.Second) {
		identifiers := engine.registry.activeIDs()
		if len(identifiers) == 0 {
			continue
		}
		rows, failure := engine.Pool.Query(operation, `select device_id, installation_id from campaign_nodes
			where campaign_id = any($1) and state in ('pending', 'backoff') and next_attempt_at <= $2 order by next_attempt_at limit 20000`, identifiers, time.Now())
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("the due scan failed", "error", failure)
			}
			continue
		}
		var keys []NodeKey
		for rows.Next() {
			var key NodeKey
			if rows.Scan(&key.DeviceID, &key.InstallationID) == nil {
				keys = append(keys, key)
			}
		}
		rows.Close()
		for _, key := range keys {
			if engine.view.Online(key) && !engine.dispatcher.inFlightFor(key) {
				engine.hint(key)
			}
		}
	}
}
