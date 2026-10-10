package campaign

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/selector"
)

var (
	ErrNotFound        = errors.New("campaign not found")
	ErrVersionConflict = errors.New("the campaign changed since it was read")
	ErrNotDraft        = errors.New("only a draft campaign can be edited")
	ErrNotArchivable   = errors.New("the campaign cannot be archived yet")
	ErrRowNotFound     = errors.New("campaign node not found")
	ErrMalformedCursor = errors.New("malformed cursor")
)

type querier interface {
	Exec(operation context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	Query(operation context.Context, sql string, arguments ...any) (pgx.Rows, error)
	QueryRow(operation context.Context, sql string, arguments ...any) pgx.Row
}

type Store struct {
	pool *pgxpool.Pool
}

func NewStore(pool *pgxpool.Pool) *Store {
	return &Store{pool: pool}
}

func (store *Store) Pool() *pgxpool.Pool {
	return store.pool
}

const campaignColumns = `id, name, description, tenant, status, kind, selector, coalesce(selector_sql_snapshot, ''), selector_sql_parameters,
	action, policy, salt, created_by, created_at, updated_at, started_at, paused_at, finished_at, current_phase, phase_started_at,
	phase_paused_seconds, gate_override_after, pause_kind, pause_reason, abort_reason, last_dispatch_at, version`

func scanCampaign(row pgx.Row) (*Campaign, error) {
	campaign := &Campaign{}
	var parameters, action, policy []byte
	failure := row.Scan(&campaign.ID, &campaign.Name, &campaign.Description, &campaign.Tenant, &campaign.Status, &campaign.Kind,
		&campaign.Selector, &campaign.SelectorSQL, &parameters, &action, &policy, &campaign.Salt, &campaign.CreatedBy, &campaign.CreatedAt,
		&campaign.UpdatedAt, &campaign.StartedAt, &campaign.PausedAt, &campaign.FinishedAt, &campaign.CurrentPhase, &campaign.PhaseStartedAt,
		&campaign.PhasePausedSeconds, &campaign.GateOverrideAfter, &campaign.PauseKind, &campaign.PauseReason, &campaign.AbortReason,
		&campaign.LastDispatchAt, &campaign.Version)
	if errors.Is(failure, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if failure != nil {
		return nil, failure
	}
	if len(parameters) > 0 {
		if failure := json.Unmarshal(parameters, &campaign.SelectorParameters); failure != nil {
			return nil, fmt.Errorf("campaign %s selector parameters: %w", campaign.ID, failure)
		}
	}
	if failure := json.Unmarshal(action, &campaign.Action); failure != nil {
		return nil, fmt.Errorf("campaign %s action: %w", campaign.ID, failure)
	}
	if failure := json.Unmarshal(policy, &campaign.Policy); failure != nil {
		return nil, fmt.Errorf("campaign %s policy: %w", campaign.ID, failure)
	}
	campaign.thresholds = campaign.Policy.Thresholds()
	return campaign, nil
}

func addEvent(operation context.Context, target querier, campaign uuid.UUID, kind, actor string, detail any) error {
	if detail == nil {
		detail = map[string]any{}
	}
	encoded, failure := json.Marshal(detail)
	if failure != nil {
		return failure
	}
	_, failure = target.Exec(operation, `insert into campaign_events (campaign_id, kind, actor, detail) values ($1, $2, $3, $4::jsonb)`, campaign, kind, actor, encoded)
	return failure
}

func (store *Store) AddEvent(operation context.Context, campaign uuid.UUID, kind, actor string, detail any) error {
	return addEvent(operation, store.pool, campaign, kind, actor, detail)
}

func (store *Store) Create(operation context.Context, definition Definition, createdBy string, now time.Time) (*Campaign, error) {
	definition.Normalize()
	if failure := definition.Validate(now); failure != nil {
		return nil, failure
	}
	salt := make([]byte, 32)
	if _, failure := rand.Read(salt); failure != nil {
		return nil, failure
	}
	identifier, failure := uuid.NewV7()
	if failure != nil {
		return nil, failure
	}
	action, _ := json.Marshal(definition.Action)
	policy, _ := json.Marshal(definition.Policy)
	var created *Campaign
	failure = pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		if failure := database.CreateCampaignPartitions(operation, transaction, identifier); failure != nil {
			return failure
		}
		row := transaction.QueryRow(operation, `insert into campaigns (id, name, description, tenant, status, kind, selector, action, policy, salt, created_by, created_at, updated_at)
			values ($1, $2, $3, $4, 'draft', $5, $6, $7::jsonb, $8::jsonb, $9, $10, $11, $11) returning `+campaignColumns,
			identifier, definition.Name, definition.Description, definition.Tenant, definition.Action.Kind, definition.Selector, action, policy, salt, createdBy, now)
		var failure error
		if created, failure = scanCampaign(row); failure != nil {
			return failure
		}
		return addEvent(operation, transaction, identifier, "created", createdBy, map[string]any{"name": definition.Name})
	})
	if failure != nil {
		return nil, fmt.Errorf("create campaign: %w", failure)
	}
	return created, nil
}

