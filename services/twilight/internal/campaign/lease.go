package campaign

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"

	"dusk/services/twilight/internal/database"
)

var ErrNoLease = errors.New("the partition has no row in node_partition_leases")

type Lease struct {
	Partition int32
	Epoch     int64
}

const leaseGuard = `exists (select 1 from node_partition_leases where partition = $%d and epoch = $%d)`

func underSliceLock(operation context.Context, target querier, partition int32, sql string, arguments ...any) (pgconn.CommandTag, error) {
	batch := &pgx.Batch{}
	batch.Queue(`select pg_advisory_xact_lock_shared($1)`, database.SliceLock(partition))
	batch.Queue(sql, arguments...)
	results := target.SendBatch(operation, batch)
	_, locking := results.Exec()
	tag, writing := results.Exec()
	return tag, errors.Join(locking, writing, results.Close())
}

func (store *Store) BumpLease(operation context.Context, partition int32, owner string) (Lease, error) {
	lease := Lease{Partition: partition}
	connection, failure := store.pool.Acquire(operation)
	if failure != nil {
		return lease, failure
	}
	defer connection.Release()
	running, stop := context.WithTimeout(context.WithoutCancel(operation), time.Minute)
	defer stop()
	finished := make(chan struct{})
	watched := make(chan error, 1)
	go func() {
		select {
		case <-operation.Done():
			watched <- errors.Join(operation.Err(), connection.Conn().PgConn().CancelRequest(running))
		case <-finished:
			watched <- nil
		}
	}()
	batch := &pgx.Batch{}
	batch.Queue(`select pg_advisory_xact_lock($1)`, database.SliceLock(partition))
	batch.Queue(`update node_partition_leases set owner = $2, epoch = epoch + 1, renewed_at = now() where partition = $1 returning epoch`, partition, owner)
	results := connection.SendBatch(running, batch)
	_, locking := results.Exec()
	raising := results.QueryRow().Scan(&lease.Epoch)
	failure = errors.Join(locking, results.Close())
	close(finished)
	if cancelled := <-watched; cancelled != nil {
		closing, cancel := context.WithTimeout(context.WithoutCancel(operation), 5*time.Second)
		defer cancel()
		failure = errors.Join(failure, cancelled, connection.Conn().Close(closing))
	}
	switch {
	case errors.Is(raising, pgx.ErrNoRows) && failure == nil:
		return lease, fmt.Errorf("partition %d: %w", partition, ErrNoLease)
	case raising != nil:
		return lease, errors.Join(raising, failure)
	case failure != nil:
		return lease, fmt.Errorf("the epoch of partition %d was raised to %d while the raise was being cancelled: %w", partition, lease.Epoch, failure)
	}
	return lease, nil
}

func (store *Store) LeaseHeld(operation context.Context, lease Lease) (bool, error) {
	var held bool
	failure := store.pool.QueryRow(operation, `select exists (select 1 from node_partition_leases where partition = $1 and epoch = $2)`, lease.Partition, lease.Epoch).Scan(&held)
	return held, failure
}

func (store *Store) CurrentLease(operation context.Context, device, installation string) (Lease, error) {
	var lease Lease
	failure := store.pool.QueryRow(operation, `select partition, epoch from node_partition_leases
		where partition = node_partition($1, $2, (select count(*)::integer from node_partition_leases))`, device, installation).Scan(&lease.Partition, &lease.Epoch)
	if errors.Is(failure, pgx.ErrNoRows) {
		return lease, ErrNoLease
	}
	return lease, failure
}

func (store *Store) InFlightRows(operation context.Context, partition int32, now time.Time) ([]Row, error) {
	rows, failure := store.pool.Query(operation, `select `+rowColumns+` from campaign_nodes
		where partition = $1 and state in ('dispatching', 'dispatched', 'delivered') and deadline_at > $2`, partition, now)
	if failure != nil {
		return nil, failure
	}
	return collectRows(rows)
}
