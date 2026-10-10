package database

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

var campaignPartitionedTables = []string{"campaign_nodes", "campaign_events"}

const intendedProcessPartitionPrefix = "intended_processes_"

func campaignPartitionName(table string, campaign uuid.UUID) string {
	return table + "_" + strings.ReplaceAll(campaign.String(), "-", "")
}

func CreateCampaignPartitions(operation context.Context, transaction pgx.Tx, campaign uuid.UUID) error {
	for _, table := range campaignPartitionedTables {
		statement := fmt.Sprintf("create table if not exists %s partition of %s for values in ('%s')",
			pgx.Identifier{campaignPartitionName(table, campaign)}.Sanitize(), pgx.Identifier{table}.Sanitize(), campaign.String())
		if _, failure := transaction.Exec(operation, statement); failure != nil {
			return fmt.Errorf("create %s partition for campaign %s: %w", table, campaign, failure)
		}
	}
	return nil
}

func DropCampaignPartitions(operation context.Context, pool *pgxpool.Pool, campaign uuid.UUID) error {
	return pgx.BeginFunc(operation, pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `set local lock_timeout = '5s'`); failure != nil {
			return failure
		}
		for _, table := range campaignPartitionedTables {
			statement := "drop table if exists " + pgx.Identifier{campaignPartitionName(table, campaign)}.Sanitize()
			if _, failure := transaction.Exec(operation, statement); failure != nil {
				return fmt.Errorf("drop %s partition for campaign %s: %w", table, campaign, failure)
			}
		}
		return nil
	})
}

func intendedProcessPartitionName(day time.Time) string {
	return intendedProcessPartitionPrefix + day.UTC().Format("20060102")
}

type IntendedProcessPartitionChanges struct {
	Created []string
	Dropped []string
}

func MaintainIntendedProcessPartitions(operation context.Context, pool *pgxpool.Pool, now time.Time, daysAhead int, retention time.Duration) (IntendedProcessPartitionChanges, error) {
	var changes IntendedProcessPartitionChanges
	today := now.UTC().Truncate(24 * time.Hour)
	failure := pgx.BeginFunc(operation, pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `set local lock_timeout = '5s'`); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `select pg_advisory_xact_lock($1)`, partitionLockKey); failure != nil {
			return failure
		}
		existing := map[string]bool{}
		rows, failure := transaction.Query(operation, `select child.relname from pg_inherits
			join pg_class child on child.oid = pg_inherits.inhrelid
			join pg_class parent on parent.oid = pg_inherits.inhparent
			where parent.relname = 'intended_processes'`)
		if failure != nil {
			return failure
		}
		for rows.Next() {
			var name string
			if failure := rows.Scan(&name); failure != nil {
				rows.Close()
				return failure
			}
			existing[name] = true
		}
		rows.Close()
		if failure := rows.Err(); failure != nil {
			return failure
		}
		for offset := -1; offset <= daysAhead; offset++ {
			day := today.AddDate(0, 0, offset)
			name := intendedProcessPartitionName(day)
			if existing[name] {
				continue
			}
			statement := fmt.Sprintf("create table %s partition of intended_processes for values from ('%s') to ('%s')",
				pgx.Identifier{name}.Sanitize(), day.Format(time.RFC3339), day.AddDate(0, 0, 1).Format(time.RFC3339))
			if _, failure := transaction.Exec(operation, statement); failure != nil {
				return fmt.Errorf("create %s: %w", name, failure)
			}
			changes.Created = append(changes.Created, name)
		}
		cutoff := now.Add(-retention)
		for name := range existing {
			day, failure := time.Parse("20060102", strings.TrimPrefix(name, intendedProcessPartitionPrefix))
			if failure != nil || !strings.HasPrefix(name, intendedProcessPartitionPrefix) {
				continue
			}
			if day.AddDate(0, 0, 1).After(cutoff) {
				continue
			}
			if _, failure := transaction.Exec(operation, "drop table "+pgx.Identifier{name}.Sanitize()); failure != nil {
				return fmt.Errorf("drop %s: %w", name, failure)
			}
			changes.Dropped = append(changes.Dropped, name)
		}
		return nil
	})
	if failure != nil {
		return IntendedProcessPartitionChanges{}, fmt.Errorf("maintain intended_processes partitions: %w", failure)
	}
	return changes, nil
}
