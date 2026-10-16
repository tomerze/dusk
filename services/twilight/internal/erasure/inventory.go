package erasure

import (
	"context"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/database"
)

func (manager *Manager) deleteInventory(operation context.Context, erasure *Erasure, step *Step) (bool, error) {
	if manager.ClickHouse == nil {
		return false, errNoClickHouse
	}
	if step.Cursor == "" {
		step.Counts["alerts_kept"] = 0
	}
	if erasure.Tenant != nil && step.Cursor == "" {
		removed, failure := manager.deleteTenantCampaigns(operation, *erasure.Tenant)
		if failure != nil {
			return false, failure
		}
		step.Counts["campaigns"] += removed
	}
	for {
		devices, failure := manager.scopeDevices(operation, erasure, step.Cursor)
		if failure != nil {
			return false, failure
		}
		if len(devices) == 0 {
			break
		}
		if failure := pgx.BeginFunc(operation, manager.Pool, func(transaction pgx.Tx) error {
			return eraseInventoryBatch(operation, transaction, devices, step.Counts, manager.Now())
		}); failure != nil {
			return false, fmt.Errorf("erase the inventory of %d devices: %w", len(devices), failure)
		}
		step.Cursor = devices[len(devices)-1]
	}
	if waiting := step.Counts["intended_processes_waiting"]; waiting > 0 {
		step.Detail = fmt.Sprintf("waiting for %d intended processes to be withdrawn from dusk.intended-processes", waiting)
		step.Counts["intended_processes_waiting"] = 0
		step.Cursor = ""
		return false, nil
	}
	delete(step.Counts, "intended_processes_waiting")
	step.Detail = fmt.Sprintf("deleted %d installations, %d campaign rows and %d intended processes; scrubbed %d campaign events",
		step.Counts["installations"], step.Counts["campaign_rows"], step.Counts["intended_processes"], step.Counts["campaign_events"])
	return true, nil
}

func eraseInventoryBatch(operation context.Context, transaction pgx.Tx, devices []string, counts map[string]int64, now time.Time) error {
	if _, failure := transaction.Exec(operation, `update intended_processes set expires_at = $2
		where device_id = any($1) and tombstoned_at is null and expires_at > $2`, devices, now); failure != nil {
		return fmt.Errorf("expire the devices' intended processes: %w", failure)
	}
	tag, failure := transaction.Exec(operation, `delete from intended_processes where device_id = any($1) and tombstoned_at is not null`, devices)
	if failure != nil {
		return fmt.Errorf("delete the devices' intended processes: %w", failure)
	}
	counts["intended_processes"] += tag.RowsAffected()
	var waiting int64
	if failure := transaction.QueryRow(operation, `select count(*) from intended_processes where device_id = any($1)`, devices).Scan(&waiting); failure != nil {
		return failure
	}
	counts["intended_processes_waiting"] += waiting
	affected, failure := collect(operation, transaction, `with removed as (delete from campaign_nodes where device_id = any($1) returning campaign_id)
		select campaign_id::text from removed`, devices)
	if failure != nil {
		return fmt.Errorf("delete the devices' campaign rows: %w", failure)
	}
	counts["campaign_rows"] += int64(len(affected))
	if len(affected) > 0 {
		finished, failure := collect(operation, transaction, `select id::text from campaigns where id = any($1::uuid[]) and status not in ('running', 'paused')`, affected)
		if failure != nil {
			return fmt.Errorf("read the finished campaigns the devices left: %w", failure)
		}
		if _, failure := transaction.Exec(operation, `delete from campaign_counters where campaign_id = any($1::uuid[])`, finished); failure != nil {
			return fmt.Errorf("clear the counters of the campaigns the devices left: %w", failure)
		}
		if _, failure := transaction.Exec(operation, `insert into campaign_counters (campaign_id, phase, state, count)
			select campaign_id, phase, state, count(*) from campaign_nodes where campaign_id = any($1::uuid[]) group by campaign_id, phase, state`, finished); failure != nil {
			return fmt.Errorf("recount the campaigns the devices left: %w", failure)
		}
	}
	tag, failure = transaction.Exec(operation, `update campaign_events set detail = detail - 'device_id' - 'installation_id' where detail ->> 'device_id' = any($1)`, devices)
	if failure != nil {
		return fmt.Errorf("scrub the devices from campaign events: %w", failure)
	}
	counts["campaign_events"] += tag.RowsAffected()
	var alertsKept int64
	if failure := transaction.QueryRow(operation, `select count(*) from alerts where detail ->> 'device_id' = any($1)`, devices).Scan(&alertsKept); failure != nil {
		return failure
	}
	counts["alerts_kept"] += alertsKept
	tag, failure = transaction.Exec(operation, `update alert_transitions set hostname = null
		where hostname is not null and alert_id in (select id from alerts where detail ->> 'device_id' = any($1))`, devices)
	if failure != nil {
		return fmt.Errorf("remove the devices' hostnames from alert notifications: %w", failure)
	}
	counts["alert_hostnames"] += tag.RowsAffected()
	if _, failure := transaction.Exec(operation, `delete from reconcile_default_shell where device_id = any($1)`, devices); failure != nil {
		return fmt.Errorf("delete the devices' reconcile windows: %w", failure)
	}
	if _, failure := transaction.Exec(operation, `update revocations set cursor_device_id = '', cursor_installation_id = ''
		where finished_at is not null and cursor_device_id = any($1)`, devices); failure != nil {
		return fmt.Errorf("clear finished revocations that stopped at the devices: %w", failure)
	}
	tag, failure = transaction.Exec(operation, `delete from nodes where device_id = any($1)`, devices)
	if failure != nil {
		return fmt.Errorf("delete the devices' installations: %w", failure)
	}
	counts["installations"] += tag.RowsAffected()
	return nil
}

func (manager *Manager) deleteTenantCampaigns(operation context.Context, tenant string) (int64, error) {
	identifiers, failure := func() ([]uuid.UUID, error) {
		rows, failure := manager.Pool.Query(operation, `select id from campaigns where tenant = $1 and status not in ('running', 'paused')`, tenant)
		if failure != nil {
			return nil, failure
		}
		return pgx.CollectRows(rows, pgx.RowTo[uuid.UUID])
	}()
	if failure != nil {
		return 0, fmt.Errorf("read the tenant's campaigns: %w", failure)
	}
	for _, identifier := range identifiers {
		if failure := database.DropCampaignPartitions(operation, manager.Pool, identifier); failure != nil {
			return 0, failure
		}
		if _, failure := manager.Pool.Exec(operation, `delete from campaigns where id = $1`, identifier); failure != nil {
			return 0, fmt.Errorf("delete campaign %s: %w", identifier, failure)
		}
		manager.Logger.Info("campaign deleted with its tenant", "campaign_id", identifier, "tenant", tenant)
	}
	return int64(len(identifiers)), nil
}
