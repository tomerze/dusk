package engine

import (
	"context"
	"time"

	"dusk/services/twilight/internal/alerts"
)

func (engine *Engine) runRevocationCheck(operation context.Context, term int64) {
	grace := 2 * time.Duration(engine.Config.Engine.CensusIntervalSeconds) * time.Second
	for sleep(operation, time.Minute) {
		withdrawn, failure := engine.Inventory.Withdrawn(operation)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("reading revoked nodes failed", "error", failure)
			}
			continue
		}
		now := time.Now()
		for _, node := range withdrawn {
			if now.Sub(node.ChangedAt) < grace {
				continue
			}
			sessions := engine.view.Sessions(node.Key)
			if len(sessions) == 0 {
				continue
			}
			if _, failure := engine.Alerts.Raise(operation, alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindRevocationNotEnforced,
				Fingerprint: "revocation_not_enforced:" + node.Key.String(),
				Detail: map[string]any{"device_id": node.DeviceID, "installation_id": node.InstallationID, "lifecycle": node.Lifecycle,
					"changed_at": node.ChangedAt.UTC().Format(time.RFC3339), "instance": sessions[0].Instance, "namespace_id": sessions[0].NamespaceID}}); failure != nil {
				engine.Logger.Warn("the revocation alert was not raised", "device_id", node.DeviceID, "installation_id", node.InstallationID, "error", failure)
			}
		}
	}
}
