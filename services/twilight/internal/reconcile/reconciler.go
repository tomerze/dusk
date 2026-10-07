package reconcile

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"math/rand/v2"
	"slices"
	"sync"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
)

var (
	entriesTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_reconcile_entries_total",
		Help: "dusk.ledger entries reconcile checked, by kind.",
	}, []string{"kind"})
	duplicatesTotal = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_reconcile_duplicate_entries_total",
		Help: "dusk.ledger entries skipped as exact duplicates of entries already verified.",
	})
	findingsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_reconcile_findings_total",
		Help: "Reconcile findings raised as alerts, by alert kind, repeats of an open alert included.",
	}, []string{"kind"})
	lagSeconds = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_reconcile_lag_seconds",
		Help: "How far behind the present the least advanced dusk.ledger partition is reconciled.",
	})
	unattributedProcesses = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_unattributed_processes_total",
		Help: "Processes clients created on nodes without naming their pid, by principal.",
	}, []string{"principal"})
	cacheEntries = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_reconcile_intended_process_cache_entries",
		Help: "Intended processes held in the reconcile lookup cache.",
	})
)

type Dependencies struct {
	Config       config.Config
	Pool         *pgxpool.Pool
	Alerts       *alerts.Store
	Validator    *kafka.Validator
	KafkaOptions []kgo.Opt
	Keys         *VerifyingKeys
	Logger       *slog.Logger
}

type Reconciler struct {
	Dependencies
	rules    Rules
	intended *intendedCache
	interval time.Duration
	mutex    sync.Mutex
	assigned map[int32]bool
	owned    map[int32]*partitionState
	chains   *Chains
}

func New(dependencies Dependencies) *Reconciler {
	settings := dependencies.Config.Reconcile
	return &Reconciler{
		Dependencies: dependencies,
		rules:        Rules{},
		intended:     newIntendedCache(settings.IntendedProcessCacheEntries),
		interval:     time.Duration(settings.CheckpointIntervalMS) * time.Millisecond,
	}
}

func sleep(operation context.Context, duration time.Duration) bool {
	select {
	case <-operation.Done():
		return false
	case <-time.After(duration):
		return true
	}
}

func (reconciler *Reconciler) Run(operation context.Context) {
	var running sync.WaitGroup
	for _, task := range []func(context.Context){reconciler.runLedger, reconciler.runResults, reconciler.runLedgerCheck, reconciler.runPrune} {
		running.Add(1)
		go func() {
			defer running.Done()
			task(operation)
		}()
	}
	running.Wait()
}

func (reconciler *Reconciler) runLedger(operation context.Context) {
	backoff := time.Second
	for operation.Err() == nil {
		started := time.Now()
		failure := reconciler.followLedger(operation)
		if operation.Err() != nil {
			return
		}
		if time.Since(started) > time.Minute {
			backoff = time.Second
		}
		reconciler.Logger.Warn("the ledger stream stopped; restarting from the stored offsets", "error", failure, "retry_in_at_most", backoff)
		if !sleep(operation, time.Duration(rand.Int64N(int64(backoff)))+1) {
			return
		}
		backoff = min(2*backoff, time.Minute)
	}
}

func (reconciler *Reconciler) reset() {
	reconciler.mutex.Lock()
	defer reconciler.mutex.Unlock()
	reconciler.assigned = map[int32]bool{}
	reconciler.owned = map[int32]*partitionState{}
	reconciler.chains = NewChains(reconciler.Keys, reconciler.interval)
}

