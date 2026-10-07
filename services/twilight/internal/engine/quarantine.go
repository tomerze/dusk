package engine

import (
	"context"
	"errors"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

func (engine *Engine) runQuarantineFlips(operation context.Context, term int64) {
	for sleep(operation, 2*time.Second) {
		for _, target := range engine.registry.targets() {
			if target.Campaign.Kind != campaign.KindQuarantine || target.Campaign.Status != campaign.StatusRunning {
				continue
			}
			rows, failure := engine.Campaigns.RowsInState(operation, target.Campaign.ID, campaign.StateVerifying, 500)
			if failure != nil {
				if operation.Err() == nil {
					engine.Logger.Warn("reading quarantine rows failed", "campaign_id", target.Campaign.ID, "error", failure)
				}
				continue
			}
			for _, row := range rows {
				engine.flip(operation, term, target.Campaign, row)
			}
		}
	}
}

var errWithdrawnNode = errors.New("the node is revoked or retired")

func (engine *Engine) flip(operation context.Context, term int64, target *campaign.Campaign, row campaign.Row) {
	key := NodeKey{DeviceID: row.DeviceID, InstallationID: row.InstallationID}
	reason := "quarantine campaign " + target.Name
	now := time.Now()
	previous, failure := engine.Inventory.SetLifecycle(operation, key, "quarantined", reason, now, func(previous string) error {
		if Withdrawn(previous) {
			return errWithdrawnNode
		}
		producing, cancel := context.WithTimeout(operation, 10*time.Second)
		defer cancel()
		return engine.NodeState.ProduceNodeState(producing, kafka.NewInstallationState(key.DeviceID, key.InstallationID, "quarantined", &reason, target.Subject(), now))
	})
	switch {
	case errors.Is(failure, errWithdrawnNode):
		engine.Logger.Info("a quarantine campaign left a withdrawn node as it is", "campaign_id", target.ID, "device_id", key.DeviceID, "installation_id", key.InstallationID, "lifecycle", previous)
		if next, changed := row.Withdrawn(now, previous); changed {
			engine.write(operation, term, &row, next, "node withdrawn")
		}
		return
	case errors.Is(failure, inventory.ErrNotFound):
		engine.Logger.Warn("a quarantine row names a node inventory does not hold; it is retried", "campaign_id", target.ID, "device_id", key.DeviceID, "installation_id", key.InstallationID)
		return
	case failure != nil:
		if operation.Err() == nil {
			engine.Logger.Warn("the quarantine was not recorded; the flip will be retried", "campaign_id", target.ID, "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		}
		return
	}
	engine.Logger.Info("node quarantined", "campaign_id", target.ID, "device_id", key.DeviceID, "installation_id", key.InstallationID, "from", previous)
	if next, changed := row.Flipped(now); changed {
		engine.write(operation, term, &row, next, "node-state acknowledged")
	}
}
