package alerts

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/config"
)

type Severity string

const (
	Critical Severity = "critical"
	High     Severity = "high"
	Medium   Severity = "medium"
	Low      Severity = "low"
)

const (
	KindEnrollmentRate            = "enrollment_rate"
	KindRevocationNotEnforced     = "revocation_not_enforced"
	KindCampaignConflict          = "campaign_conflict"
	KindProcessWithoutIntent      = "process_without_intent"
	KindDefaultShellWithoutIntent = "default_shell_without_intent"
	KindTargetMismatch            = "target_mismatch"
	KindProcessAfterDeadline      = "process_after_deadline"
	KindPidReused                 = "pid_reused"
	KindProcessAfterResult        = "process_after_result"
	KindProcessShape              = "process_shape"
	KindResultWithoutLedger       = "result_without_ledger"
	KindQuarantineOverride        = "quarantine_override"
	KindLedgerChainBroken         = "ledger_chain_broken"
	KindCampaignPausedByGate      = "campaign_paused_by_gate"
	KindCampaignFailedByPolicy    = "campaign_failed_by_policy"
)

type Alert struct {
	ID             int64           `json:"id"`
	Time           time.Time       `json:"time"`
	LastSeenAt     time.Time       `json:"last_seen_at"`
	Occurrences    int64           `json:"occurrences"`
	Severity       Severity        `json:"severity"`
	Kind           string          `json:"kind"`
	Fingerprint    string          `json:"fingerprint"`
	Tenant         *string         `json:"tenant"`
	Detail         json.RawMessage `json:"detail"`
	AcknowledgedBy *string         `json:"acknowledged_by"`
	AcknowledgedAt *time.Time      `json:"acknowledged_at"`
	ResolvedBy     *string         `json:"resolved_by"`
	ResolvedAt     *time.Time      `json:"resolved_at"`
	Deliveries     []Delivery      `json:"deliveries"`
}

type Delivery struct {
	Receiver      string     `json:"receiver"`
	Transition    string     `json:"transition"`
	State         string     `json:"state"`
	Attempts      int        `json:"attempts"`
	LastError     *string    `json:"last_error"`
	NextAttemptAt *time.Time `json:"next_attempt_at"`
	DeliveredAt   *time.Time `json:"delivered_at"`
}

type Raised struct {
	Severity    Severity
	Kind        string
	Fingerprint string
	Tenant      string
	Detail      map[string]any
	At          time.Time
}

