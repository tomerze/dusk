package engine

import (
	"context"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
)

var (
	reapRequests = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_reap_requests_total",
		Help: "Requests twilight sent dawn to reap pids on a node, by outcome.",
	}, []string{"outcome"})
	reapedPids = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_reaped_pids_total",
		Help: "Pids dawn reported reaped.",
	})
)

const (
	reapsPerRequest = 256
	reapBuckets     = 30
	reapKeysPerRead = 1000
)

func (engine *Engine) reapRetry() time.Duration {
	return time.Duration(engine.Config.Engine.ReapRetrySeconds) * time.Second
}

func (current *dispatcher) offerReap(key NodeKey) {
	if !current.reaps.offer(key) {
		return
	}
	select {
	case current.wake <- struct{}{}:
	default:
	}
}

func (engine *Engine) offerReaps(operation context.Context, keys []NodeKey, now time.Time) error {
	var devices, installations []string
	for _, key := range keys {
		if engine.view.Online(key) && !engine.dispatcher.inFlightFor(key) {
			devices, installations = append(devices, key.DeviceID), append(installations, key.InstallationID)
		}
	}
	found, failure := engine.Campaigns.Reapable(operation, devices, installations, now, engine.reapRetry(), 1)
	if failure != nil {
		return failure
	}
	for _, entry := range found {
		engine.dispatcher.offerReap(NodeKey{DeviceID: entry.DeviceID, InstallationID: entry.InstallationID})
	}
	return nil
}

func (engine *Engine) runReapSweep(operation context.Context, term int64) {
	interval := max(time.Duration(engine.Config.Engine.ReapIntervalSeconds)*time.Second/reapBuckets, time.Second)
	bucket := uint32(0)
	for sleep(operation, interval) {
		current := bucket
		keys, _ := engine.view.LastSeen(func(key NodeKey) bool { return bucketOf(key, reapBuckets) == current })
		for start := 0; start < len(keys) && operation.Err() == nil; start += reapKeysPerRead {
			if failure := engine.offerReaps(operation, keys[start:min(start+reapKeysPerRead, len(keys))], time.Now()); failure != nil && operation.Err() == nil {
				engine.Logger.Warn("the pids to reap were not read; their nodes are swept again on the next pass", "bucket", current, "nodes", len(keys), "error", failure)
				break
			}
		}
		bucket = (bucket + 1) % reapBuckets
	}
}

func (current *dispatcher) reap(operation context.Context, key NodeKey) {
	engine := current.engine
	defer current.release(key)
	session, online := engine.view.Target(key)
	if !online {
		return
	}
	now := time.Now()
	found, failure := engine.Campaigns.Reapable(operation, []string{key.DeviceID}, []string{key.InstallationID}, now, engine.reapRetry(), reapsPerRequest)
	if failure != nil {
		if operation.Err() == nil {
			engine.Logger.Warn("the pids to reap were not read; the node is asked again on a later sweep", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		}
		return
	}
	if len(found) == 0 {
		return
	}
	pids := make([]campaign.Pid, len(found))
	texts := make([]string, len(found))
	for index, entry := range found {
		pids[index], texts[index] = entry.Pid, entry.Pid.String()
	}
	lifetime := time.Duration(engine.Config.Engine.ProcessLifetimeSeconds) * time.Second
	if _, failure := engine.recordRandomProcess(operation, key, "reap", engine.Config.Instance, "twilight", now.Add(lifetime), 0, len(pids), now); failure != nil {
		if operation.Err() == nil {
			engine.Logger.Warn("the reap was not recorded as an intended process; the node is asked again on a later sweep", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
		}
		return
	}
	calling, cancel := context.WithTimeout(operation, time.Duration(engine.Config.Dawn.RequestTimeoutSeconds)*time.Second)
	failure = engine.Dawn.Reap(calling, nodeReference(key, session), pids)
	cancel()
	outcome := dawn.OutcomeOf(failure)
	reapRequests.WithLabelValues(outcome.String()).Inc()
	if outcome != dawn.Accepted && outcome != dawn.Ambiguous {
		if outcome == dawn.Busy {
			current.mutex.Lock()
			current.reapTokens.Return(1)
			current.mutex.Unlock()
		}
		engine.Logger.Warn("dawn did not take the pids to reap; they are asked again on a later sweep", "device_id", key.DeviceID, "installation_id", key.InstallationID,
			"pids", texts, "outcome", outcome.String(), "error", failure)
		return
	}
	if failure := engine.Campaigns.RecordReapRequested(operation, key.DeviceID, key.InstallationID, pids, now); failure != nil && operation.Err() == nil {
		engine.Logger.Warn("a reap request was not recorded; its pids may be asked again before their retry interval", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
	}
	engine.Logger.Info("asked dawn to reap pids", "device_id", key.DeviceID, "installation_id", key.InstallationID, "namespace_id", session.NamespaceID,
		"pids", texts, "outcome", outcome.String())
}

func (engine *Engine) applyReaped(operation context.Context, key NodeKey, pid campaign.Pid, at time.Time) error {
	recorded, failure := engine.Campaigns.RecordReaped(operation, key.DeviceID, key.InstallationID, pid, at)
	if failure != nil {
		return failure
	}
	if recorded {
		reapedPids.Inc()
		engine.Logger.Info("pid reaped", "device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String())
	} else {
		engine.Logger.Debug("a reap result names a pid with no unreaped intended process", "device_id", key.DeviceID, "installation_id", key.InstallationID, "pid", pid.String())
	}
	return nil
}