func (reconciler *Reconciler) followLedger(operation context.Context) error {
	topic := reconciler.Config.Kafka.Topics.Ledger
	reconciler.reset()
	client, failure := kgo.NewClient(append(append([]kgo.Opt(nil), reconciler.KafkaOptions...),
		kgo.ConsumerGroup(reconciler.Config.Kafka.ReconcileGroup),
		kgo.ConsumeTopics(topic),
		kgo.FetchIsolationLevel(kgo.ReadCommitted()),
		kgo.KeepControlRecords(),
		kgo.DisableAutoCommit(),
		kgo.BlockRebalanceOnPoll(),
		kgo.FetchMaxWait(time.Second),
		kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()),
		kgo.AdjustFetchOffsetsFn(reconciler.adjustOffsets),
		kgo.OnPartitionsAssigned(reconciler.assign),
		kgo.OnPartitionsRevoked(reconciler.release),
		kgo.OnPartitionsLost(reconciler.release),
	)...)
	if failure != nil {
		return failure
	}
	defer client.CloseAllowingRebalance()
	reconciler.Logger.Info("reconcile is following the ledger", "topic", topic, "group", reconciler.Config.Kafka.ReconcileGroup)
	admin := kadm.NewClient(client)
	checked := time.Now()
	for operation.Err() == nil {
		polling, cancel := context.WithTimeout(operation, 2*time.Second)
		fetches := client.PollRecords(polling, 5000)
		cancel()
		if operation.Err() != nil {
			return nil
		}
		if failure := kafka.FetchFailure(fetches); failure != nil {
			return failure
		}
		var batchFailure error
		fetches.EachPartition(func(partition kgo.FetchTopicPartition) {
			if batchFailure == nil && len(partition.Records) > 0 {
				batchFailure = reconciler.processPartition(operation, partition.Partition, partition.Records)
			}
		})
		if batchFailure != nil {
			return batchFailure
		}
		if time.Since(checked) >= 5*time.Second {
			reconciler.markCaughtUp(operation, admin, topic)
			checked = time.Now()
		}
		client.AllowRebalance()
	}
	return nil
}

func (reconciler *Reconciler) adjustOffsets(operation context.Context, offsets map[string]map[int32]kgo.Offset) (map[string]map[int32]kgo.Offset, error) {
	for _, partitions := range offsets {
		identifiers := make([]int32, 0, len(partitions))
		for partition := range partitions {
			identifiers = append(identifiers, partition)
		}
		stored, failure := storedOffsets(operation, reconciler.Pool, identifiers)
		if failure != nil {
			reconciler.Logger.Error("the stored ledger offsets could not be read", "error", failure)
			return nil, failure
		}
		for _, partition := range identifiers {
			if offset, found := stored[partition]; found && offset >= 0 {
				partitions[partition] = kgo.NewOffset().At(offset + 1).WithEpoch(-1)
			} else {
				partitions[partition] = kgo.NewOffset().AtStart()
			}
			reconciler.Logger.Info("ledger partition assigned", "partition", partition, "stored_offset", stored[partition])
		}
	}
	return offsets, nil
}

func (reconciler *Reconciler) assign(_ context.Context, _ *kgo.Client, assigned map[string][]int32) {
	reconciler.mutex.Lock()
	defer reconciler.mutex.Unlock()
	for _, partitions := range assigned {
		for _, partition := range partitions {
			reconciler.assigned[partition] = true
		}
	}
}

func (reconciler *Reconciler) release(_ context.Context, _ *kgo.Client, revoked map[string][]int32) {
	reconciler.mutex.Lock()
	defer reconciler.mutex.Unlock()
	for _, partitions := range revoked {
		for _, partition := range partitions {
			delete(reconciler.assigned, partition)
			delete(reconciler.owned, partition)
			delete(reconciler.chains.Tails, int64(partition))
			for key := range reconciler.chains.Heads {
				if key.Partition == int64(partition) {
					delete(reconciler.chains.Heads, key)
				}
			}
			reconciler.Logger.Info("ledger partition released", "partition", partition)
		}
	}
}

func (reconciler *Reconciler) claim(operation context.Context, partition int32) (*partitionState, error) {
	if state, owned := reconciler.owned[partition]; owned {
		return state, nil
	}
	state, tail, heads, failure := loadPartition(operation, reconciler.Pool, partition)
	if failure != nil {
		return nil, failure
	}
	for key, head := range heads {
		reconciler.chains.Heads[key] = head
	}
	if tail != nil {
		reconciler.chains.Tails[int64(partition)] = tail
	}
	reconciler.owned[partition] = &state
	return &state, nil
}

type batch struct {
	partition    int32
	offset       int64
	processedAt  *time.Time
	findings     []alerts.Raised
	calls        []Call
	defaultShell []Call
	processes    map[campaign.Pid]*processActivity
}