type Querier interface {
	Exec(operation context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	QueryRow(operation context.Context, sql string, arguments ...any) pgx.Row
}

var (
	ErrNotFound        = errors.New("alert not found")
	ErrMalformedCursor = errors.New("malformed cursor")
	openAlerts         = promauto.NewGaugeVec(prometheus.GaugeOpts{
		Name: "twilight_alerts_open",
		Help: "Alerts that are not resolved, by severity and kind.",
	}, []string{"severity", "kind"})
)

type Store struct {
	pool     *pgxpool.Pool
	logger   *slog.Logger
	notifier *notifier
	exported map[[2]string]bool
}

func NewStore(pool *pgxpool.Pool, settings config.Alerts, instance string, logger *slog.Logger) (*Store, error) {
	built, failure := newNotifier(settings, instance, logger)
	if failure != nil {
		return nil, failure
	}
	return &Store{pool: pool, logger: logger, notifier: built}, nil
}

const alertColumns = `id, time, last_seen_at, occurrences, severity, kind, fingerprint, tenant, detail, acknowledged_by, acknowledged_at, resolved_by, resolved_at`

func alertTargets(alert *Alert) []any {
	return []any{&alert.ID, &alert.Time, &alert.LastSeenAt, &alert.Occurrences, &alert.Severity, &alert.Kind, &alert.Fingerprint,
		&alert.Tenant, &alert.Detail, &alert.AcknowledgedBy, &alert.AcknowledgedAt, &alert.ResolvedBy, &alert.ResolvedAt}
}

func scanAlert(row pgx.Row) (Alert, error) {
	var alert Alert
	failure := row.Scan(alertTargets(&alert)...)
	if errors.Is(failure, pgx.ErrNoRows) {
		return alert, ErrNotFound
	}
	alert.Deliveries = []Delivery{}
	return alert, failure
}

func RaiseIn(operation context.Context, target Querier, raised Raised) (Alert, bool, error) {
	detail, failure := json.Marshal(raised.Detail)
	if failure != nil {
		return Alert{}, false, failure
	}
	if raised.At.IsZero() {
		raised.At = time.Now()
	}
	var alert Alert
	var inserted bool
	failure = target.QueryRow(operation, `insert into alerts (time, last_seen_at, severity, kind, fingerprint, detail, tenant)
		values ($1, $1, $2, $3, $4, $5::jsonb, coalesce(nullif($6, ''),
			(select tenant from nodes where device_id = $5::jsonb ->> 'device_id' and installation_id = $5::jsonb ->> 'installation_id'),
			(select tenant from campaigns where id = case when $5::jsonb ->> 'campaign_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
				then ($5::jsonb ->> 'campaign_id')::uuid end)))
		on conflict (fingerprint) where resolved_at is null do update set occurrences = alerts.occurrences + 1,
			last_seen_at = greatest(alerts.last_seen_at, excluded.last_seen_at),
			severity = case when alert_severity_rank(excluded.severity) > alert_severity_rank(alerts.severity) then excluded.severity else alerts.severity end,
			detail = case when alert_severity_rank(excluded.severity) > alert_severity_rank(alerts.severity) then excluded.detail else alerts.detail end,
			acknowledged_by = case when alert_severity_rank(excluded.severity) > alert_severity_rank(alerts.severity) then null else alerts.acknowledged_by end,
			acknowledged_at = case when alert_severity_rank(excluded.severity) > alert_severity_rank(alerts.severity) then null else alerts.acknowledged_at end
		returning `+alertColumns+`, (xmax = 0)`, raised.At, raised.Severity, raised.Kind, raised.Fingerprint, detail, raised.Tenant).Scan(append(alertTargets(&alert), &inserted)...)
	alert.Deliveries = []Delivery{}
	return alert, inserted, failure
}

func (store *Store) Raise(operation context.Context, raised Raised) (Alert, error) {
	alert, inserted, failure := RaiseIn(operation, store.pool, raised)
	if failure != nil {
		return alert, fmt.Errorf("raise %s alert: %w", raised.Kind, failure)
	}
	if inserted {
		store.Announce(alert)
	}
	return alert, nil
}

func (store *Store) Announce(alert Alert) {
	store.logger.Error("alert raised", "alert_id", alert.ID, "severity", alert.Severity, "kind", alert.Kind, "fingerprint", alert.Fingerprint, "tenant", alert.Tenant, "detail", alert.Detail)
}

type Page struct {
	Alerts []Alert `json:"alerts"`
	Next   string  `json:"next"`
}

func encodeCursor(alert Alert) string {
	return base64.RawURLEncoding.EncodeToString([]byte(alert.Time.UTC().Format(time.RFC3339Nano) + "|" + strconv.FormatInt(alert.ID, 10)))
}

func decodeCursor(cursor string) (time.Time, int64, error) {
	decoded, failure := base64.RawURLEncoding.DecodeString(cursor)
	if failure != nil {
		return time.Time{}, 0, ErrMalformedCursor
	}
	at, identifier, found := strings.Cut(string(decoded), "|")
	if !found {
		return time.Time{}, 0, ErrMalformedCursor
	}
	parsedTime, timeFailure := time.Parse(time.RFC3339Nano, at)
	parsedIdentifier, identifierFailure := strconv.ParseInt(identifier, 10, 64)
	if timeFailure != nil || identifierFailure != nil {
		return time.Time{}, 0, ErrMalformedCursor
	}
	return parsedTime, parsedIdentifier, nil
}

func (store *Store) List(operation context.Context, open bool, cursor string, limit int) (Page, error) {
	if limit < 1 || limit > 1000 {
		return Page{}, errors.New("limit must be between 1 and 1000")
	}
	query := `select ` + alertColumns + ` from alerts where true`
	var values []any
	if open {
		query += ` and resolved_at is null`
	}
	if cursor != "" {
		at, identifier, failure := decodeCursor(cursor)
		if failure != nil {
			return Page{}, failure
		}
		values = append(values, at, identifier)
		query += ` and (time, id) < ($1, $2)`
	}
	values = append(values, limit+1)
	rows, failure := store.pool.Query(operation, query+fmt.Sprintf(` order by time desc, id desc limit $%d`, len(values)), values...)
	if failure != nil {
		return Page{}, failure
	}
	listed, failure := pgx.CollectRows(rows, func(row pgx.CollectableRow) (Alert, error) { return scanAlert(row) })
	if failure != nil {
		return Page{}, failure
	}
	page := Page{Alerts: listed}
	if len(listed) > limit {
		page.Alerts = listed[:limit]
		page.Next = encodeCursor(page.Alerts[limit-1])
	}
	if page.Alerts == nil {
		page.Alerts = []Alert{}
	}
	return page, store.attachDeliveries(operation, page.Alerts)
}

func (store *Store) attachDeliveries(operation context.Context, listed []Alert) error {
	if len(listed) == 0 {
		return nil
	}
	identifiers := make([]int64, len(listed))
	byIdentifier := make(map[int64]*Alert, len(listed))
	for index := range listed {
		identifiers[index] = listed[index].ID
		byIdentifier[listed[index].ID] = &listed[index]
	}
	rows, failure := store.pool.Query(operation, `select delivery.alert_id, delivery.receiver, transition.transition, delivery.state, delivery.attempts,
			delivery.last_error, case when delivery.state = 'pending' then delivery.next_attempt_at end, delivery.delivered_at
		from alert_deliveries delivery join alert_transitions transition on transition.id = delivery.transition_id
		where delivery.alert_id = any($1) order by delivery.alert_id, delivery.transition_id, delivery.receiver`, identifiers)
	if failure != nil {
		return failure
	}
	defer rows.Close()
	for rows.Next() {
		var identifier int64
		var delivery Delivery
		if failure := rows.Scan(&identifier, &delivery.Receiver, &delivery.Transition, &delivery.State, &delivery.Attempts,
			&delivery.LastError, &delivery.NextAttemptAt, &delivery.DeliveredAt); failure != nil {
			return failure
		}
		byIdentifier[identifier].Deliveries = append(byIdentifier[identifier].Deliveries, delivery)
	}
	return rows.Err()
}

func (store *Store) withDeliveries(operation context.Context, alert Alert, failure error) (Alert, error) {
	if failure != nil {
		return alert, failure
	}
	listed := []Alert{alert}
	if failure := store.attachDeliveries(operation, listed); failure != nil {
		return alert, failure
	}
	return listed[0], nil
}

func (store *Store) Get(operation context.Context, identifier int64) (Alert, error) {
	alert, failure := scanAlert(store.pool.QueryRow(operation, `select `+alertColumns+` from alerts where id = $1`, identifier))
	return store.withDeliveries(operation, alert, failure)
}

func (store *Store) Acknowledge(operation context.Context, identifier int64, actor string, now time.Time) (Alert, error) {
	alert, failure := scanAlert(store.pool.QueryRow(operation, `update alerts set acknowledged_by = coalesce(acknowledged_by, $2), acknowledged_at = coalesce(acknowledged_at, $3)
		where id = $1 returning `+alertColumns, identifier, actor, now))
	return store.withDeliveries(operation, alert, failure)
}

func (store *Store) Resolve(operation context.Context, identifier int64, actor string, now time.Time) (Alert, error) {
	alert, failure := scanAlert(store.pool.QueryRow(operation, `update alerts set resolved_by = coalesce(resolved_by, $2), resolved_at = coalesce(resolved_at, $3)
		where id = $1 returning `+alertColumns, identifier, actor, now))
	return store.withDeliveries(operation, alert, failure)
}

func ResolveFingerprintIn(operation context.Context, target Querier, fingerprint, actor string, now time.Time) error {
	_, failure := target.Exec(operation, `update alerts set resolved_by = $2, resolved_at = $3 where fingerprint = $1 and resolved_at is null`, fingerprint, actor, now)
	return failure
}

type Counts struct {
	Open           map[Severity]int64 `json:"open"`
	Unacknowledged map[Severity]int64 `json:"unacknowledged"`
}

type kindCount struct {
	severity       Severity
	kind           string
	open           int64
	unacknowledged int64
}

func (store *Store) countByKind(operation context.Context) ([]kindCount, error) {
	rows, failure := store.pool.Query(operation, `select severity, kind, count(*), count(*) filter (where acknowledged_at is null)
		from alerts where resolved_at is null group by severity, kind`)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (kindCount, error) {
		var counted kindCount
		return counted, row.Scan(&counted.severity, &counted.kind, &counted.open, &counted.unacknowledged)
	})
}

