package erasure

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const erasureColumns = `id, scope, device_id, tenant, reason, requested_by, requested_at, status, steps, retained, attempts, last_error, next_attempt_at, completed_at`

func scanErasure(row pgx.Row) (Erasure, error) {
	var erasure Erasure
	var steps, retained []byte
	failure := row.Scan(&erasure.ID, &erasure.Scope, &erasure.DeviceID, &erasure.Tenant, &erasure.Reason, &erasure.RequestedBy,
		&erasure.RequestedAt, &erasure.Status, &steps, &retained, &erasure.Attempts, &erasure.LastError, &erasure.NextAttemptAt, &erasure.CompletedAt)
	if errors.Is(failure, pgx.ErrNoRows) {
		return erasure, ErrNotFound
	}
	if failure != nil {
		return erasure, failure
	}
	if failure := json.Unmarshal(steps, &erasure.Steps); failure != nil {
		return erasure, fmt.Errorf("decode the steps of erasure %s: %w", erasure.ID, failure)
	}
	if failure := json.Unmarshal(retained, &erasure.Retained); failure != nil {
		return erasure, fmt.Errorf("decode what erasure %s retained: %w", erasure.ID, failure)
	}
	for index := range erasure.Steps {
		if erasure.Steps[index].Counts == nil {
			erasure.Steps[index].Counts = map[string]int64{}
		}
	}
	if erasure.Retained == nil {
		erasure.Retained = []Retained{}
	}
	return erasure, nil
}

type querier interface {
	QueryRow(operation context.Context, sql string, arguments ...any) pgx.Row
}

func insert(operation context.Context, target querier, erasure Erasure) (Erasure, bool, error) {
	steps, failure := json.Marshal(erasure.Steps)
	if failure != nil {
		return Erasure{}, false, failure
	}
	inserted, failure := scanErasure(target.QueryRow(operation, `insert into erasures (id, scope, device_id, tenant, reason, requested_by, requested_at,
			status, steps, next_attempt_at, updated_at)
		values ($1, $2, $3, $4, $5, $6, $7, 'running', $8::jsonb, $7, $7)
		on conflict (scope, coalesce(device_id, tenant)) where status = 'running' do nothing
		returning `+erasureColumns, erasure.ID, erasure.Scope, erasure.DeviceID, erasure.Tenant, erasure.Reason, erasure.RequestedBy,
		erasure.RequestedAt, steps))
	if errors.Is(failure, ErrNotFound) {
		running, failure := scanErasure(target.QueryRow(operation, `select `+erasureColumns+` from erasures
			where status = 'running' and scope = $1 and coalesce(device_id, tenant) = $2`,
			erasure.Scope, subjectOf(erasure)))
		return running, false, failure
	}
	return inserted, failure == nil, failure
}

func subjectOf(erasure Erasure) string {
	if erasure.DeviceID != nil {
		return *erasure.DeviceID
	}
	return *erasure.Tenant
}

func get(operation context.Context, pool *pgxpool.Pool, identifier uuid.UUID) (Erasure, error) {
	return scanErasure(pool.QueryRow(operation, `select `+erasureColumns+` from erasures where id = $1`, identifier))
}

func due(operation context.Context, pool *pgxpool.Pool, now time.Time, limit int) ([]Erasure, error) {
	rows, failure := pool.Query(operation, `select `+erasureColumns+` from erasures
		where status = 'running' and next_attempt_at <= $1 order by requested_at, id limit $2`, now, limit)
	if failure != nil {
		return nil, fmt.Errorf("read the erasures that are due: %w", failure)
	}
	defer rows.Close()
	var found []Erasure
	for rows.Next() {
		erasure, failure := scanErasure(rows)
		if failure != nil {
			return nil, failure
		}
		found = append(found, erasure)
	}
	return found, rows.Err()
}

func save(operation context.Context, pool *pgxpool.Pool, erasure Erasure, now time.Time) error {
	steps, failure := json.Marshal(erasure.Steps)
	if failure != nil {
		return failure
	}
	retained, failure := json.Marshal(erasure.Retained)
	if failure != nil {
		return failure
	}
	_, failure = pool.Exec(operation, `update erasures set steps = $2::jsonb, retained = $3::jsonb, status = $4, attempts = $5, last_error = $6,
			next_attempt_at = $7, updated_at = $8, completed_at = $9
		where id = $1`, erasure.ID, steps, retained, erasure.Status, erasure.Attempts, erasure.LastError, erasure.NextAttemptAt, now, erasure.CompletedAt)
	if failure != nil {
		return fmt.Errorf("save erasure %s: %w", erasure.ID, failure)
	}
	return nil
}

type stalled struct {
	ID          uuid.UUID
	Scope       Scope
	Subject     string
	RequestedAt time.Time
	Attempts    int
	LastError   *string
}

func stalledErasures(operation context.Context, pool *pgxpool.Pool, overdueBefore time.Time, attempts int) ([]stalled, error) {
	rows, failure := pool.Query(operation, `select id, scope, coalesce(device_id, tenant), requested_at, attempts, last_error from erasures
		where status = 'running' and (requested_at < $1 or attempts >= $2) order by requested_at limit 100`, overdueBefore, attempts)
	if failure != nil {
		return nil, fmt.Errorf("read the stalled erasures: %w", failure)
	}
	defer rows.Close()
	var found []stalled
	for rows.Next() {
		var current stalled
		if failure := rows.Scan(&current.ID, &current.Scope, &current.Subject, &current.RequestedAt, &current.Attempts, &current.LastError); failure != nil {
			return nil, failure
		}
		found = append(found, current)
	}
	return found, rows.Err()
}
