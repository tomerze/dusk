package engine

import (
	"context"
	"time"

	"dusk/services/twilight/internal/database"
)

func (engine *Engine) runPartitionMaintenance(operation context.Context, term int64) {
	retention := time.Duration(engine.Config.Engine.IntendedProcessRetentionDays) * 24 * time.Hour
	for {
		changes, failure := database.MaintainIntendedProcessPartitions(operation, engine.Pool, time.Now(), 3, retention)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Error("intended_processes partitions were not maintained", "error", failure)
			}
		} else if len(changes.Created)+len(changes.Dropped) > 0 {
			engine.Logger.Info("intended_processes partitions maintained", "created", changes.Created, "dropped", changes.Dropped)
		}
		if !sleep(operation, time.Hour) {
			return
		}
	}
}