func (store *Store) Get(operation context.Context, identifier uuid.UUID) (*Campaign, error) {
	return scanCampaign(store.pool.QueryRow(operation, `select `+campaignColumns+` from campaigns where id = $1`, identifier))
}

func (store *Store) Active(operation context.Context) ([]*Campaign, error) {
	rows, failure := store.pool.Query(operation, `select `+campaignColumns+` from campaigns where status in ('running', 'paused') order by started_at, id`)
	if failure != nil {
		return nil, failure
	}
	defer rows.Close()
	var campaigns []*Campaign
	for rows.Next() {
		campaign, failure := scanCampaign(rows)
		if failure != nil {
			return nil, failure
		}
		campaigns = append(campaigns, campaign)
	}
	return campaigns, rows.Err()
}

type ListPage struct {
	Campaigns []*Campaign `json:"campaigns"`
	Next      string      `json:"next"`
}

func encodeListCursor(campaign *Campaign) string {
	return base64.RawURLEncoding.EncodeToString([]byte(campaign.CreatedAt.UTC().Format(time.RFC3339Nano) + "|" + campaign.ID.String()))
}

func decodeListCursor(cursor string) (time.Time, uuid.UUID, error) {
	decoded, failure := base64.RawURLEncoding.DecodeString(cursor)
	if failure != nil {
		return time.Time{}, uuid.Nil, ErrMalformedCursor
	}
	at, identifier, found := strings.Cut(string(decoded), "|")
	if !found {
		return time.Time{}, uuid.Nil, ErrMalformedCursor
	}
	parsedTime, timeFailure := time.Parse(time.RFC3339Nano, at)
	parsedIdentifier, identifierFailure := uuid.Parse(identifier)
	if timeFailure != nil || identifierFailure != nil {
		return time.Time{}, uuid.Nil, ErrMalformedCursor
	}
	return parsedTime, parsedIdentifier, nil
}

func (store *Store) List(operation context.Context, statuses []Status, cursor string, limit int) (ListPage, error) {
	if limit < 1 || limit > 500 {
		return ListPage{}, errors.New("limit must be between 1 and 500")
	}
	query := `select ` + campaignColumns + ` from campaigns where true`
	var values []any
	if len(statuses) > 0 {
		names := make([]string, len(statuses))
		for index, status := range statuses {
			names[index] = string(status)
		}
		values = append(values, names)
		query += fmt.Sprintf(` and status = any($%d)`, len(values))
	}
	if cursor != "" {
		at, identifier, failure := decodeListCursor(cursor)
		if failure != nil {
			return ListPage{}, failure
		}
		values = append(values, at, identifier)
		query += fmt.Sprintf(` and (created_at, id) < ($%d, $%d)`, len(values)-1, len(values))
	}
	values = append(values, limit+1)
	query += fmt.Sprintf(` order by created_at desc, id desc limit $%d`, len(values))
	rows, failure := store.pool.Query(operation, query, values...)
	if failure != nil {
		return ListPage{}, failure
	}
	defer rows.Close()
	var page ListPage
	for rows.Next() {
		campaign, failure := scanCampaign(rows)
		if failure != nil {
			return ListPage{}, failure
		}
		page.Campaigns = append(page.Campaigns, campaign)
	}
	if failure := rows.Err(); failure != nil {
		return ListPage{}, failure
	}
	if len(page.Campaigns) > limit {
		page.Campaigns = page.Campaigns[:limit]
		page.Next = encodeListCursor(page.Campaigns[limit-1])
	}
	return page, nil
}

func (store *Store) UpdateDefinition(operation context.Context, identifier uuid.UUID, definition Definition, expectedVersion int, actor string, now time.Time) (*Campaign, error) {
	definition.Normalize()
	if failure := definition.Validate(now); failure != nil {
		return nil, failure
	}
	action, _ := json.Marshal(definition.Action)
	policy, _ := json.Marshal(definition.Policy)
	var updated *Campaign
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		current, failure := scanCampaign(transaction.QueryRow(operation, `select `+campaignColumns+` from campaigns where id = $1 for update`, identifier))
		if failure != nil {
			return failure
		}
		if current.Status != StatusDraft {
			return ErrNotDraft
		}
		if current.Version != expectedVersion {
			return ErrVersionConflict
		}
		updated, failure = scanCampaign(transaction.QueryRow(operation, `update campaigns set name = $2, description = $3, tenant = $4, kind = $5, selector = $6,
			action = $7::jsonb, policy = $8::jsonb, updated_at = $9, version = version + 1 where id = $1 returning `+campaignColumns,
			identifier, definition.Name, definition.Description, definition.Tenant, definition.Action.Kind, definition.Selector, action, policy, now))
		if failure != nil {
			return failure
		}
		return addEvent(operation, transaction, identifier, "updated", actor, nil)
	})
	return updated, failure
}

