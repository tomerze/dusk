package engine

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

var resultsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_process_results_total",
	Help: "dusk.process-results messages applied, by action kind and status.",
}, []string{"action_kind", "status"})

func (engine *Engine) runResults(operation context.Context, term int64) {
	kafka.ConsumeGroup(operation, engine.KafkaOptions, "results", engine.Config.Kafka.ResultsGroup, engine.Config.Kafka.Topics.ProcessResults, engine.Logger.With("term", term), func(operation context.Context, record *kgo.Record) error {
		if record.Value == nil {
			return nil
		}
		result, valid := kafka.Decode[kafka.ProcessResult](engine.Validator, kafka.ContractProcessResults, record, engine.Logger)
		if !valid {
			return nil
		}
		pid, failure := campaign.ParsePid(result.Pid)
		if failure != nil || pid == 0 {
			kafka.CountInvalid(record.Topic)
			engine.Logger.Warn("dropped a process result whose pid names no process", "topic", record.Topic, "partition", record.Partition, "offset", record.Offset, "pid", result.Pid)
			return nil
		}
		return engine.applyResult(operation, term, pid, result)
	})
}

func reportOf(reported *kafka.Reported) inventory.Reported {
	report := inventory.Reported{VersionKey: reported.VersionKey, Version: reported.Version, ConfigHash: reported.ConfigHash, Services: reported.Services}
	if reported.Facts != nil {
		report.Facts = reported.Facts
	}
	return report
}

func satisfiedBy(target *campaign.Campaign, reported *kafka.Reported) *bool {
	if reported == nil {
		return nil
	}
	var verdict bool
	switch target.Kind {
	case campaign.KindEnsureVersion:
		if reported.VersionKey == nil || reported.Version == nil || *reported.VersionKey != target.Action.VersionKey {
			return nil
		}
		verdict = *reported.Version == target.Action.Version
	case campaign.KindEnsureConfig:
		if reported.ConfigHash == nil {
			return nil
		}
		verdict = *reported.ConfigHash == target.Action.ConfigHash
	default:
		return nil
	}
	return &verdict
}

func (engine *Engine) campaignFor(operation context.Context, identifier uuid.UUID) (*campaign.Campaign, error) {
	if tracked := engine.registry.campaignOf(identifier); tracked != nil {
		return tracked, nil
	}
	return engine.Campaigns.Get(operation, identifier)
}

const resultRaces = 20

func (engine *Engine) applyResult(operation context.Context, term int64, pid campaign.Pid, result kafka.ProcessResult) error {
	key := NodeKey{DeviceID: result.DeviceID, InstallationID: result.InstallationID}
	at, failure := kafka.ParseTime(result.Time)
	if failure != nil {
		at = time.Now()
	}
	if result.Status == string(campaign.ResultReaped) {
		if failure := engine.applyReaped(operation, key, pid, at); failure != nil {
			return fmt.Errorf("record pid %s reaped: %w", pid, failure)
		}
		resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
		return nil
	}
	if result.ActionKind == "reap" {
		resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
		engine.Logger.Warn("dawn did not reap a pid; it is asked again on a later sweep", "device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String(),
			"status", result.Status, "error", result.Error)
		return nil
	}
	if result.Reported != nil {
		failure := engine.Inventory.ApplyReport(operation, key, result.NamespaceID, at, reportOf(result.Reported))
		switch {
		case errors.Is(failure, inventory.ErrNotFound):
			engine.Logger.Warn("a process result reports the state of a node inventory does not hold", "device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String())
		case failure != nil:
			return fmt.Errorf("store the reported state of pid %s: %w", pid, failure)
		default:
			defer engine.hint(key)
		}
	}
	if result.CampaignID == nil {
		resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
		return nil
	}
	identifier, failure := uuid.Parse(*result.CampaignID)
	if failure != nil {
		engine.Logger.Warn("a process result names a malformed campaign id", "pid", pid.String(), "campaign_id", *result.CampaignID)
		return nil
	}
	target, failure := engine.campaignFor(operation, identifier)
	if errors.Is(failure, campaign.ErrNotFound) || (target != nil && target.Status == campaign.StatusArchived) {
		engine.Logger.Info("a result for an archived or unknown campaign was dropped", "campaign_id", identifier, "pid", pid.String(), "status", result.Status)
		return nil
	}
	if failure != nil {
		return fmt.Errorf("read the campaign of pid %s: %w", pid, failure)
	}
	errorText := ""
	if result.Error != nil {
		errorText = *result.Error
	}
	if result.ActionKind != string(target.Kind) {
		resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
		engine.Logger.Info("a step of a campaign's work finished; only the work's own result moves its row", "campaign_id", identifier, "pid", pid.String(),
			"device_id", key.DeviceID, "installation_id", key.InstallationID, "action_kind", result.ActionKind, "status", result.Status, "error", errorText)
		return nil
	}
	applied := campaign.Result{Pid: pid, Status: campaign.ResultStatus(result.Status), Delivered: result.Delivered, Error: errorText, Satisfied: satisfiedBy(target, result.Reported)}
	for range resultRaces {
		row, failure := engine.Campaigns.Row(operation, identifier, key.DeviceID, key.InstallationID)
		if errors.Is(failure, campaign.ErrRowNotFound) {
			engine.Logger.Info("a process result names a node with no row in its campaign", "campaign_id", identifier, "pid", pid.String(), "device_id", key.DeviceID, "installation_id", key.InstallationID)
			return nil
		}
		if failure != nil {
			return fmt.Errorf("read the row of pid %s: %w", pid, failure)
		}
		next, effect, changed := row.ApplyResult(target, applied, time.Now(), engine.randomFloat())
		if !changed {
			resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
			engine.finishInFlight(key, pid, result)
			return nil
		}
		_, written, failure := engine.write(operation, term, &row, next, "result "+result.Status)
		if failure != nil {
			return fmt.Errorf("write the row of pid %s: %w", pid, failure)
		}
		if !written {
			current, failure := engine.Campaigns.Term(operation)
			if failure != nil {
				return fmt.Errorf("read the leadership term: %w", failure)
			}
			if current != term {
				return fmt.Errorf("term %d no longer leads; term %d does", term, current)
			}
			continue
		}
		resultsTotal.WithLabelValues(result.ActionKind, result.Status).Inc()
		engine.finishInFlight(key, pid, result)
		if effect == campaign.EffectPauseForDenied {
			reason := "permission denied"
			if result.Error != nil {
				reason += ": " + *result.Error
			}
			engine.pauseForPermission(operation, target, reason, map[string]any{"pid": pid.String(), "device_id": result.DeviceID, "installation_id": result.InstallationID})
		}
		return nil
	}
	return fmt.Errorf("pid %s lost %d compare-and-set races on its row in a row", pid, resultRaces)
}

func (engine *Engine) finishInFlight(key NodeKey, pid campaign.Pid, result kafka.ProcessResult) {
	if !campaign.ResultStatus(result.Status).Final() {
		return
	}
	engine.dispatcher.complete(key, pid)
}
