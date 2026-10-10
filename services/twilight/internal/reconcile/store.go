package reconcile

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/kafka"
)

var (
	errOwnershipLost   = errors.New("another reconciler advanced this ledger partition")
	errMalformedResult = errors.New("malformed process result")
)

type querier interface {
	Exec(operation context.Context, sql string, arguments ...any) (pgconn.CommandTag, error)
	Query(operation context.Context, sql string, arguments ...any) (pgx.Rows, error)
	QueryRow(operation context.Context, sql string, arguments ...any) pgx.Row
	SendBatch(operation context.Context, batch *pgx.Batch) pgx.BatchResults
}

type partitionState struct {
	offset      int64
	processedAt *time.Time
}

func storedOffsets(operation context.Context, target querier, partitions []int32) (map[int32]int64, error) {
	if _, failure := target.Exec(operation, `insert into reconcile_offsets (partition) select unnest($1::integer[]) on conflict (partition) do nothing`, partitions); failure != nil {
		return nil, fmt.Errorf("record ledger partitions: %w", failure)
	}
	rows, failure := target.Query(operation, `select partition, "offset" from reconcile_offsets where partition = any($1)`, partitions)
	if failure != nil {
		return nil, fmt.Errorf("read ledger offsets: %w", failure)
	}
	offsets := map[int32]int64{}
	for rows.Next() {
		var partition int32
		var offset int64
		if failure := rows.Scan(&partition, &offset); failure != nil {
			rows.Close()
			return nil, failure
		}
		offsets[partition] = offset
	}
	return offsets, rows.Err()
}

func loadPartition(operation context.Context, target querier, partition int32) (partitionState, *PartitionTail, map[ChainKey]*ChainHead, error) {
	if _, failure := storedOffsets(operation, target, []int32{partition}); failure != nil {
		return partitionState{}, nil, nil, failure
	}
	var state partitionState
	var tailInstance, tailHash *string
	var tailSequence *int64
	if failure := target.QueryRow(operation, `select "offset", processed_at, tail_instance, tail_sequence, tail_hash from reconcile_offsets where partition = $1`, partition).
		Scan(&state.offset, &state.processedAt, &tailInstance, &tailSequence, &tailHash); failure != nil {
		return partitionState{}, nil, nil, fmt.Errorf("read ledger partition %d: %w", partition, failure)
	}
	var tail *PartitionTail
	if tailInstance != nil && tailSequence != nil && tailHash != nil {
		tail = &PartitionTail{Instance: *tailInstance, Sequence: *tailSequence, Hash: *tailHash}
	}
	rows, failure := target.Query(operation, `select instance, sequence, hash, checkpoint_sequence, unsigned_since
		from ledger_chain_heads where partition = $1`, partition)
	if failure != nil {
		return partitionState{}, nil, nil, fmt.Errorf("read ledger chain heads: %w", failure)
	}
	defer rows.Close()
	heads := map[ChainKey]*ChainHead{}
	for rows.Next() {
		var instance string
		head := &ChainHead{}
		if failure := rows.Scan(&instance, &head.Sequence, &head.Hash, &head.CheckpointSequence, &head.UnsignedSince); failure != nil {
			return partitionState{}, nil, nil, failure
		}
		heads[ChainKey{Instance: instance, Partition: int64(partition)}] = head
	}
	return state, tail, heads, rows.Err()
}

const intendedQuery = `select intended.pid::text, intended.campaign_id::text, intended.device_id, intended.installation_id, intended.action_kind,
	intended.expires_at, intended.last_dispatched_at, intended.max_commands
	from intended_processes intended where intended.pid = any($1::text[]::numeric[]) order by intended.created_at`

func pidTexts(pids []campaign.Pid) []string {
	texts := make([]string, len(pids))
	for index, pid := range pids {
		texts[index] = pid.String()
	}
	return texts
}

func lookupIntended(operation context.Context, target querier, pids []campaign.Pid) (map[campaign.Pid]*IntendedProcess, error) {
	found := map[campaign.Pid]*IntendedProcess{}
	if len(pids) == 0 {
		return found, nil
	}
	rows, failure := target.Query(operation, intendedQuery, pidTexts(pids))
	if failure != nil {
		return nil, fmt.Errorf("look up intended processes: %w", failure)
	}
	defer rows.Close()
	for rows.Next() {
		intended := &IntendedProcess{}
		var pid string
		if failure := rows.Scan(&pid, &intended.CampaignID, &intended.DeviceID, &intended.InstallationID, &intended.ActionKind,
			&intended.ExpiresAt, &intended.LastDispatchedAt, &intended.MaxCommands); failure != nil {
			return nil, fmt.Errorf("read an intended process: %w", failure)
		}
		if intended.Pid, failure = campaign.ParsePid(pid); failure != nil {
			return nil, fmt.Errorf("read an intended process: %w", failure)
		}
		found[intended.Pid] = intended
	}
	return found, rows.Err()
}

