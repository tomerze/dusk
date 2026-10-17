package campaign

import (
	"context"
	"errors"
	"fmt"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

func (store *Store) SweepCursor(operation context.Context, identifier uuid.UUID, partition int32) ([]byte, error) {
	var cursor []byte
	failure := store.pool.QueryRow(operation, `select cursor from campaign_sweep_cursors where campaign_id = $1 and partition = $2`, identifier, partition).Scan(&cursor)
	if errors.Is(failure, pgx.ErrNoRows) {
		return []byte(`{}`), nil
	}
	return cursor, failure
}

func (store *Store) WriteSweepCursor(operation context.Context, lease Lease, identifier uuid.UUID, cursor []byte) (bool, error) {
	tag, failure := underSliceLock(operation, store.pool, lease.Partition, `insert into campaign_sweep_cursors (campaign_id, partition, cursor)
		select $1, $2, $3::jsonb where exists (select 1 from campaigns where id = $1) and `+fmt.Sprintf(leaseGuard, 2, 4)+`
		on conflict (campaign_id, partition) do update set cursor = excluded.cursor, updated_at = now()`, identifier, lease.Partition, cursor, lease.Epoch)
	return tag.RowsAffected() == 1, failure
}

func (store *Store) SweptPhase(operation context.Context, identifier uuid.UUID) (int, error) {
	var swept int
	failure := store.pool.QueryRow(operation, `select case when count(*) filter (where cursor ->> 'swept' is not null) < (select count(*) from node_partition_leases) then -1
		else coalesce(min((cursor ->> 'swept')::integer), -1) end from campaign_sweep_cursors where campaign_id = $1`, identifier).Scan(&swept)
	return swept, failure
}