var transitionEvents = map[Transition]string{
	TransitionStart:    "started",
	TransitionPause:    "paused",
	TransitionResume:   "resumed",
	TransitionAbort:    "aborted",
	TransitionFail:     "failed",
	TransitionComplete: "completed",
	TransitionArchive:  "archived",
}

type TransitionRequest struct {
	Transition   Transition
	Actor        string
	Reason       string
	PauseKind    PauseKind
	OverrideGate bool
	Now          time.Time
	Detail       map[string]any
	Alert        *alerts.Raised
}

func (store *Store) Transition(operation context.Context, identifier uuid.UUID, request TransitionRequest) (*Campaign, error) {
	var updated *Campaign
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		current, failure := scanCampaign(transaction.QueryRow(operation, `select `+campaignColumns+` from campaigns where id = $1 for update`, identifier))
		if failure != nil {
			return failure
		}
		next, failure := NextStatus(current.Status, request.Transition)
		if failure != nil {
			return failure
		}
		now := request.Now
		detail := map[string]any{}
		for key, value := range request.Detail {
			detail[key] = value
		}
		if request.Reason != "" {
			detail["reason"] = request.Reason
		}
		set := []string{"status = $2", "updated_at = $3", "version = version + 1"}
		values := []any{identifier, next, now}
		add := func(column string, value any) {
			values = append(values, value)
			set = append(set, fmt.Sprintf("%s = $%d", column, len(values)))
		}
		cancelRows := false
		switch request.Transition {
		case TransitionStart:
			parsed, failure := selector.Parse(current.Selector)
			if failure != nil {
				return failure
			}
			compiled := parsed.Compile("n", 1)
			parameters, _ := json.Marshal(compiled.Parameters)
			add("selector_sql_snapshot", compiled.SQL)
			add("selector_sql_parameters", string(parameters))
			set[len(set)-1] += "::jsonb"
			add("started_at", now)
			add("current_phase", 0)
			add("phase_started_at", now)
			add("phase_paused_seconds", 0)
		case TransitionPause:
			kind := request.PauseKind
			if kind == "" {
				kind = PauseOperator
			}
			add("paused_at", now)
			add("pause_kind", string(kind))
			add("pause_reason", request.Reason)
			detail["pause_kind"] = string(kind)
		case TransitionResume:
			if current.PauseKind != nil && *current.PauseKind == PauseGate {
				if !request.OverrideGate || strings.TrimSpace(request.Reason) == "" {
					return ErrGateOverrideRequired
				}
				add("gate_override_after", now)
				detail["override_gate"] = true
			}
			if current.PausedAt != nil {
				add("phase_paused_seconds", current.PhasePausedSeconds+int64(now.Sub(*current.PausedAt).Seconds()))
			}
			add("paused_at", nil)
			add("pause_kind", nil)
			add("pause_reason", nil)
		case TransitionAbort, TransitionFail:
			add("finished_at", now)
			add("abort_reason", request.Reason)
			cancelRows = true
		case TransitionComplete:
			add("finished_at", now)
			cancelRows = true
		case TransitionArchive:
			if now.Before(current.ArchivableAt()) {
				return fmt.Errorf("%w: wait until %s, an hour past the node timeout after the last dispatch", ErrNotArchivable, current.ArchivableAt().UTC().Format(time.RFC3339))
			}
		}
		updated, failure = scanCampaign(transaction.QueryRow(operation, `update campaigns set `+strings.Join(set, ", ")+` where id = $1 returning `+campaignColumns, values...))
		if failure != nil {
			return failure
		}
		if cancelRows {
			if _, failure := transaction.Exec(operation, `update campaign_nodes set state = 'cancelled', finished_at = $2, next_attempt_at = null, deadline_at = null, revision = revision + 1, updated_at = now()
				where campaign_id = $1 and state in ('pending', 'backoff', 'excluded', 'conflict', 'verifying')`, identifier, now); failure != nil {
				return failure
			}
		}
		if request.Transition == TransitionResume || request.Transition == TransitionAbort || request.Transition == TransitionComplete || request.Transition == TransitionFail {
			if failure := alerts.ResolveFingerprintIn(operation, transaction, alerts.KindCampaignPausedByGate+":"+identifier.String(), request.Actor, now); failure != nil {
				return failure
			}
		}
		if request.Alert != nil {
			if _, _, failure := alerts.RaiseIn(operation, transaction, *request.Alert); failure != nil {
				return failure
			}
		}
		if request.Transition == TransitionArchive {
			return nil
		}
		return addEvent(operation, transaction, identifier, transitionEvents[request.Transition], request.Actor, detail)
	})
	if failure != nil {
		return nil, failure
	}
	if request.Transition == TransitionArchive {
		if failure := database.DropCampaignPartitions(operation, store.pool, identifier); failure != nil {
			return updated, fmt.Errorf("the campaign is archived but its partitions were not dropped: %w", failure)
		}
	}
	return updated, nil
}