type processActivity struct {
	sessions   map[string]bool
	createdAt  *time.Time
	lastCallAt *time.Time
	commands   int
}

func earliest(current *time.Time, candidate time.Time) *time.Time {
	if current == nil || candidate.Before(*current) {
		return &candidate
	}
	return current
}

func latest(current *time.Time, candidate time.Time) *time.Time {
	if current == nil || candidate.After(*current) {
		return &candidate
	}
	return current
}

const processColumns = `pid::text, sessions, created_at, last_call_at, commands, delivered_at, result_status, result_at, result_source`

func scanProcess(row pgx.Row) (ProcessState, error) {
	var process ProcessState
	var pid string
	var source []byte
	if failure := row.Scan(&pid, &process.Sessions, &process.CreatedAt, &process.LastCallAt, &process.Commands, &process.DeliveredAt, &process.ResultStatus, &process.ResultAt, &source); failure != nil {
		return process, failure
	}
	parsed, failure := campaign.ParsePid(pid)
	if failure != nil {
		return process, failure
	}
	process.Pid = parsed
	if len(source) > 0 {
		if failure := json.Unmarshal(source, &process.ResultSource); failure != nil {
			return process, fmt.Errorf("result_source of pid %s: %w", pid, failure)
		}
	}
	return process, nil
}

func upsertProcesses(operation context.Context, target querier, activities map[campaign.Pid]*processActivity) (map[campaign.Pid]ProcessState, error) {
	processes := map[campaign.Pid]ProcessState{}
	if len(activities) == 0 {
		return processes, nil
	}
	pids := make([]campaign.Pid, 0, len(activities))
	for pid := range activities {
		pids = append(pids, pid)
	}
	slices.Sort(pids)
	batch := &pgx.Batch{}
	for _, pid := range pids {
		activity := activities[pid]
		sessions := make([]string, 0, len(activity.sessions))
		for session := range activity.sessions {
			sessions = append(sessions, session)
		}
		slices.Sort(sessions)
		batch.Queue(`insert into reconcile_processes as existing (pid, sessions, created_at, last_call_at, commands) values ($1::text::numeric, $2, $3, $4, $5)
			on conflict (pid) do update set
				sessions = (select coalesce((array_agg(distinct session order by session))[1:64], '{}') from unnest(existing.sessions || excluded.sessions) as session),
				created_at = least(existing.created_at, excluded.created_at),
				last_call_at = greatest(existing.last_call_at, excluded.last_call_at),
				commands = existing.commands + excluded.commands
			returning `+processColumns, pid.String(), sessions, activity.createdAt, activity.lastCallAt, activity.commands)
	}
	results := target.SendBatch(operation, batch)
	for range pids {
		process, failure := scanProcess(results.QueryRow())
		if failure != nil {
			results.Close()
			return nil, fmt.Errorf("record process activity: %w", failure)
		}
		processes[process.Pid] = process
	}
	return processes, results.Close()
}

func writeHeads(operation context.Context, target querier, chains *Chains) error {
	if len(chains.ChangedHeads) == 0 {
		return nil
	}
	batch := &pgx.Batch{}
	for key := range chains.ChangedHeads {
		head := chains.Heads[key]
		batch.Queue(`insert into ledger_chain_heads (instance, partition, sequence, hash, checkpoint_sequence, unsigned_since, updated_at)
			values ($1, $2, $3, $4, $5, $6, now())
			on conflict (instance, partition) do update set sequence = excluded.sequence, hash = excluded.hash, checkpoint_sequence = excluded.checkpoint_sequence,
				unsigned_since = excluded.unsigned_since, updated_at = now()`,
			key.Instance, key.Partition, head.Sequence, head.Hash, head.CheckpointSequence, head.UnsignedSince)
	}
	if failure := target.SendBatch(operation, batch).Close(); failure != nil {
		return fmt.Errorf("record ledger chain heads: %w", failure)
	}
	return nil
}

