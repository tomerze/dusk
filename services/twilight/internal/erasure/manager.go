package erasure

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"sort"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/clickhouse"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/objectstore"
)

const (
	tickInterval         = 10 * time.Second
	waitInterval         = 15 * time.Second
	maximumBackoff       = time.Hour
	stalledAttempts      = 5
	stalledCheckInterval = 5 * time.Minute
	retentionInterval    = 24 * time.Hour
)

var (
	erasuresCompleted = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_erasures_completed_total",
		Help: "Erasures that finished, by scope and kind (erasure or retention).",
	}, []string{"scope", "kind"})
	erasureFailures = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_erasure_step_failures_total",
		Help: "Erasure steps that failed and are retried, by step.",
	}, []string{"step"})
	erasuresRunning = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_erasures_running",
		Help: "Erasures that have not finished.",
	})
)

type Dependencies struct {
	Pool        *pgxpool.Pool
	ClickHouse  *clickhouse.Client
	ObjectStore *objectstore.Client
	Alerts      *alerts.Store
	Topics      func(operation context.Context) ([]kafka.TopicRetention, error)
	Online      func(key inventory.Key) bool
	Config      config.Config
	Logger      *slog.Logger
	Now         func() time.Time
}

type Manager struct {
	Dependencies
	scheduledAt time.Time
	checkedAt   time.Time
}

func New(dependencies Dependencies) *Manager {
	if dependencies.Now == nil {
		dependencies.Now = time.Now
	}
	if dependencies.Online == nil {
		dependencies.Online = func(inventory.Key) bool { return false }
	}
	return &Manager{Dependencies: dependencies}
}

func ValidReason(reason string) (string, error) {
	trimmed := strings.TrimSpace(reason)
	if trimmed == "" || len(trimmed) > 1000 {
		return "", ErrInvalidReason
	}
	return trimmed, nil
}

func (manager *Manager) RequestDevice(operation context.Context, device, reason, actor string) (Erasure, bool, error) {
	reason, failure := ValidReason(reason)
	if failure != nil {
		return Erasure{}, false, failure
	}
	now := manager.Now()
	var result Erasure
	var created bool
	failure = pgx.BeginFunc(operation, manager.Pool, func(transaction pgx.Tx) error {
		var deviceLifecycle *string
		if failure := transaction.QueryRow(operation, `select lifecycle from devices where device_id = $1 for share`, device).Scan(&deviceLifecycle); failure != nil && !errors.Is(failure, pgx.ErrNoRows) {
			return failure
		}
		withdrawnDevice := deviceLifecycle != nil && (*deviceLifecycle == "retired" || *deviceLifecycle == "revoked")
		rows, failure := transaction.Query(operation, `select installation_id, lifecycle from nodes where device_id = $1`, device)
		if failure != nil {
			return failure
		}
		problem := NotReadyError{Subject: "device " + device}
		for rows.Next() {
			var installation, lifecycle string
			if failure := rows.Scan(&installation, &lifecycle); failure != nil {
				rows.Close()
				return failure
			}
			if !withdrawnDevice && lifecycle != "retired" && lifecycle != "revoked" {
				problem.NotWithdrawn++
			}
			if manager.Online(inventory.Key{DeviceID: device, InstallationID: installation}) {
				problem.Online++
			}
		}
		rows.Close()
		if failure := rows.Err(); failure != nil {
			return failure
		}
		if problem.NotWithdrawn > 0 || problem.Online > 0 {
			return &problem
		}
		result, created, failure = insert(operation, transaction, Erasure{ID: uuid.Must(uuid.NewV7()), Scope: ScopeDevice, DeviceID: &device, Reason: reason,
			RequestedBy: actor, RequestedAt: now, Steps: newSteps(false)})
		return failure
	})
	if failure != nil {
		return Erasure{}, false, failure
	}
	manager.logRequest(result, created, reason, actor)
	return result, created, nil
}

func (manager *Manager) RequestTenant(operation context.Context, tenant, reason, actor string) (Erasure, bool, error) {
	reason, failure := ValidReason(reason)
	if failure != nil {
		return Erasure{}, false, failure
	}
	now := manager.Now()
	var result Erasure
	var created bool
	failure = pgx.BeginFunc(operation, manager.Pool, func(transaction pgx.Tx) error {
		problem := NotReadyError{Subject: "tenant " + tenant}
		if failure := transaction.QueryRow(operation, `select count(*) from nodes node left join devices device using (device_id)
			where node.tenant = $1 and node.lifecycle not in ('retired', 'revoked') and coalesce(device.lifecycle, 'active') not in ('retired', 'revoked')`,
			tenant).Scan(&problem.NotWithdrawn); failure != nil {
			return failure
		}
		if failure := transaction.QueryRow(operation, `select count(*) from campaigns where tenant = $1 and status in ('running', 'paused')`, tenant).
			Scan(&problem.RunningCampaigns); failure != nil {
			return failure
		}
		rows, failure := transaction.Query(operation, `select device_id, installation_id from nodes where tenant = $1`, tenant)
		if failure != nil {
			return failure
		}
		for rows.Next() {
			var key inventory.Key
			if failure := rows.Scan(&key.DeviceID, &key.InstallationID); failure != nil {
				rows.Close()
				return failure
			}
			if manager.Online(key) {
				problem.Online++
			}
		}
		rows.Close()
		if failure := rows.Err(); failure != nil {
			return failure
		}
		if problem.NotWithdrawn > 0 || problem.Online > 0 || problem.RunningCampaigns > 0 {
			return &problem
		}
		result, created, failure = insert(operation, transaction, Erasure{ID: uuid.Must(uuid.NewV7()), Scope: ScopeTenant, Tenant: &tenant, Reason: reason,
			RequestedBy: actor, RequestedAt: now, Steps: newSteps(false)})
		return failure
	})
	if failure != nil {
		return Erasure{}, false, failure
	}
	manager.logRequest(result, created, reason, actor)
	return result, created, nil
}