func (reconciler *Reconciler) processPartition(operation context.Context, partition int32, records []*kgo.Record) error {
	reconciler.mutex.Lock()
	defer reconciler.mutex.Unlock()
	state, failure := reconciler.claim(operation, partition)
	if failure != nil {
		return failure
	}
	current := &batch{partition: partition, offset: state.offset, processedAt: state.processedAt, processes: map[campaign.Pid]*processActivity{}}
	for _, record := range records {
		if record.Offset <= state.offset {
			continue
		}
		current.offset = record.Offset
		if record.Attrs.IsControl() || record.Value == nil {
			continue
		}
		entry, valid := kafka.Decode[kafka.LedgerEntry](reconciler.Validator, kafka.ContractLedger, record, reconciler.Logger)
		if !valid {
			continue
		}
		reconciler.inspect(current, record, entry)
	}
	if current.offset == state.offset {
		return nil
	}
	if failure := reconciler.commit(operation, current, state); failure != nil {
		return failure
	}
	state.offset, state.processedAt = current.offset, current.processedAt
	return nil
}

func (reconciler *Reconciler) inspect(current *batch, record *kgo.Record, entry kafka.LedgerEntry) {
	at, failure := kafka.ParseTime(entry.Time)
	if failure != nil {
		reconciler.Logger.Warn("a ledger entry carries an unreadable time", "partition", record.Partition, "offset", record.Offset, "error", failure)
		return
	}
	entriesTotal.WithLabelValues(entry.Kind).Inc()
	current.processedAt = latest(current.processedAt, at)
	if entry.Partition != int64(record.Partition) {
		found := Break{Kind: BreakWrongPartition, Instance: entry.Instance, Partition: entry.Partition, Sequence: entry.Sequence,
			Detail: fmt.Sprintf("an entry of ledger partition %d was written to Kafka partition %d", entry.Partition, record.Partition)}
		current.findings = append(current.findings, ChainBroken(found, record.Partition, record.Offset, at))
	} else {
		computed, failure := EntryHash(record.Value)
		if failure != nil {
			reconciler.Logger.Warn("a ledger entry could not be canonicalized", "partition", record.Partition, "offset", record.Offset, "error", failure)
			return
		}
		chained, failure := ChainEntryOf(entry, computed)
		if failure != nil {
			reconciler.Logger.Warn("a ledger entry could not be read for its chain", "partition", record.Partition, "offset", record.Offset, "error", failure)
			return
		}
		breaks, duplicate := reconciler.chains.Apply(chained)
		if duplicate {
			duplicatesTotal.Inc()
			reconciler.Logger.Debug("skipped a duplicate ledger entry", "instance", entry.Instance, "sequence", entry.Sequence, "offset", record.Offset)
			return
		}
		for _, found := range breaks {
			current.findings = append(current.findings, ChainBroken(found, record.Partition, record.Offset, at))
		}
	}
	if raised := QuarantineOverride(entry, at); raised != nil {
		current.findings = append(current.findings, *raised)
	}
	call, isCall, failure := CallOf(entry, at)
	if failure != nil {
		kafka.CountInvalid(record.Topic)
		reconciler.Logger.Warn("a ledger call carries a pid that is not a u64", "partition", record.Partition, "offset", record.Offset, "pid", entry.Pid, "error", failure)
		return
	}
	if !isCall {
		return
	}
	if call.Unattributed() {
		unattributedProcesses.WithLabelValues(call.Principal).Inc()
		reconciler.Logger.Debug("a client created a process without naming its pid", "principal", call.Principal, "session_id", call.SessionID,
			"device_id", call.DeviceID, "installation_id", call.InstallationID, "partition", record.Partition, "offset", record.Offset)
	}
	if call.InDefaultShell() {
		current.defaultShell = append(current.defaultShell, call)
	}
	if !call.Tracked() {
		return
	}
	current.calls = append(current.calls, call)
	activity := current.processes[call.Pid]
	if activity == nil {
		activity = &processActivity{sessions: map[string]bool{}}
		current.processes[call.Pid] = activity
	}
	if call.CreatesProcess() {
		activity.createdAt = earliest(activity.createdAt, at)
		if call.SessionID != "" {
			activity.sessions[call.SessionID] = true
		}
	}
	if !call.Reaps() {
		activity.lastCallAt = latest(activity.lastCallAt, at)
	}
	if call.RunsCommand() {
		activity.commands++
	}
}