func advanceOffset(operation context.Context, target querier, partition int32, previous, next int64, processedAt *time.Time, tail *PartitionTail) error {
	var instance, hash *string
	var sequence *int64
	if tail != nil {
		instance, sequence, hash = &tail.Instance, &tail.Sequence, &tail.Hash
	}
	tag, failure := target.Exec(operation, `update reconcile_offsets set "offset" = $3, processed_at = greatest(processed_at, $4),
		tail_instance = coalesce($5, tail_instance), tail_sequence = coalesce($6, tail_sequence), tail_hash = coalesce($7, tail_hash), updated_at = now()
		where partition = $1 and "offset" = $2`, partition, previous, next, processedAt, instance, sequence, hash)
	if failure != nil {
		return fmt.Errorf("record the ledger offset: %w", failure)
	}
	if tag.RowsAffected() != 1 {
		return errOwnershipLost
	}
	return nil
}

func markProcessed(operation context.Context, target querier, partition int32, offset int64, at time.Time) (bool, error) {
	tag, failure := target.Exec(operation, `update reconcile_offsets set processed_at = greatest(processed_at, $3), updated_at = now() where partition = $1 and "offset" = $2`, partition, offset, at)
	return tag.RowsAffected() == 1, failure
}

func finalStatus(status string) bool {
	switch status {
	case "started", "running", "duplicate", "reaped":
		return false
	}
	return true
}

func recordResult(operation context.Context, target querier, result kafka.ProcessResult) (ProcessState, bool, error) {
	at, failure := kafka.ParseTime(result.Time)
	if failure != nil {
		return ProcessState{}, false, fmt.Errorf("%w: time %q: %w", errMalformedResult, result.Time, failure)
	}
	pid, failure := campaign.ParsePid(result.Pid)
	if failure != nil || pid == 0 {
		return ProcessState{}, false, fmt.Errorf("%w: pid %q", errMalformedResult, result.Pid)
	}
	var deliveredAt, resultAt *time.Time
	var status *string
	if result.Delivered {
		deliveredAt = &at
	}
	if finalStatus(result.Status) {
		finished := at
		if result.FinishedAt != nil {
			if parsed, failure := kafka.ParseTime(*result.FinishedAt); failure == nil {
				finished = parsed
			}
		}
		resultAt, status = &finished, &result.Status
	}
	if deliveredAt == nil && resultAt == nil {
		return ProcessState{}, false, nil
	}
	source, _ := json.Marshal(map[string]any{
		"device_id": result.DeviceID, "installation_id": result.InstallationID, "namespace_id": result.NamespaceID,
		"dawn_instance": result.DawnInstance, "action_kind": result.ActionKind, "campaign_id": result.CampaignID, "status": result.Status,
	})
	process, failure := scanProcess(target.QueryRow(operation, `insert into reconcile_processes (pid, delivered_at, result_status, result_at, result_source)
		values ($1::text::numeric, $2, $3, $4, $5)
		on conflict (pid) do update set
			delivered_at = least(reconcile_processes.delivered_at, excluded.delivered_at),
			result_status = case when excluded.result_at is not null and (reconcile_processes.result_at is null or excluded.result_at >= reconcile_processes.result_at)
				then excluded.result_status else reconcile_processes.result_status end,
			result_at = greatest(reconcile_processes.result_at, excluded.result_at),
			result_source = coalesce(reconcile_processes.result_source, excluded.result_source)
		returning `+processColumns, pid.String(), deliveredAt, status, resultAt, source))
	if failure != nil {
		return ProcessState{}, false, fmt.Errorf("record a process result: %w", failure)
	}
	return process, true, nil
}

func takeUnledgeredResults(operation context.Context, target querier, limit int) ([]ProcessState, error) {
	rows, failure := target.Query(operation, `select `+processColumns+` from reconcile_processes
		where created_at is null and not ledger_checked
			and delivered_at <= (select min(coalesce(processed_at, '-infinity')) from reconcile_offsets) - make_interval(secs => $1)
		order by delivered_at limit $2 for update skip locked`, ResultWithoutLedgerAge.Seconds(), limit)
	if failure != nil {
		return nil, fmt.Errorf("read results with no ledger entry: %w", failure)
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (ProcessState, error) { return scanProcess(row) })
}

func markLedgerChecked(operation context.Context, target querier, pids []campaign.Pid) error {
	_, failure := target.Exec(operation, `update reconcile_processes set ledger_checked = true where pid = any($1::text[]::numeric[])`, pidTexts(pids))
	return failure
}

func processedThrough(operation context.Context, target querier) (*time.Time, bool, error) {
	var oldest *time.Time
	var unknown bool
	failure := target.QueryRow(operation, `select min(processed_at), coalesce(bool_or(processed_at is null), true) from reconcile_offsets`).Scan(&oldest, &unknown)
	return oldest, unknown, failure
}