func (manager *Manager) logRequest(erasure Erasure, created bool, reason, actor string) {
	if created {
		manager.Logger.Info("erasure requested", "erasure_id", erasure.ID, "scope", erasure.Scope, "subject", erasure.Subject(), "principal", actor, "reason", reason)
		return
	}
	manager.Logger.Info("erasure requested while one is running; the running erasure answers it", "erasure_id", erasure.ID, "scope", erasure.Scope,
		"subject", erasure.Subject(), "principal", actor, "reason", reason)
}

func (manager *Manager) Get(operation context.Context, identifier uuid.UUID) (Erasure, error) {
	return get(operation, manager.Pool, identifier)
}

func sleep(operation context.Context, duration time.Duration) bool {
	select {
	case <-operation.Done():
		return false
	case <-time.After(duration):
		return true
	}
}

func (manager *Manager) Run(operation context.Context, term int64) {
	for sleep(operation, tickInterval) {
		manager.scheduleRetention(operation)
		erasures, failure := due(operation, manager.Pool, manager.Now(), 10)
		if failure != nil {
			if operation.Err() == nil {
				manager.Logger.Warn("the erasures that are due were not read; they are read again on the next pass", "term", term, "error", failure)
			}
			continue
		}
		for _, erasure := range erasures {
			if operation.Err() != nil {
				return
			}
			manager.advance(operation, erasure)
		}
		manager.raiseStalled(operation)
	}
}

func backoff(attempts int) time.Duration {
	ceiling := min(30*time.Second<<min(attempts, 10), maximumBackoff)
	return ceiling/2 + time.Duration(rand.Int64N(int64(ceiling)/2+1))
}

type stepFunction func(operation context.Context, erasure *Erasure, step *Step) (bool, error)

func (manager *Manager) stepFunction(name string) stepFunction {
	switch name {
	case StepScope:
		return manager.scope
	case StepClickHouse:
		return manager.deleteRows
	case StepLake:
		return manager.rewriteLake
	case StepFiles:
		return manager.deleteFiles
	case StepInventory:
		return manager.deleteInventory
	}
	return func(context.Context, *Erasure, *Step) (bool, error) {
		return false, fmt.Errorf("unknown erasure step %q", name)
	}
}

func (manager *Manager) advance(operation context.Context, erasure Erasure) {
	for index := range erasure.Steps {
		step := &erasure.Steps[index]
		if step.State == StepDone {
			continue
		}
		now := manager.Now()
		if step.StartedAt == nil {
			step.StartedAt = &now
		}
		step.State = StepRunning
		done, failure := manager.stepFunction(step.Name)(operation, &erasure, step)
		now = manager.Now()
		switch {
		case failure != nil && operation.Err() != nil:
			return
		case failure != nil:
			erasure.Attempts++
			message := failure.Error()
			erasure.LastError = &message
			erasure.NextAttemptAt = now.Add(backoff(erasure.Attempts))
			erasureFailures.WithLabelValues(step.Name).Inc()
			manager.Logger.Warn("an erasure step failed; it is tried again", "erasure_id", erasure.ID, "subject", erasure.Subject(), "step", step.Name,
				"attempts", erasure.Attempts, "next_attempt_at", erasure.NextAttemptAt, "error", failure)
		case !done:
			erasure.NextAttemptAt = now.Add(waitInterval)
		default:
			step.State = StepDone
			step.FinishedAt = &now
			step.Cursor = ""
			erasure.Attempts = 0
			erasure.LastError = nil
			manager.Logger.Info("erasure step finished", "erasure_id", erasure.ID, "subject", erasure.Subject(), "step", step.Name, "detail", step.Detail)
			if failure := save(operation, manager.Pool, erasure, now); failure != nil {
				manager.Logger.Warn("the erasure's progress was not saved; the step runs again", "erasure_id", erasure.ID, "step", step.Name, "error", failure)
				return
			}
			continue
		}
		if failure := save(operation, manager.Pool, erasure, now); failure != nil && operation.Err() == nil {
			manager.Logger.Warn("the erasure's progress was not saved", "erasure_id", erasure.ID, "step", step.Name, "error", failure)
		}
		return
	}
	manager.complete(operation, erasure)
}