func (reconciler *Reconciler) cachedIntended(operation context.Context, pids []campaign.Pid) (map[campaign.Pid]*IntendedProcess, error) {
	found := map[campaign.Pid]*IntendedProcess{}
	var missing []campaign.Pid
	for _, pid := range pids {
		if intended, cached := reconciler.intended.get(pid); cached {
			found[pid] = intended
		} else {
			missing = append(missing, pid)
		}
	}
	if len(missing) > 0 {
		looked, failure := lookupIntended(operation, reconciler.Pool, missing)
		if failure != nil {
			return nil, failure
		}
		for _, pid := range missing {
			found[pid] = looked[pid]
			reconciler.intended.put(pid, looked[pid])
		}
	}
	cacheEntries.Set(float64(reconciler.intended.size()))
	return found, nil
}

func (reconciler *Reconciler) commit(operation context.Context, current *batch, state *partitionState) error {
	pids := make([]campaign.Pid, 0, len(current.processes))
	for pid := range current.processes {
		pids = append(pids, pid)
	}
	slices.Sort(pids)
	intended, failure := reconciler.cachedIntended(operation, pids)
	if failure != nil {
		return failure
	}
	var announced []alerts.Alert
	now := time.Now()
	failure = pgx.BeginFunc(operation, reconciler.Pool, func(transaction pgx.Tx) error {
		processes, failure := upsertProcesses(operation, transaction, current.processes)
		if failure != nil {
			return failure
		}
		suspects := map[campaign.Pid]bool{}
		for _, call := range current.calls {
			if reconciler.rules.AgainstIntent(call, intended[call.Pid]) != nil {
				suspects[call.Pid] = true
			}
		}
		for pid, process := range processes {
			if len(reconciler.rules.AgainstProcess(process, intended[pid], now)) > 0 {
				suspects[pid] = true
			}
		}
		if len(suspects) > 0 {
			refreshed := make([]campaign.Pid, 0, len(suspects))
			for pid := range suspects {
				refreshed = append(refreshed, pid)
			}
			fresh, failure := lookupIntended(operation, transaction, refreshed)
			if failure != nil {
				return failure
			}
			for _, pid := range refreshed {
				intended[pid] = fresh[pid]
				reconciler.intended.put(pid, fresh[pid])
			}
		}
		findings := append([]alerts.Raised(nil), current.findings...)
		for _, call := range current.calls {
			if raised := reconciler.rules.AgainstIntent(call, intended[call.Pid]); raised != nil {
				findings = append(findings, *raised)
			}
		}
		for _, pid := range pids {
			findings = append(findings, reconciler.rules.AgainstProcess(processes[pid], intended[pid], now)...)
		}
		shell, failure := reconciler.checkDefaultShell(operation, transaction, current.defaultShell)
		if failure != nil {
			return failure
		}
		findings = append(findings, shell...)
		raised, failure := raiseFindings(operation, transaction, findings)
		if failure != nil {
			return failure
		}
		announced = raised
		if failure := writeHeads(operation, transaction, reconciler.chains); failure != nil {
			return failure
		}
		return advanceOffset(operation, transaction, current.partition, state.offset, current.offset, current.processedAt, reconciler.chains.Tails[int64(current.partition)])
	})
	if failure != nil {
		return failure
	}
	reconciler.chains.ClearChanges()
	for _, alert := range announced {
		reconciler.Alerts.Announce(alert)
	}
	reconciler.Logger.Debug("ledger batch reconciled", "partition", current.partition, "offset", current.offset, "calls", len(current.calls), "processes", len(pids), "findings", len(announced))
	return nil
}