func (store *Store) AdvancePhase(operation context.Context, identifier uuid.UUID, fromPhase int, now time.Time, detail map[string]any) (bool, error) {
	advanced := false
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		tag, failure := transaction.Exec(operation, `update campaigns set current_phase = current_phase + 1, phase_started_at = $3, phase_paused_seconds = 0,
			gate_override_after = null, updated_at = $3 where id = $1 and status = 'running' and current_phase = $2`, identifier, fromPhase, now)
		if failure != nil || tag.RowsAffected() == 0 {
			return failure
		}
		advanced = true
		return addEvent(operation, transaction, identifier, "phase_advanced", "twilight", detail)
	})
	return advanced, failure
}

func (store *Store) HoldForGate(operation context.Context, identifier uuid.UUID, reason string, detail map[string]any, now time.Time) (bool, error) {
	held := false
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		tag, failure := transaction.Exec(operation, `update campaigns set pause_kind = 'gate', pause_reason = $2, updated_at = $3
			where id = $1 and status = 'paused' and pause_kind is distinct from 'gate'`, identifier, reason, now)
		if failure != nil || tag.RowsAffected() == 0 {
			return failure
		}
		held = true
		recorded := map[string]any{"reason": reason, "pause_kind": string(PauseGate)}
		for key, value := range detail {
			recorded[key] = value
		}
		return addEvent(operation, transaction, identifier, "gate_failed", "twilight", recorded)
	})
	return held, failure
}

func (store *Store) RecordDispatch(operation context.Context, identifier uuid.UUID, at time.Time) error {
	_, failure := store.pool.Exec(operation, `update campaigns set last_dispatch_at = greatest(coalesce(last_dispatch_at, $2), $2) where id = $1`, identifier, at)
	return failure
}

type Event struct {
	ID     int64           `json:"id"`
	Time   time.Time       `json:"time"`
	Kind   string          `json:"kind"`
	Actor  string          `json:"actor"`
	Detail json.RawMessage `json:"detail"`
}

func (store *Store) Events(operation context.Context, identifier uuid.UUID, after int64, limit int) ([]Event, error) {
	if limit < 1 || limit > 1000 {
		return nil, errors.New("limit must be between 1 and 1000")
	}
	rows, failure := store.pool.Query(operation, `select id, time, kind, actor, detail from campaign_events where campaign_id = $1 and id > $2 order by id limit $3`, identifier, after, limit)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Event, error) {
		var event Event
		return event, row.Scan(&event.ID, &event.Time, &event.Kind, &event.Actor, &event.Detail)
	})
}

type Counter struct {
	Phase int       `json:"phase"`
	State NodeState `json:"state"`
	Count int64     `json:"count"`
}

func (store *Store) Counters(operation context.Context, identifiers []uuid.UUID) (map[uuid.UUID][]Counter, error) {
	rows, failure := store.pool.Query(operation, `select campaign_id, phase, state, count from campaign_counters where campaign_id = any($1) and count > 0 order by campaign_id, phase, state`, identifiers)
	if failure != nil {
		return nil, failure
	}
	defer rows.Close()
	counters := map[uuid.UUID][]Counter{}
	for rows.Next() {
		var identifier uuid.UUID
		var counter Counter
		if failure := rows.Scan(&identifier, &counter.Phase, &counter.State, &counter.Count); failure != nil {
			return nil, failure
		}
		counters[identifier] = append(counters[identifier], counter)
	}
	return counters, rows.Err()
}

func (store *Store) CountRows(operation context.Context, identifier uuid.UUID) ([]Counter, error) {
	rows, failure := store.pool.Query(operation, `select phase, state, count(*) from campaign_nodes where campaign_id = $1 group by phase, state order by phase, state`, identifier)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Counter, error) {
		var counter Counter
		return counter, row.Scan(&counter.Phase, &counter.State, &counter.Count)
	})
}

func (store *Store) WriteCounters(operation context.Context, identifier uuid.UUID, counters []Counter) error {
	return pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `delete from campaign_counters where campaign_id = $1`, identifier); failure != nil {
			return failure
		}
		for _, counter := range counters {
			if counter.Count <= 0 {
				continue
			}
			if _, failure := transaction.Exec(operation, `insert into campaign_counters (campaign_id, phase, state, count) values ($1, $2, $3, $4)`,
				identifier, counter.Phase, counter.State, counter.Count); failure != nil {
				return failure
			}
		}
		return nil
	})
}

const rowColumns = `campaign_id, device_id, installation_id, phase, state, attempt, failures, unreached, pid::text, coalesce(epoch, 0), coalesce(namespace_id, ''),
	dispatched_at, delivered_at, deadline_at, finished_at, next_attempt_at, coalesce(last_error, ''), coalesce(last_status, ''), event_at, back_at, silent, reaped_at, breakdown, revision`

func pidText(pid Pid) *string {
	if pid == 0 {
		return nil
	}
	text := pid.String()
	return &text
}

func scanPid(text *string) (Pid, error) {
	if text == nil {
		return 0, nil
	}
	return ParsePid(*text)
}

