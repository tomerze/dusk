package alerts

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
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
	KindDeniedEnrollmentsSpike    = "denied_enrollments_spike"
	KindNewNetworkBurst           = "enrollment_from_new_network_burst"
	KindCredentialQuotaReached    = "credential_quota_reached"
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
)

type Alert struct {
	ID             int64           `json:"id"`
	Time           time.Time       `json:"time"`
	LastSeenAt     time.Time       `json:"last_seen_at"`
	Occurrences    int64           `json:"occurrences"`
	Severity       Severity        `json:"severity"`
	Kind           string          `json:"kind"`
	Fingerprint    string          `json:"fingerprint"`
	Detail         json.RawMessage `json:"detail"`
	AcknowledgedBy *string         `json:"acknowledged_by"`
	AcknowledgedAt *time.Time      `json:"acknowledged_at"`
	ResolvedBy     *string         `json:"resolved_by"`
	ResolvedAt     *time.Time      `json:"resolved_at"`
}

type Raised struct {
	Severity    Severity
	Kind        string
	Fingerprint string
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
	webhookFailures = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_alert_webhook_failures_total",
		Help: "Alert webhook deliveries that failed after their retries.",
	})
)

type Store struct {
	pool       *pgxpool.Pool
	logger     *slog.Logger
	webhookURL string
	http       *http.Client
	queue      chan Alert
	exported   map[[2]string]bool
}

func NewStore(pool *pgxpool.Pool, webhookURL string, logger *slog.Logger) *Store {
	return &Store{pool: pool, logger: logger, webhookURL: webhookURL, http: &http.Client{Timeout: 10 * time.Second}, queue: make(chan Alert, 1024)}
}

const alertColumns = `id, time, last_seen_at, occurrences, severity, kind, fingerprint, detail, acknowledged_by, acknowledged_at, resolved_by, resolved_at`

func scanAlert(row pgx.Row) (Alert, error) {
	var alert Alert
	failure := row.Scan(&alert.ID, &alert.Time, &alert.LastSeenAt, &alert.Occurrences, &alert.Severity, &alert.Kind, &alert.Fingerprint,
		&alert.Detail, &alert.AcknowledgedBy, &alert.AcknowledgedAt, &alert.ResolvedBy, &alert.ResolvedAt)
	if errors.Is(failure, pgx.ErrNoRows) {
		return alert, ErrNotFound
	}
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
	var inserted bool
	alert, failure := scanAlertWithInsert(target.QueryRow(operation, `insert into alerts (time, last_seen_at, severity, kind, fingerprint, detail)
		values ($1, $1, $2, $3, $4, $5::jsonb)
		on conflict (fingerprint) where resolved_at is null do update set occurrences = alerts.occurrences + 1,
			last_seen_at = greatest(alerts.last_seen_at, excluded.last_seen_at)
		returning `+alertColumns+`, (xmax = 0)`, raised.At, raised.Severity, raised.Kind, raised.Fingerprint, detail), &inserted)
	return alert, inserted, failure
}

func scanAlertWithInsert(row pgx.Row, inserted *bool) (Alert, error) {
	var alert Alert
	failure := row.Scan(&alert.ID, &alert.Time, &alert.LastSeenAt, &alert.Occurrences, &alert.Severity, &alert.Kind, &alert.Fingerprint,
		&alert.Detail, &alert.AcknowledgedBy, &alert.AcknowledgedAt, &alert.ResolvedBy, &alert.ResolvedAt, inserted)
	return alert, failure
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

func (store *Store) Open(operation context.Context, raised Raised) (Alert, bool, error) {
	alert, inserted, failure := RaiseIn(operation, store.pool, raised)
	if failure != nil {
		return alert, false, fmt.Errorf("raise %s alert: %w", raised.Kind, failure)
	}
	if inserted {
		store.Announce(alert)
	}
	return alert, inserted, nil
}

func (store *Store) OpenOfKinds(operation context.Context, kinds []string) ([]Alert, error) {
	rows, failure := store.pool.Query(operation, `select `+alertColumns+` from alerts where resolved_at is null and kind = any($1)`, kinds)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Alert, error) { return scanAlert(row) })
}