func (reconciler *Reconciler) checkDefaultShell(operation context.Context, transaction pgx.Tx, calls []Call) ([]alerts.Raised, error) {
	if len(calls) == 0 {
		return nil, nil
	}
	open, failure := openProcesses(operation, transaction, calls)
	if failure != nil {
		return nil, failure
	}
	type windowKey struct {
		deviceID, installationID string
		start                    int64
	}
	var findings []alerts.Raised
	var windows []*DefaultShellWindow
	byKey := map[windowKey]*DefaultShellWindow{}
	for _, call := range calls {
		start, found := reconciler.rules.InDefaultShell(call, open[nodeKey{call.DeviceID, call.InstallationID}])
		if found != nil {
			findings = append(findings, *found)
			continue
		}
		key := windowKey{call.DeviceID, call.InstallationID, start.UnixMicro()}
		window := byKey[key]
		if window == nil {
			window = &DefaultShellWindow{DeviceID: call.DeviceID, InstallationID: call.InstallationID, Start: start}
			byKey[key] = window
			windows = append(windows, window)
		}
		window.Commands++
		if call.Time.After(window.LastCallAt) {
			window.LastCallAt = call.Time
		}
	}
	counted, failure := countDefaultShell(operation, transaction, windows)
	if failure != nil {
		return nil, failure
	}
	for _, window := range counted {
		if found := DefaultShellShape(window); found != nil {
			findings = append(findings, *found)
		}
	}
	return findings, nil
}

func raiseFindings(operation context.Context, transaction pgx.Tx, findings []alerts.Raised) ([]alerts.Alert, error) {
	seen := map[string]bool{}
	var announced []alerts.Alert
	for _, finding := range findings {
		if seen[finding.Fingerprint] {
			continue
		}
		seen[finding.Fingerprint] = true
		findingsTotal.WithLabelValues(finding.Kind).Inc()
		alert, inserted, failure := alerts.RaiseIn(operation, transaction, finding)
		if failure != nil {
			return nil, failure
		}
		if inserted {
			announced = append(announced, alert)
		}
	}
	return announced, nil
}

func (reconciler *Reconciler) markCaughtUp(operation context.Context, admin *kadm.Client, topic string) {
	reconciler.mutex.Lock()
	owned := map[int32]int64{}
	for partition := range reconciler.assigned {
		state, failure := reconciler.claim(operation, partition)
		if failure != nil {
			reconciler.mutex.Unlock()
			reconciler.Logger.Warn("an assigned ledger partition could not be loaded", "partition", partition, "error", failure)
			return
		}
		owned[partition] = state.offset
	}
	reconciler.mutex.Unlock()
	listing, cancel := context.WithTimeout(operation, 10*time.Second)
	defer cancel()
	listed, failure := admin.ListCommittedOffsets(listing, topic)
	if failure != nil {
		reconciler.Logger.Warn("the ledger's stable offsets could not be listed", "error", failure)
		return
	}
	now := time.Now()
	listed.Each(func(offset kadm.ListedOffset) {
		position, owns := owned[offset.Partition]
		if !owns || offset.Err != nil || position+1 < offset.Offset {
			return
		}
		updated, failure := markProcessed(listing, reconciler.Pool, offset.Partition, position, now)
		if failure != nil {
			reconciler.Logger.Warn("the reconciled time of a ledger partition was not recorded", "partition", offset.Partition, "error", failure)
			return
		}
		if updated {
			reconciler.mutex.Lock()
			if state, still := reconciler.owned[offset.Partition]; still && state.offset == position {
				state.processedAt = latest(state.processedAt, now)
			}
			reconciler.mutex.Unlock()
		}
	})
}

func (reconciler *Reconciler) runResults(operation context.Context) {
	kafka.ConsumeGroup(operation, reconciler.KafkaOptions, "reconcile-results", reconciler.Config.Kafka.ReconcileResultsGroup, reconciler.Config.Kafka.Topics.ProcessResults, reconciler.Logger,
		func(operation context.Context, record *kgo.Record) error {
			if record.Value == nil {
				return nil
			}
			result, valid := kafka.Decode[kafka.ProcessResult](reconciler.Validator, kafka.ContractProcessResults, record, reconciler.Logger)
			if !valid || result.ActionKind == "reap" {
				return nil
			}
			failure := reconciler.applyResult(operation, result)
			if errors.Is(failure, errMalformedResult) {
				kafka.CountInvalid(record.Topic)
				reconciler.Logger.Error("a process result was dropped from reconcile because its pid or one of its times does not parse", "pid", result.Pid,
					"partition", record.Partition, "offset", record.Offset, "error", failure)
				return nil
			}
			return failure
		})
}

