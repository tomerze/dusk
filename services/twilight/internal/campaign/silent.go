package campaign

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"

	"dusk/services/twilight/internal/database"
)

const heldLeaseGuard = `exists (select 1 from node_partition_leases lease, unnest($%d::smallint[], $%d::bigint[]) as held (partition, epoch)
	where lease.partition = held.partition and lease.epoch = held.epoch and held.partition = c.partition)`

func underSliceLocks(operation context.Context, target querier, leases []Lease, sql string, arguments ...any) (pgconn.CommandTag, error) {
	sorted := slices.SortedFunc(slices.Values(leases), func(left, right Lease) int { return int(left.Partition - right.Partition) })
	batch := &pgx.Batch{}
	for _, lease := range sorted {
		batch.Queue(`select pg_advisory_xact_lock_shared($1)`, database.SliceLock(lease.Partition))
	}
	batch.Queue(sql, arguments...)
	results := target.SendBatch(operation, batch)
	failures := make([]error, 0, len(sorted)+2)
	for range sorted {
		_, locking := results.Exec()
		failures = append(failures, locking)
	}
	tag, writing := results.Exec()
	failures = append(failures, writing, results.Close())
	return tag, errors.Join(failures...)
}

func leaseColumns(leases []Lease) ([]int32, []int64) {
	partitions := make([]int32, len(leases))
	epochs := make([]int64, len(leases))
	for index, lease := range leases {
		partitions[index], epochs[index] = lease.Partition, lease.Epoch
	}
	return partitions, epochs
}

func (store *Store) RecordGone(operation context.Context, leases []Lease, campaigns []uuid.UUID, devices, installations []string, times []time.Time, epochs []int64) error {
	if len(leases) == 0 || len(devices) == 0 {
		return nil
	}
	partitions, leaseEpochs := leaseColumns(leases)
	_, failure := underSliceLocks(operation, store.pool, leases, `update campaign_nodes c set event_at = gone.at, revision = c.revision + 1, updated_at = now()
		from unnest($1::text[], $2::text[], $3::timestamptz[], $4::bigint[]) as gone(device_id, installation_id, at, epoch)
		where c.campaign_id = any($5) and c.device_id = gone.device_id and c.installation_id = gone.installation_id
		and c.state = 'succeeded' and c.event_at is null and coalesce(c.epoch, 0) <= gone.epoch and c.finished_at <= gone.at
		and `+fmt.Sprintf(heldLeaseGuard, 6, 7), devices, installations, times, epochs, campaigns, partitions, leaseEpochs)
	return failure
}

func (store *Store) RecordBack(operation context.Context, leases []Lease, campaigns []uuid.UUID, devices, installations []string, times []time.Time) error {
	if len(leases) == 0 || len(devices) == 0 {
		return nil
	}
	partitions, leaseEpochs := leaseColumns(leases)
	_, failure := underSliceLocks(operation, store.pool, leases, `update campaign_nodes c set back_at = back.at,
		silent = extract(epoch from back.at - c.event_at) > coalesce((k.policy->'gates'->>'silent_window_seconds')::double precision, 0),
		revision = c.revision + 1, updated_at = now()
		from unnest($1::text[], $2::text[], $3::timestamptz[]) as back(device_id, installation_id, at), campaigns k
		where k.id = c.campaign_id and c.campaign_id = any($4) and c.device_id = back.device_id and c.installation_id = back.installation_id
		and c.state = 'succeeded' and c.event_at is not null and c.back_at is null and back.at >= c.event_at
		and `+fmt.Sprintf(heldLeaseGuard, 5, 6), devices, installations, times, campaigns, partitions, leaseEpochs)
	return failure
}

func (store *Store) MarkLapsed(operation context.Context, leases []Lease, identifier uuid.UUID, before time.Time) error {
	if len(leases) == 0 {
		return nil
	}
	partitions, leaseEpochs := leaseColumns(leases)
	_, failure := underSliceLocks(operation, store.pool, leases, `update campaign_nodes c set silent = true, revision = c.revision + 1, updated_at = now()
		where c.campaign_id = $1 and c.partition = any($3) and c.state = 'succeeded' and c.event_at is not null and c.back_at is null and c.silent is null and c.event_at < $2
		and `+fmt.Sprintf(heldLeaseGuard, 3, 4), identifier, before, partitions, leaseEpochs)
	return failure
}