func (store *Store) Escalate(operation context.Context, fingerprint string, detail map[string]any, at time.Time) (Alert, error) {
	encoded, failure := json.Marshal(detail)
	if failure != nil {
		return Alert{}, failure
	}
	alert, failure := scanAlert(store.pool.QueryRow(operation, `update alerts set detail = $2::jsonb, occurrences = occurrences + 1,
			last_seen_at = greatest(last_seen_at, $3)
		where fingerprint = $1 and resolved_at is null returning `+alertColumns, fingerprint, encoded, at))
	if failure != nil {
		return alert, fmt.Errorf("escalate the alert %s: %w", fingerprint, failure)
	}
	store.logger.Error("alert escalated", "alert_id", alert.ID, "severity", alert.Severity, "kind", alert.Kind, "fingerprint", alert.Fingerprint, "detail", alert.Detail)
	store.notify(alert)
	return alert, nil
}

func (store *Store) ResolveOpen(operation context.Context, fingerprint, actor string, at time.Time) (Alert, error) {
	alert, failure := scanAlert(store.pool.QueryRow(operation, `update alerts set resolved_by = $2, resolved_at = $3
		where fingerprint = $1 and resolved_at is null returning `+alertColumns, fingerprint, actor, at))
	if failure == nil {
		store.logger.Info("alert resolved", "alert_id", alert.ID, "kind", alert.Kind, "fingerprint", alert.Fingerprint, "principal", actor)
	}
	return alert, failure
}

func (store *Store) Announce(alert Alert) {
	store.logger.Error("alert raised", "alert_id", alert.ID, "severity", alert.Severity, "kind", alert.Kind, "fingerprint", alert.Fingerprint, "detail", alert.Detail)
	store.notify(alert)
}

func (store *Store) notify(alert Alert) {
	if store.webhookURL == "" || (alert.Severity != Critical && alert.Severity != High) {
		return
	}
	select {
	case store.queue <- alert:
	default:
		webhookFailures.Inc()
		store.logger.Error("alert webhook queue is full; the alert stays in the alerts table", "alert_id", alert.ID, "kind", alert.Kind)
	}
}

func (store *Store) RunWebhook(operation context.Context) {
	for {
		select {
		case <-operation.Done():
			return
		case alert := <-store.queue:
			store.deliver(operation, alert)
		}
	}
}

func (store *Store) deliver(operation context.Context, alert Alert) {
	body, _ := json.Marshal(alert)
	delay := time.Second
	for attempt := 1; attempt <= 5; attempt++ {
		request, failure := http.NewRequestWithContext(operation, http.MethodPost, store.webhookURL, bytes.NewReader(body))
		if failure != nil {
			store.logger.Error("alert webhook request", "error", failure)
			return
		}
		request.Header.Set("Content-Type", "application/json")
		response, failure := store.http.Do(request)
		if failure == nil {
			response.Body.Close()
			if response.StatusCode < 300 {
				return
			}
			failure = fmt.Errorf("status %d", response.StatusCode)
		}
		store.logger.Warn("alert webhook delivery failed", "alert_id", alert.ID, "attempt", attempt, "error", failure)
		select {
		case <-operation.Done():
			return
		case <-time.After(time.Duration(rand.Int64N(int64(delay)) + 1)):
		}
		delay = min(2*delay, time.Minute)
	}
	webhookFailures.Inc()
	store.logger.Error("alert webhook gave up", "alert_id", alert.ID, "kind", alert.Kind)
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
	return page, nil
}

func (store *Store) Acknowledge(operation context.Context, identifier int64, actor string, now time.Time) (Alert, error) {
	return scanAlert(store.pool.QueryRow(operation, `update alerts set acknowledged_by = coalesce(acknowledged_by, $2), acknowledged_at = coalesce(acknowledged_at, $3)
		where id = $1 returning `+alertColumns, identifier, actor, now))
}

func (store *Store) Resolve(operation context.Context, identifier int64, actor string, now time.Time) (Alert, error) {
	return scanAlert(store.pool.QueryRow(operation, `update alerts set resolved_by = coalesce(resolved_by, $2), resolved_at = coalesce(resolved_at, $3)
		where id = $1 returning `+alertColumns, identifier, actor, now))
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