func scanRow(row pgx.Row) (Row, error) {
	var scanned Row
	var breakdown []byte
	var pid *string
	failure := row.Scan(&scanned.CampaignID, &scanned.DeviceID, &scanned.InstallationID, &scanned.Phase, &scanned.State, &scanned.Attempt, &scanned.Failures, &scanned.Unreached,
		&pid, &scanned.Epoch, &scanned.NamespaceID, &scanned.DispatchedAt, &scanned.DeliveredAt, &scanned.DeadlineAt, &scanned.FinishedAt,
		&scanned.NextAttemptAt, &scanned.LastError, &scanned.LastStatus, &scanned.EventAt, &scanned.BackAt, &scanned.Silent, &scanned.ReapedAt, &breakdown, &scanned.Revision)
	if errors.Is(failure, pgx.ErrNoRows) {
		return scanned, ErrRowNotFound
	}
	if failure != nil {
		return scanned, failure
	}
	if scanned.Pid, failure = scanPid(pid); failure != nil {
		return scanned, fmt.Errorf("pid: %w", failure)
	}
	scanned.Breakdown = map[string]string{}
	if len(breakdown) > 0 {
		if failure := json.Unmarshal(breakdown, &scanned.Breakdown); failure != nil {
			return scanned, fmt.Errorf("breakdown: %w", failure)
		}
	}
	return scanned, nil
}

func collectRows(rows pgx.Rows) ([]Row, error) {
	defer rows.Close()
	var collected []Row
	for rows.Next() {
		scanned, failure := scanRow(rows)
		if failure != nil {
			return nil, failure
		}
		collected = append(collected, scanned)
	}
	return collected, rows.Err()
}

func (store *Store) Row(operation context.Context, identifier uuid.UUID, device, installation string) (Row, error) {
	return scanRow(store.pool.QueryRow(operation, `select `+rowColumns+` from campaign_nodes where campaign_id = $1 and device_id = $2 and installation_id = $3`, identifier, device, installation))
}

func (store *Store) RowsForNodes(operation context.Context, campaigns []uuid.UUID, devices, installations []string) ([]Row, error) {
	if len(campaigns) == 0 || len(devices) == 0 {
		return nil, nil
	}
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes
		where campaign_id = any($1) and (device_id, installation_id) in (select * from unnest($2::text[], $3::text[]))`, campaigns, devices, installations)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}

type RowQuery struct {
	States []NodeState
	Phase  *int
	After  string
	Limit  int
}

type RowPage struct {
	Rows []Row  `json:"nodes"`
	Next string `json:"next"`
}

func (store *Store) Rows(operation context.Context, identifier uuid.UUID, query RowQuery) (RowPage, error) {
	if query.Limit < 1 || query.Limit > 1000 {
		return RowPage{}, errors.New("limit must be between 1 and 1000")
	}
	statement := `select ` + rowColumns + ` from campaign_nodes where campaign_id = $1`
	values := []any{identifier}
	if len(query.States) > 0 {
		names := make([]string, len(query.States))
		for index, state := range query.States {
			names[index] = string(state)
		}
		values = append(values, names)
		statement += fmt.Sprintf(` and state = any($%d)`, len(values))
	}
	if query.Phase != nil {
		values = append(values, *query.Phase)
		statement += fmt.Sprintf(` and phase = $%d`, len(values))
	}
	if query.After != "" {
		decoded, failure := base64.RawURLEncoding.DecodeString(query.After)
		device, installation, found := strings.Cut(string(decoded), "/")
		if failure != nil || !found {
			return RowPage{}, ErrMalformedCursor
		}
		values = append(values, device, installation)
		statement += fmt.Sprintf(` and (device_id, installation_id) > ($%d, $%d)`, len(values)-1, len(values))
	}
	values = append(values, query.Limit+1)
	statement += fmt.Sprintf(` order by device_id, installation_id limit $%d`, len(values))
	rows, failure := store.pool.Query(operation, statement, values...)
	if failure != nil {
		return RowPage{}, failure
	}
	collected, failure := collectRows(rows)
	if failure != nil {
		return RowPage{}, failure
	}
	page := RowPage{Rows: collected}
	if len(collected) > query.Limit {
		page.Rows = collected[:query.Limit]
		last := page.Rows[query.Limit-1]
		page.Next = base64.RawURLEncoding.EncodeToString([]byte(last.DeviceID + "/" + last.InstallationID))
	}
	return page, nil
}

func (store *Store) DueRows(operation context.Context, identifier uuid.UUID, now time.Time, limit int) ([]Row, error) {
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes where campaign_id = $1
		and ((state = 'pending' and (next_attempt_at is null or next_attempt_at <= $2)) or (state = 'backoff' and next_attempt_at <= $2))
		order by next_attempt_at nulls first limit $3`, identifier, now, limit)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}

func (store *Store) ExpiredRows(operation context.Context, now time.Time, limit int) ([]Row, error) {
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes
		where state in ('dispatching', 'dispatched', 'delivered') and deadline_at <= $1 order by deadline_at limit $2`, now, limit)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}

func (store *Store) RowsInState(operation context.Context, identifier uuid.UUID, state NodeState, limit int) ([]Row, error) {
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes where campaign_id = $1 and state = $2 limit $3`, identifier, state, limit)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}

