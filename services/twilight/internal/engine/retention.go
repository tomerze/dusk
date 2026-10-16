package engine

import (
	"context"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
)

const retentionBatch = 1000

var (
	installationsPurged = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_installations_purged_total",
		Help: "Inventory rows of installations removed after retention.installation_days without being seen, by lifecycle.",
	}, []string{"lifecycle"})
	alertsPurged = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_alerts_purged_total",
		Help: "Resolved alerts removed retention.alert_days after they were resolved.",
	})
)

func (engine *Engine) runRetention(operation context.Context, term int64) {
	for {
		engine.applyRetention(operation, term, time.Now())
		if !sleep(operation, time.Hour) {
			return
		}
	}
}

func (engine *Engine) applyRetention(operation context.Context, term int64, now time.Time) {
	day := 24 * time.Hour
	inactiveBefore := now.Add(-time.Duration(engine.Config.Retention.InstallationDays) * day)
	purged := map[string]int64{}
	for operation.Err() == nil {
		removed, failure := engine.Inventory.PurgeInactive(operation, inactiveBefore, retentionBatch)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Error("inactive installations were not purged from the inventory; the next pass tries again", "term", term, "error", failure)
			}
			break
		}
		var batch int64
		for lifecycle, count := range removed {
			purged[lifecycle] += count
			installationsPurged.WithLabelValues(lifecycle).Add(float64(count))
			batch += count
		}
		if batch < retentionBatch {
			break
		}
	}
	if len(purged) > 0 {
		engine.Logger.Info("installations not seen within retention.installation_days left the inventory", "term", term,
			"inactive_before", inactiveBefore, "by_lifecycle", purged)
	}
	resolvedBefore := now.Add(-time.Duration(engine.Config.Retention.AlertDays) * day)
	var alerts int64
	for operation.Err() == nil {
		removed, failure := engine.Alerts.PurgeResolved(operation, resolvedBefore, retentionBatch)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Error("resolved alerts past retention were not purged; the next pass tries again", "term", term, "error", failure)
			}
			break
		}
		alerts += removed
		alertsPurged.Add(float64(removed))
		if removed < retentionBatch {
			break
		}
	}
	if alerts > 0 {
		engine.Logger.Info("resolved alerts older than retention.alert_days were removed", "term", term, "resolved_before", resolvedBefore, "alerts", alerts)
	}
}
