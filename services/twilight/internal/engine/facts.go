package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"slices"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/inventory"
)

var factsRefreshes = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_facts_refresh_total",
	Help: "Facts refreshes asked of dawn, by outcome.",
}, []string{"outcome"})

type factsQueue struct {
	*keyQueue
}

func newFactsQueue(capacity int) *factsQueue {
	return &factsQueue{newKeyQueue(capacity)}
}

type attemptLog struct {
	mutex    sync.Mutex
	entries  map[NodeKey]attempt
	capacity int
}

type attempt struct {
	namespace string
	at        time.Time
}

func newAttemptLog(capacity int) *attemptLog {
	return &attemptLog{entries: map[NodeKey]attempt{}, capacity: capacity}
}

func (log *attemptLog) recent(key NodeKey, namespace string, within time.Duration, now time.Time) bool {
	log.mutex.Lock()
	defer log.mutex.Unlock()
	entry, found := log.entries[key]
	return found && entry.namespace == namespace && now.Sub(entry.at) < within
}

func (log *attemptLog) record(key NodeKey, namespace string, now time.Time) {
	log.mutex.Lock()
	defer log.mutex.Unlock()
	if len(log.entries) >= log.capacity {
		log.entries = map[NodeKey]attempt{}
	}
	log.entries[key] = attempt{namespace: namespace, at: now}
}

func (engine *Engine) needsFacts(node *inventory.Node, session Session, now time.Time) bool {
	maximumAge := time.Duration(engine.Config.Engine.FactsMaxAgeSeconds) * time.Second
	stale := node.FactsNamespaceID == nil || *node.FactsNamespaceID != session.NamespaceID || node.FactsReadAt == nil || now.Sub(*node.FactsReadAt) > maximumAge
	return stale && !engine.factsTried.recent(node.Key(), session.NamespaceID, time.Hour, now)
}

func (engine *Engine) runFacts(operation context.Context, term int64) {
	bucket := NewTokenBucket(float64(engine.Config.Engine.FactsPerSecond), engine.Config.Engine.FactsPerSecond, time.Now())
	var bucketMutex sync.Mutex
	var running sync.WaitGroup
	defer running.Wait()
	for range engine.Config.Engine.FactsWorkers {
		running.Add(1)
		go func() {
			defer running.Done()
			for operation.Err() == nil {
				keys := engine.facts.take(1)
				if len(keys) == 0 {
					select {
					case <-operation.Done():
						return
					case <-engine.facts.notify:
					case <-time.After(time.Second):
					}
					continue
				}
				for {
					bucketMutex.Lock()
					taken := bucket.Take(time.Now())
					wait := bucket.Wait(time.Now())
					bucketMutex.Unlock()
					if taken || !sleep(operation, max(wait, 5*time.Millisecond)) {
						break
					}
				}
				engine.refreshFacts(operation, keys[0])
			}
		}()
	}
	running.Wait()
}

func (engine *Engine) refreshFacts(operation context.Context, key NodeKey) {
	defer engine.hint(key)
	session, online := engine.view.Target(key)
	if !online {
		return
	}
	now := time.Now()
	engine.factsTried.record(key, session.NamespaceID, now)
	versionKeys, failure := engine.factsVersionKeys(operation, key)
	if failure != nil {
		factsRefreshes.WithLabelValues("error").Inc()
		engine.Logger.Warn("the version keys to read were not found; the facts are read on a later refresh", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		return
	}
	pid, failure := engine.recordRandomProcess(operation, key, "collect_facts", engine.Config.Instance, "twilight",
		now.Add(time.Duration(engine.Config.Engine.ProcessLifetimeSeconds)*time.Second), factsCommands(versionKeys), 0, now)
	if failure != nil {
		factsRefreshes.WithLabelValues("error").Inc()
		engine.Logger.Warn("the facts process was not recorded", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		return
	}
	calling, cancel := context.WithTimeout(operation, time.Duration(engine.Config.Dawn.RequestTimeoutSeconds)*time.Second)
	facts, failure := engine.Dawn.Facts(calling, nodeReference(key, session), pid, versionKeys)
	cancel()
	if failure != nil {
		factsRefreshes.WithLabelValues(dawn.OutcomeOf(failure).String()).Inc()
		engine.Logger.Warn("facts refresh failed", "device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String(), "error", failure)
		return
	}
	report := inventory.Reported{Facts: facts.Facts}
	if facts.Facts == nil {
		report.Facts = map[string]json.RawMessage{}
	}
	if facts.Reported != nil {
		report.VersionKey, report.Version, report.ConfigHash, report.Services = facts.Reported.VersionKey, facts.Reported.Version, facts.Reported.ConfigHash, facts.Reported.Services
	}
	if failure := engine.Inventory.ApplyReport(operation, key, session.NamespaceID, now, report); failure != nil {
		factsRefreshes.WithLabelValues("error").Inc()
		engine.Logger.Warn("refreshed facts were not stored", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		return
	}
	factsRefreshes.WithLabelValues("accepted").Inc()
	engine.Logger.Info("facts refreshed", "device_id", key.DeviceID, "installation_id", key.InstallationID, "namespace_id", session.NamespaceID, "pid", pid.String())
}

func (engine *Engine) factsVersionKeys(operation context.Context, key NodeKey) ([]string, error) {
	keys := map[uuid.UUID]string{}
	var identifiers []uuid.UUID
	for _, target := range engine.registry.targets() {
		if target.Campaign.Kind == campaign.KindEnsureVersion {
			keys[target.Campaign.ID] = target.Campaign.Action.VersionKey
			identifiers = append(identifiers, target.Campaign.ID)
		}
	}
	rows, failure := engine.Campaigns.RowsForNodes(operation, identifiers, []string{key.DeviceID}, []string{key.InstallationID})
	if failure != nil {
		return nil, failure
	}
	return versionKeys(keys, rows), nil
}

func versionKeys(keys map[uuid.UUID]string, rows []campaign.Row) []string {
	found := []string{}
	for _, row := range rows {
		if versionKey, tracked := keys[row.CampaignID]; tracked && !row.State.Terminal() && !slices.Contains(found, versionKey) {
			found = append(found, versionKey)
		}
	}
	slices.Sort(found)
	return found
}

func factsCommands(versionKeys []string) int {
	read := []string{inventory.ConfigHashFact}
	for _, versionKey := range versionKeys {
		if !slices.Contains(read, versionKey) {
			read = append(read, versionKey)
		}
	}
	return 1 + len(read)
}

func (engine *Engine) recordRandomProcess(operation context.Context, key NodeKey, actionKind, principal, subject string, expires time.Time, maxCommands, reapedPids int, now time.Time) (campaign.Pid, error) {
	pid, failure := campaign.RandomPid()
	if failure != nil {
		return 0, failure
	}
	intended := campaign.IntendedProcess{Pid: pid, DeviceID: key.DeviceID, InstallationID: key.InstallationID, ActionKind: actionKind,
		Principal: principal, Subject: subject, ExpiresAt: expires, MaxCommands: maxCommands,
		DefaultShellCommands: engine.Config.Reconcile.DefaultShell.Commands(actionKind, reapedPids), At: now}
	if failure := engine.Campaigns.RecordIntendedProcess(operation, intended); failure != nil {
		return 0, fmt.Errorf("record the intended %s process: %w", actionKind, failure)
	}
	if failure := engine.publishIntended(operation, key, pid); failure != nil {
		return 0, fmt.Errorf("publish the intended %s process: %w", actionKind, failure)
	}
	return pid, nil
}