func (reconciler *Reconciler) applyResult(operation context.Context, result kafka.ProcessResult) error {
	var announced []alerts.Alert
	failure := pgx.BeginFunc(operation, reconciler.Pool, func(transaction pgx.Tx) error {
		process, recorded, failure := recordResult(operation, transaction, result)
		if failure != nil || !recorded || process.ResultAt == nil || process.LastCallAt == nil {
			return failure
		}
		intended, failure := lookupIntended(operation, transaction, []campaign.Pid{process.Pid})
		if failure != nil {
			return failure
		}
		var findings []alerts.Raised
		for _, finding := range reconciler.rules.AgainstProcess(process, intended[process.Pid], time.Now()) {
			if finding.Kind == alerts.KindProcessAfterResult {
				findings = append(findings, finding)
			}
		}
		announced, failure = raiseFindings(operation, transaction, findings)
		return failure
	})
	if failure != nil {
		return failure
	}
	for _, alert := range announced {
		reconciler.Alerts.Announce(alert)
	}
	return nil
}

func (reconciler *Reconciler) runLedgerCheck(operation context.Context) {
	for sleep(operation, 30*time.Second) {
		reconciler.checkLedger(operation)
	}
}

func (reconciler *Reconciler) checkLedger(operation context.Context) {
	oldest, unknown, failure := processedThrough(operation, reconciler.Pool)
	switch {
	case failure != nil:
		reconciler.Logger.Warn("the reconciled time of the ledger could not be read", "error", failure)
	case unknown || oldest == nil:
		lagSeconds.Set(math.Inf(1))
	default:
		lagSeconds.Set(max(time.Since(*oldest).Seconds(), 0))
	}
	for operation.Err() == nil {
		var announced []alerts.Alert
		var taken int
		failure := pgx.BeginFunc(operation, reconciler.Pool, func(transaction pgx.Tx) error {
			processes, failure := takeUnledgeredResults(operation, transaction, 500)
			if failure != nil || len(processes) == 0 {
				return failure
			}
			taken = len(processes)
			findings := make([]alerts.Raised, 0, len(processes))
			pids := make([]campaign.Pid, 0, len(processes))
			for _, process := range processes {
				findings = append(findings, ResultWithoutLedger(process))
				pids = append(pids, process.Pid)
			}
			if announced, failure = raiseFindings(operation, transaction, findings); failure != nil {
				return failure
			}
			return markLedgerChecked(operation, transaction, pids)
		})
		if failure != nil {
			if operation.Err() == nil {
				reconciler.Logger.Warn("delivered process results could not be checked against the ledger", "error", failure)
			}
			return
		}
		for _, alert := range announced {
			reconciler.Alerts.Announce(alert)
		}
		if taken < 500 {
			return
		}
	}
}

func (reconciler *Reconciler) runPrune(operation context.Context) {
	retention := time.Duration(reconciler.Config.Engine.IntendedProcessRetentionDays) * 24 * time.Hour
	for sleep(operation, time.Hour) {
		var removed int64
		for operation.Err() == nil {
			deleted, failure := pruneProcesses(operation, reconciler.Pool, time.Now().Add(-retention), 10000)
			if failure != nil {
				if !errors.Is(failure, context.Canceled) {
					reconciler.Logger.Warn("old reconcile process state was not pruned", "error", failure)
				}
				break
			}
			removed += deleted
			if deleted < 10000 {
				break
			}
		}
		windows, failure := pruneDefaultShell(operation, reconciler.Pool, time.Now().Add(-retention))
		if failure != nil && !errors.Is(failure, context.Canceled) {
			reconciler.Logger.Warn("old default-shell windows were not pruned", "error", failure)
		}
		if removed > 0 || windows > 0 {
			reconciler.Logger.Info("pruned reconcile process state past the intended process retention", "processes", removed, "default_shell_windows", windows)
		}
	}
}