type nodeKey struct {
	deviceID, installationID string
}

func openProcesses(operation context.Context, target querier, calls []Call) (map[nodeKey][]OpenProcess, error) {
	spans := map[nodeKey][2]time.Time{}
	for _, call := range calls {
		key := nodeKey{call.DeviceID, call.InstallationID}
		span, seen := spans[key]
		if !seen || call.Time.Before(span[0]) {
			span[0] = call.Time
		}
		if !seen || call.Time.After(span[1]) {
			span[1] = call.Time
		}
		spans[key] = span
	}
	var devices, installations []string
	var earliest, latest []time.Time
	for key, span := range spans {
		devices, installations = append(devices, key.deviceID), append(installations, key.installationID)
		earliest, latest = append(earliest, span[0]), append(latest, span[1])
	}
	rows, failure := target.Query(operation, `select node.device_id, node.installation_id, intended.created_at, intended.expires_at, intended.default_shell_commands
		from unnest($1::text[], $2::text[], $3::timestamptz[], $4::timestamptz[]) as node (device_id, installation_id, earliest, latest)
		join intended_processes intended on intended.device_id = node.device_id and intended.installation_id = node.installation_id
			and intended.expires_at >= node.earliest - make_interval(secs => $5) and intended.created_at <= node.latest + make_interval(secs => $5)`,
		devices, installations, earliest, latest, clockSkew.Seconds())
	if failure != nil {
		return nil, fmt.Errorf("read the open intended processes of nodes: %w", failure)
	}
	defer rows.Close()
	open := map[nodeKey][]OpenProcess{}
	for rows.Next() {
		var key nodeKey
		var process OpenProcess
		if failure := rows.Scan(&key.deviceID, &key.installationID, &process.CreatedAt, &process.ExpiresAt, &process.DefaultShellCommands); failure != nil {
			return nil, fmt.Errorf("read an open intended process: %w", failure)
		}
		open[key] = append(open[key], process)
	}
	return open, rows.Err()
}

func countDefaultShell(operation context.Context, target querier, windows []*DefaultShellWindow) ([]DefaultShellWindow, error) {
	if len(windows) == 0 {
		return nil, nil
	}
	var devices, installations []string
	var starts, lastCalls []time.Time
	var commands []int
	for _, window := range windows {
		devices, installations = append(devices, window.DeviceID), append(installations, window.InstallationID)
		starts, lastCalls, commands = append(starts, window.Start), append(lastCalls, window.LastCallAt), append(commands, window.Commands)
	}
	rows, failure := target.Query(operation, `insert into reconcile_default_shell as shell (device_id, installation_id, window_start, commands, last_call_at)
		select * from unnest($1::text[], $2::text[], $3::timestamptz[], $4::integer[], $5::timestamptz[])
		on conflict (device_id, installation_id, window_start) do update
			set commands = shell.commands + excluded.commands, last_call_at = greatest(shell.last_call_at, excluded.last_call_at)
		returning shell.device_id, shell.installation_id, shell.window_start, shell.commands, shell.last_call_at,
			(select coalesce(sum(intended.default_shell_commands), 0) from intended_processes intended
				where intended.device_id = shell.device_id and intended.installation_id = shell.installation_id
					and intended.created_at <= shell.last_call_at + make_interval(secs => $6)
					and intended.expires_at >= shell.window_start - make_interval(secs => $6))`,
		devices, installations, starts, commands, lastCalls, clockSkew.Seconds())
	if failure != nil {
		return nil, fmt.Errorf("count default-shell commands: %w", failure)
	}
	defer rows.Close()
	var counted []DefaultShellWindow
	for rows.Next() {
		var window DefaultShellWindow
		var allowed int64
		if failure := rows.Scan(&window.DeviceID, &window.InstallationID, &window.Start, &window.Commands, &window.LastCallAt, &allowed); failure != nil {
			return nil, fmt.Errorf("read a default-shell window: %w", failure)
		}
		window.Allowed = int(allowed)
		counted = append(counted, window)
	}
	return counted, rows.Err()
}

func pruneDefaultShell(operation context.Context, target querier, before time.Time) (int64, error) {
	tag, failure := target.Exec(operation, `delete from reconcile_default_shell where last_call_at < $1`, before)
	return tag.RowsAffected(), failure
}

func pruneProcesses(operation context.Context, target querier, before time.Time, limit int) (int64, error) {
	tag, failure := target.Exec(operation, `delete from reconcile_processes where pid in (select pid from reconcile_processes where first_seen_at < $1 limit $2)`, before, limit)
	return tag.RowsAffected(), failure
}