func (store *Store) Counts(operation context.Context) (Counts, error) {
	counted, failure := store.countByKind(operation)
	if failure != nil {
		return Counts{}, failure
	}
	counts := Counts{Open: map[Severity]int64{}, Unacknowledged: map[Severity]int64{}}
	for _, entry := range counted {
		counts.Open[entry.severity] += entry.open
		if entry.unacknowledged > 0 {
			counts.Unacknowledged[entry.severity] += entry.unacknowledged
		}
	}
	return counts, nil
}

func (store *Store) exportGauge(operation context.Context) error {
	counted, failure := store.countByKind(operation)
	if failure != nil {
		return failure
	}
	current := map[[2]string]bool{}
	for _, entry := range counted {
		labels := [2]string{string(entry.severity), entry.kind}
		current[labels] = true
		openAlerts.WithLabelValues(labels[0], labels[1]).Set(float64(entry.open))
	}
	for labels := range store.exported {
		if !current[labels] {
			openAlerts.DeleteLabelValues(labels[0], labels[1])
		}
	}
	store.exported = current
	if len(store.notifier.senders) == 0 {
		return nil
	}
	rows, failure := store.pool.Query(operation, `select receiver, count(*) from alert_deliveries where state = 'pending' group by receiver`)
	if failure != nil {
		return failure
	}
	var receiver string
	var count int64
	waiting := map[string]int64{}
	if _, failure := pgx.ForEachRow(rows, []any{&receiver, &count}, func() error {
		waiting[receiver] = count
		return nil
	}); failure != nil {
		return failure
	}
	for name := range store.notifier.senders {
		pendingDeliveries.WithLabelValues(name).Set(float64(waiting[name]))
	}
	return nil
}

func (store *Store) RunMetrics(operation context.Context, interval time.Duration) {
	for {
		if failure := store.exportGauge(operation); failure != nil && operation.Err() == nil {
			store.logger.Warn("open alert counts were not refreshed", "error", failure)
		}
		select {
		case <-operation.Done():
			return
		case <-time.After(interval):
		}
	}
}