func (store *Store) Outstanding(operation context.Context, identifier uuid.UUID) (bool, error) {
	var outstanding bool
	failure := store.pool.QueryRow(operation, `select exists (select 1 from campaign_nodes where campaign_id = $1
		and state in ('pending', 'dispatching', 'dispatched', 'delivered', 'backoff', 'verifying'))`, identifier).Scan(&outstanding)
	return outstanding, failure
}

const termGuard = `exists (select 1 from leadership where id = 1 and term = $%d)`

func (store *Store) InsertRow(operation context.Context, term int64, row Row) (Row, bool, error) {
	breakdown, _ := json.Marshal(row.Breakdown)
	tag, failure := store.pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, attempt, failures, finished_at, last_status, breakdown, revision)
		select $1, $2, $3, $4, $5, $6, $7, $8, nullif($9, ''), $10::jsonb, 1 where `+fmt.Sprintf(termGuard, 11)+`
		on conflict do nothing`,
		row.CampaignID, row.DeviceID, row.InstallationID, row.Phase, row.State, row.Attempt, row.Failures, row.FinishedAt, row.LastStatus, breakdown, term)
	if failure != nil {
		return row, false, failure
	}
	row.Revision = 1
	return row, tag.RowsAffected() == 1, nil
}

func updateRow(operation context.Context, target querier, term int64, previous, next Row) (Row, bool, error) {
	breakdown, _ := json.Marshal(next.Breakdown)
	tag, failure := target.Exec(operation, `update campaign_nodes set phase = $5, state = $6, attempt = $7, failures = $8,
		reaped_at = case when pid is not distinct from $9::text::numeric then reaped_at end, pid = $9::text::numeric, epoch = nullif($10::bigint, 0),
		namespace_id = nullif($11, ''), dispatched_at = $12, delivered_at = $13, deadline_at = $14, finished_at = $15, next_attempt_at = $16,
		last_error = nullif($17, ''), last_status = nullif($18, ''), event_at = $19, back_at = $20, silent = $21, breakdown = $22::jsonb,
		unreached = $24, revision = revision + 1, updated_at = now()
		where campaign_id = $1 and device_id = $2 and installation_id = $3 and revision = $4 and `+fmt.Sprintf(termGuard, 23),
		previous.CampaignID, previous.DeviceID, previous.InstallationID, previous.Revision,
		next.Phase, next.State, next.Attempt, next.Failures, pidText(next.Pid), next.Epoch, next.NamespaceID, next.DispatchedAt, next.DeliveredAt,
		next.DeadlineAt, next.FinishedAt, next.NextAttemptAt, next.LastError, next.LastStatus, next.EventAt, next.BackAt, next.Silent, breakdown, term, next.Unreached)
	if failure != nil {
		return previous, false, failure
	}
	if tag.RowsAffected() == 0 {
		return previous, false, nil
	}
	if next.Pid != previous.Pid {
		next.ReapedAt = nil
	}
	next.Revision = previous.Revision + 1
	return next, true, nil
}

func (store *Store) UpdateRow(operation context.Context, term int64, previous, next Row) (Row, bool, error) {
	return updateRow(operation, store.pool, term, previous, next)
}

type IntendedProcess struct {
	Pid                  Pid
	CampaignID           *uuid.UUID
	Attempt              *int
	DeviceID             string
	InstallationID       string
	ActionKind           string
	Principal            string
	Subject              string
	ExpiresAt            time.Time
	MaxCommands          int
	DefaultShellCommands int
	At                   time.Time
}

func recordIntendedProcess(operation context.Context, target querier, intended IntendedProcess) error {
	tag, failure := target.Exec(operation, `update intended_processes set expires_at = greatest(expires_at, $2), last_dispatched_at = greatest(last_dispatched_at, $3),
			default_shell_commands = default_shell_commands + $6, max_commands = max_commands + $7, revision = revision + 1, tombstoned_at = null
		where pid = $1::text::numeric and device_id = $4 and installation_id = $5`,
		intended.Pid.String(), intended.ExpiresAt, intended.At, intended.DeviceID, intended.InstallationID, intended.DefaultShellCommands, intended.MaxCommands)
	if failure != nil || tag.RowsAffected() > 0 {
		return failure
	}
	_, failure = target.Exec(operation, `insert into intended_processes (pid, created_at, campaign_id, attempt, device_id, installation_id, action_kind, principal, subject,
			expires_at, max_commands, default_shell_commands, last_dispatched_at)
		values ($1::text::numeric, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $2)`,
		intended.Pid.String(), intended.At, intended.CampaignID, intended.Attempt, intended.DeviceID, intended.InstallationID, intended.ActionKind,
		intended.Principal, intended.Subject, intended.ExpiresAt, intended.MaxCommands, intended.DefaultShellCommands)
	return failure
}

func (store *Store) RecordIntendedProcess(operation context.Context, intended IntendedProcess) error {
	return recordIntendedProcess(operation, store.pool, intended)
}

type IntendedKey struct {
	DeviceID       string
	InstallationID string
	Pid            Pid
}

type IntendedRecord struct {
	IntendedKey
	CampaignID           *uuid.UUID
	ActionKind           string
	Principal            string
	Subject              string
	CreatedAt            time.Time
	ExpiresAt            time.Time
	MaxCommands          int
	DefaultShellCommands int
	Live                 bool
}

func (store *Store) PublishIntendedProcess(operation context.Context, key IntendedKey, now time.Time, produce func(IntendedRecord) error) (bool, error) {
	published := false
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		record := IntendedRecord{IntendedKey: key}
		var created time.Time
		var revision int64
		var reapedAt *time.Time
		row := transaction.QueryRow(operation, `select created_at, campaign_id, action_kind, principal, subject, expires_at, max_commands, default_shell_commands, revision, reaped_at
			from intended_processes where pid = $1::text::numeric and device_id = $2 and installation_id = $3
			order by created_at desc limit 1 for update`, key.Pid.String(), key.DeviceID, key.InstallationID)
		if failure := row.Scan(&created, &record.CampaignID, &record.ActionKind, &record.Principal, &record.Subject, &record.ExpiresAt,
			&record.MaxCommands, &record.DefaultShellCommands, &revision, &reapedAt); failure != nil {
			if errors.Is(failure, pgx.ErrNoRows) {
				return nil
			}
			return fmt.Errorf("read the intended process to publish: %w", failure)
		}
		record.CreatedAt = created
		record.Live = reapedAt == nil && record.ExpiresAt.After(now)
		if failure := produce(record); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `update intended_processes set published_revision = $4,
				tombstoned_at = case when $5 then null else coalesce(tombstoned_at, $6) end
			where pid = $1::text::numeric and created_at = $2 and device_id = $3`,
			key.Pid.String(), created, key.DeviceID, revision, record.Live, now); failure != nil {
			return fmt.Errorf("mark the intended process published: %w", failure)
		}
		published = true
		return nil
	})
	return published, failure
}

func (store *Store) IntendedToPublish(operation context.Context, now time.Time, limit int) ([]IntendedKey, error) {
	rows, failure := store.pool.Query(operation, `(select device_id, installation_id, pid::text from intended_processes
			where published_revision < revision and tombstoned_at is null and reaped_at is null and expires_at > $1
			order by created_at limit $2)
		union
		(select device_id, installation_id, pid::text from intended_processes
			where tombstoned_at is null and (reaped_at is not null or expires_at <= $1)
			order by expires_at limit $2)`, now, limit)
	if failure != nil {
		return nil, fmt.Errorf("read the intended processes to publish: %w", failure)
	}
	defer rows.Close()
	var keys []IntendedKey
	for rows.Next() {
		var key IntendedKey
		var pid string
		if failure := rows.Scan(&key.DeviceID, &key.InstallationID, &pid); failure != nil {
			return nil, fmt.Errorf("read an intended process to publish: %w", failure)
		}
		parsed, failure := ParsePid(pid)
		if failure != nil {
			return nil, failure
		}
		key.Pid = parsed
		keys = append(keys, key)
	}
	return keys, rows.Err()
}

type Reapable struct {
	DeviceID       string
	InstallationID string
	Pid            Pid
}

func (store *Store) Reapable(operation context.Context, devices, installations []string, now time.Time, retryAfter time.Duration, perNode int) ([]Reapable, error) {
	if len(devices) == 0 {
		return nil, nil
	}
	rows, failure := store.pool.Query(operation, `select node.device_id, node.installation_id, reapable.pid::text
		from unnest($1::text[], $2::text[]) as node (device_id, installation_id)
		cross join lateral (
			select intended.pid from intended_processes intended
			left join campaigns on campaigns.id = intended.campaign_id
			where intended.device_id = node.device_id and intended.installation_id = node.installation_id
				and intended.reaped_at is null and intended.action_kind <> 'reap'
				and (intended.reap_requested_at is null or intended.reap_requested_at < $3::timestamptz - make_interval(secs => $4))
				and intended.expires_at < $3::timestamptz - make_interval(secs => coalesce(
					(campaigns.policy -> 'retry' ->> 'max_backoff_seconds')::double precision
					+ (campaigns.policy ->> 'node_timeout_seconds')::double precision, 0))
				and not exists (
					select 1 from campaign_nodes holder
					where holder.campaign_id = intended.campaign_id and holder.device_id = intended.device_id
						and holder.installation_id = intended.installation_id and holder.pid = intended.pid
						and holder.state not in ('succeeded', 'failed', 'unknown', 'cancelled'))
			order by intended.expires_at, intended.pid
			limit $5) reapable
		order by node.device_id, node.installation_id, reapable.pid`, devices, installations, now, retryAfter.Seconds(), perNode)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Reapable, error) {
		var found Reapable
		var pid *string
		if failure := row.Scan(&found.DeviceID, &found.InstallationID, &pid); failure != nil {
			return found, failure
		}
		var failure error
		found.Pid, failure = scanPid(pid)
		return found, failure
	})
}

func pidTexts(pids []Pid) []string {
	texts := make([]string, len(pids))
	for index, pid := range pids {
		texts[index] = pid.String()
	}
	return texts
}

func (store *Store) RecordReapRequested(operation context.Context, device, installation string, pids []Pid, at time.Time) error {
	if len(pids) == 0 {
		return nil
	}
	_, failure := store.pool.Exec(operation, `update intended_processes set reap_requested_at = $4
		where pid = any($1::text[]::numeric[]) and device_id = $2 and installation_id = $3 and reaped_at is null`, pidTexts(pids), device, installation, at)
	return failure
}

func (store *Store) RecordReaped(operation context.Context, device, installation string, pid Pid, at time.Time) (bool, error) {
	recorded := false
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		rows, failure := transaction.Query(operation, `update intended_processes set reaped_at = $4
			where pid = $1::text::numeric and device_id = $2 and installation_id = $3 and reaped_at is null
			returning campaign_id`, pid.String(), device, installation, at)
		if failure != nil {
			return failure
		}
		campaigns, failure := pgx.CollectRows(rows, pgx.RowTo[*uuid.UUID])
		if failure != nil {
			return failure
		}
		recorded = len(campaigns) > 0
		for _, identifier := range campaigns {
			if identifier == nil {
				continue
			}
			if _, failure := transaction.Exec(operation, `update campaign_nodes set reaped_at = $5
				where campaign_id = $1 and device_id = $2 and installation_id = $3 and pid = $4::text::numeric and reaped_at is null`,
				*identifier, device, installation, pid.String(), at); failure != nil {
				return failure
			}
		}
		return nil
	})
	return recorded, failure
}

func (store *Store) Dispatching(operation context.Context, term int64, previous, next Row, intended IntendedProcess) (Row, bool, error) {
	var updated Row
	changed := false
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		var failure error
		updated, changed, failure = updateRow(operation, transaction, term, previous, next)
		if failure != nil || !changed {
			return failure
		}
		return recordIntendedProcess(operation, transaction, intended)
	})
	if failure != nil {
		return previous, false, failure
	}
	return updated, changed, nil
}

func (store *Store) Overlap(operation context.Context, campaign *Campaign) (int64, []uuid.UUID, error) {
	if !campaign.Kind.Converging() {
		return 0, nil, nil
	}
	running, failure := store.Active(operation)
	if failure != nil {
		return 0, nil, failure
	}
	parsed, failure := selector.Parse(campaign.Selector)
	if failure != nil {
		return 0, nil, failure
	}
	var total int64
	var overlapping []uuid.UUID
	for _, other := range running {
		if other.ID == campaign.ID || other.Kind != campaign.Kind || (campaign.Kind == KindEnsureVersion && other.Action.VersionKey != campaign.Action.VersionKey) {
			continue
		}
		otherParsed, failure := selector.Parse(other.Selector)
		if failure != nil {
			continue
		}
		mine := parsed.Compile("n", 1)
		theirs := otherParsed.Compile("n", len(mine.Parameters)+1)
		values := make([]any, 0, len(mine.Parameters)+len(theirs.Parameters))
		for _, parameter := range append(mine.Parameters, theirs.Parameters...) {
			values = append(values, parameter)
		}
		var count int64
		if failure := store.pool.QueryRow(operation, `select count(*) from nodes n where `+mine.SQL+` and `+theirs.SQL, values...).Scan(&count); failure != nil {
			return 0, nil, failure
		}
		if count > 0 {
			total += count
			overlapping = append(overlapping, other.ID)
		}
	}
	return total, overlapping, nil
}

func (store *Store) Term(operation context.Context) (int64, error) {
	var term int64
	failure := store.pool.QueryRow(operation, `select term from leadership where id = 1`).Scan(&term)
	return term, failure
}

func (store *Store) RowsForNode(operation context.Context, device, installation string, limit int) ([]Row, error) {
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes where device_id = $1 and installation_id = $2 order by updated_at desc limit $3`, device, installation, limit)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}

func (store *Store) RowsByKeys(operation context.Context, identifier uuid.UUID, states []NodeState, devices, installations []string, limit int) ([]Row, error) {
	names := make([]string, len(states))
	for index, state := range states {
		names[index] = string(state)
	}
	query := `select ` + rowColumns + ` from campaign_nodes where campaign_id = $1 and ($2::text[] is null or state = any($2))`
	values := []any{identifier, names}
	if len(states) == 0 {
		values[1] = nil
	}
	if len(devices) > 0 {
		query += ` and (device_id, installation_id) in (select * from unnest($4::text[], $5::text[]))`
		values = append(values, limit, devices, installations)
	} else {
		values = append(values, limit)
	}
	rows, failure := store.pool.Query(operation, query+` limit $3`, values...)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}