func (manager *Manager) complete(operation context.Context, erasure Erasure) {
	if erasure.Before == nil {
		erasure.Retained = manager.retained(operation, erasure)
	}
	now := manager.Now()
	erasure.Status = StatusCompleted
	erasure.CompletedAt = &now
	if failure := save(operation, manager.Pool, erasure, now); failure != nil {
		if operation.Err() == nil {
			manager.Logger.Warn("the finished erasure was not saved; it finishes again on the next pass", "erasure_id", erasure.ID, "error", failure)
		}
		return
	}
	if failure := alerts.ResolveFingerprintIn(operation, manager.Pool, alerts.KindErasureStalled+":"+erasure.ID.String(), "twilight", now); failure != nil {
		manager.Logger.Warn("the finished erasure's stalled alert was not resolved", "erasure_id", erasure.ID, "error", failure)
	}
	if manager.ClickHouse != nil {
		if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: fmt.Sprintf("ALTER TABLE dusk.erasure_scope DELETE WHERE erasure_id = '%s'", erasure.ID)}); failure != nil {
			manager.Logger.Warn("the finished erasure's scope was not deleted from ClickHouse; its table's TTL removes it", "erasure_id", erasure.ID, "error", failure)
		}
	}
	kind := "erasure"
	if erasure.Before != nil {
		kind = "retention"
	}
	erasuresCompleted.WithLabelValues(string(erasure.Scope), kind).Inc()
	counts := map[string]int64{}
	for _, step := range erasure.Steps {
		for name, count := range step.Counts {
			counts[step.Name+"."+name] = count
		}
	}
	manager.Logger.Info("erasure finished", "erasure_id", erasure.ID, "scope", erasure.Scope, "subject", erasure.Subject(), "kind", kind,
		"seconds", now.Sub(erasure.RequestedAt).Seconds(), "counts", counts)
}

func (manager *Manager) raiseStalled(operation context.Context) {
	now := manager.Now()
	if now.Sub(manager.checkedAt) < stalledCheckInterval {
		return
	}
	manager.checkedAt = now
	overdue := time.Duration(manager.Config.Retention.ErasureOverdueDays) * 24 * time.Hour
	found, failure := stalledErasures(operation, manager.Pool, now.Add(-overdue), stalledAttempts)
	if failure != nil {
		if operation.Err() == nil {
			manager.Logger.Warn("stalled erasures were not checked", "error", failure)
		}
		return
	}
	var running int64
	if failure := manager.Pool.QueryRow(operation, `select count(*) from erasures where status = 'running'`).Scan(&running); failure == nil {
		erasuresRunning.Set(float64(running))
	}
	for _, current := range found {
		detail := map[string]any{"erasure_id": current.ID.String(), "scope": string(current.Scope), "subject": current.Subject,
			"requested_at": current.RequestedAt.UTC().Format(time.RFC3339), "attempts": current.Attempts,
			"message": fmt.Sprintf("an erasure of %s %s requested %s has not finished", current.Scope, current.Subject, current.RequestedAt.UTC().Format(time.RFC3339))}
		if current.LastError != nil {
			detail["last_error"] = *current.LastError
		}
		if _, failure := manager.Alerts.Raise(operation, alerts.Raised{Severity: alerts.High, Kind: alerts.KindErasureStalled,
			Fingerprint: alerts.KindErasureStalled + ":" + current.ID.String(), Detail: detail}); failure != nil {
			manager.Logger.Warn("the stalled erasure alert was not raised", "erasure_id", current.ID, "error", failure)
		}
	}
}

func (manager *Manager) scheduleRetention(operation context.Context) {
	now := manager.Now()
	if now.Sub(manager.scheduledAt) < time.Hour || len(manager.Config.Retention.TenantDays) == 0 {
		return
	}
	manager.scheduledAt = now
	tenants := make([]string, 0, len(manager.Config.Retention.TenantDays))
	for tenant := range manager.Config.Retention.TenantDays {
		tenants = append(tenants, tenant)
	}
	sort.Strings(tenants)
	for _, tenant := range tenants {
		last, failure := lastRetention(operation, manager.Pool, tenant)
		if failure != nil {
			manager.Logger.Warn("the tenant's last retention run was not read", "tenant", tenant, "error", failure)
			continue
		}
		if last != nil && now.Sub(*last) < retentionInterval {
			continue
		}
		days := manager.Config.Retention.TenantDays[tenant]
		before := now.Add(-time.Duration(days) * 24 * time.Hour)
		erasure, created, failure := insert(operation, manager.Pool, Erasure{ID: uuid.Must(uuid.NewV7()), Scope: ScopeTenant, Tenant: &tenant, Before: &before,
			Reason: fmt.Sprintf("retention of %d days for tenant %s", days, tenant), RequestedBy: "twilight", RequestedAt: now, Steps: newSteps(true)})
		switch {
		case failure != nil:
			manager.Logger.Warn("the tenant's retention run was not scheduled", "tenant", tenant, "error", failure)
		case created:
			manager.Logger.Info("tenant retention scheduled", "erasure_id", erasure.ID, "tenant", tenant, "days", days, "before", before)
		}
	}
}
