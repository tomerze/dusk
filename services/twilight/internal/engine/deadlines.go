package engine

import (
	"context"
	"time"
)

func (engine *Engine) runDeadlines(operation context.Context, term int64) {
	for sleep(operation, 5*time.Second) {
		now := time.Now()
		engine.dispatcher.expire(now)
		rows, failure := engine.Campaigns.ExpiredRows(operation, now, 1000)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("reading rows past their deadline failed", "error", failure)
			}
			continue
		}
		for _, row := range rows {
			target, failure := engine.campaignFor(operation, row.CampaignID)
			if failure != nil {
				continue
			}
			next, changed := row.Expire(target, now, target.Policy.Backoff(row.Unreached+1, engine.randomFloat()))
			if !changed {
				continue
			}
			if _, written, _ := engine.write(operation, term, &row, next, "deadline"); written {
				engine.dispatcher.complete(NodeKey{DeviceID: row.DeviceID, InstallationID: row.InstallationID}, row.Pid)
			}
		}
	}
}
