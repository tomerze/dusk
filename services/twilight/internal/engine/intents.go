package engine

import (
	"context"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/kafka"
)

var intentPublications = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_intended_process_publications_total",
	Help: "Records twilight wrote to dusk.intended-processes, by kind (record or tombstone) and outcome.",
}, []string{"kind", "outcome"})

const (
	intentPublishInterval = 5 * time.Second
	intentPublishBatch    = 1000
	intentPublishTimeout  = 10 * time.Second
)

func intendedProcessMessage(record campaign.IntendedRecord, now time.Time) kafka.IntendedProcess {
	var campaignID *string
	if record.CampaignID != nil {
		identifier := record.CampaignID.String()
		campaignID = &identifier
	}
	return kafka.IntendedProcess{
		Envelope:             kafka.Envelope{Schema: "dusk.intended-processes/v1", ID: kafka.NewMessageID(), Time: kafka.FormatTime(now)},
		Pid:                  record.Pid.String(),
		DeviceID:             record.DeviceID,
		InstallationID:       record.InstallationID,
		CampaignID:           campaignID,
		ActionKind:           record.ActionKind,
		Principal:            record.Principal,
		Subject:              record.Subject,
		CreatedAt:            kafka.FormatTime(record.CreatedAt),
		ExpiresAt:            kafka.FormatTime(record.ExpiresAt),
		MaxCommands:          record.MaxCommands,
		DefaultShellCommands: record.DefaultShellCommands,
	}
}

func (engine *Engine) publishIntended(operation context.Context, key NodeKey, pid campaign.Pid) error {
	publishing, cancel := context.WithTimeout(operation, intentPublishTimeout)
	defer cancel()
	intended := campaign.IntendedKey{DeviceID: key.DeviceID, InstallationID: key.InstallationID, Pid: pid}
	_, failure := engine.Campaigns.PublishIntendedProcess(publishing, intended, time.Now(), func(record campaign.IntendedRecord) error {
		kind, failure := "record", error(nil)
		if record.Live {
			failure = engine.Intended.ProduceIntendedProcess(publishing, intendedProcessMessage(record, time.Now()))
		} else {
			kind = "tombstone"
			failure = engine.Intended.ClearIntendedProcess(publishing, kafka.IntendedProcessKey(key.DeviceID, key.InstallationID, pid.String()))
		}
		outcome := "ok"
		if failure != nil {
			outcome = "error"
		}
		intentPublications.WithLabelValues(kind, outcome).Inc()
		return failure
	})
	return failure
}

func (engine *Engine) runIntentPublication(operation context.Context, term int64) {
	for sleep(operation, intentPublishInterval) {
		engine.publishPending(operation, term)
	}
}

func (engine *Engine) publishPending(operation context.Context, term int64) int {
	keys, failure := engine.Campaigns.IntendedToPublish(operation, time.Now(), intentPublishBatch)
	if failure != nil {
		if operation.Err() == nil {
			engine.Logger.Warn("the intended processes to publish were not read; they are read again on the next pass", "term", term, "error", failure)
		}
		return 0
	}
	published := 0
	for _, intended := range keys {
		key := NodeKey{DeviceID: intended.DeviceID, InstallationID: intended.InstallationID}
		if failure := engine.publishIntended(operation, key, intended.Pid); failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("an intended process was not published; it is published again on the next pass",
					"device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", intended.Pid.String(), "error", failure)
			}
			break
		}
		published++
	}
	if published > 0 {
		engine.Logger.Debug("intended processes published by the sweep", "term", term, "published", published, "found", len(keys))
	}
	return published
}
