package erasure

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strings"

	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/clickhouse"
	"dusk/services/twilight/internal/objectstore"
)

const (
	scopePage     = 10000
	inventoryPage = 1000
)

var (
	errNoClickHouse  = errors.New("clickhouse is not configured; the erasure waits for it")
	errNoObjectStore = errors.New("the object store is not configured; the erasure waits for it")
	devicePattern    = regexp.MustCompile(`^[0-9a-f]{32}$`)
)

func nondeterministic() map[string]string {
	return map[string]string{"allow_nondeterministic_mutations": "1"}
}

func (manager *Manager) insertScope(operation context.Context, erasure *Erasure, kind string, values []string) error {
	if len(values) == 0 {
		return nil
	}
	var body strings.Builder
	for _, value := range values {
		body.WriteString(erasure.ID.String() + "\t" + kind + "\t" + value + "\n")
	}
	return manager.ClickHouse.Exec(operation, clickhouse.Query{Text: "INSERT INTO dusk.erasure_scope (erasure_id, kind, value) FORMAT TSV", Body: strings.NewReader(body.String())})
}

func collect(operation context.Context, transaction pgx.Tx, sql string, arguments ...any) ([]string, error) {
	rows, failure := transaction.Query(operation, sql, arguments...)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, pgx.RowTo[string])
}

func (manager *Manager) scope(operation context.Context, erasure *Erasure, step *Step) (bool, error) {
	if manager.ClickHouse == nil {
		return false, errNoClickHouse
	}
	parameters := map[string]string{"erasure": erasure.ID.String()}
	membership := `node.device_id = $1`
	argument := ""
	if erasure.DeviceID != nil {
		argument = *erasure.DeviceID
	} else {
		membership = `node.tenant = $1`
		argument = *erasure.Tenant
		parameters["tenant"] = *erasure.Tenant
	}
	var devices, hosts []string
	failure := pgx.BeginTxFunc(operation, manager.Pool, pgx.TxOptions{AccessMode: pgx.ReadOnly}, func(transaction pgx.Tx) error {
		var failure error
		if devices, failure = collect(operation, transaction, `select distinct node.device_id from nodes node where `+membership, argument); failure != nil {
			return failure
		}
		hosts, failure = collect(operation, transaction, `select distinct namespace from (
				select session ->> 'namespace_id' as namespace from nodes node join node_presence presence using (device_id, installation_id)
					cross join jsonb_array_elements(presence.sessions) as session where `+membership+`
				union all select node.facts_namespace_id from nodes node where `+membership+`
				union all select row.namespace_id from campaign_nodes row join nodes node using (device_id, installation_id) where `+membership+`
			) as found where namespace is not null`, argument)
		return failure
	})
	if failure != nil {
		return false, fmt.Errorf("read the scope from the inventory: %w", failure)
	}
	if erasure.DeviceID != nil && !slices.Contains(devices, *erasure.DeviceID) {
		devices = append(devices, *erasure.DeviceID)
	}
	for start := 0; start < len(devices); start += scopePage {
		if failure := manager.insertScope(operation, erasure, "device", devices[start:min(start+scopePage, len(devices))]); failure != nil {
			return false, failure
		}
	}
	if erasure.Tenant != nil {
		for _, source := range []string{"connections", "enrollments", "lake_connections_parquet", "lake_enrollments_parquet"} {
			if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: `INSERT INTO dusk.erasure_scope (erasure_id, kind, value)
				SELECT {erasure:UUID}, 'device', device_id FROM dusk.` + source + ` WHERE tenant = {tenant:String} AND device_id != '' GROUP BY device_id`,
				Parameters: parameters}); failure != nil {
				return false, fmt.Errorf("add the tenant's devices from %s: %w", source, failure)
			}
		}
	}
	for index := range hosts {
		hosts[index] = hostOf(hosts[index])
	}
	if failure := manager.insertScope(operation, erasure, "host", hosts); failure != nil {
		return false, failure
	}
	for _, source := range []string{"connections", "process_results", "lake_connections_parquet", "lake_process_results_parquet"} {
		if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: `INSERT INTO dusk.erasure_scope (erasure_id, kind, value)
			SELECT {erasure:UUID}, 'host', replaceRegexpOne(namespace_id, '^0+(.)', '\\1') FROM dusk.` + source + `
			WHERE device_id IN ` + scopeSet(erasure.ID, "device") + ` AND namespace_id != '' GROUP BY namespace_id`, Parameters: parameters}); failure != nil {
			return false, fmt.Errorf("add the namespaces from %s: %w", source, failure)
		}
	}
	if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: "SYSTEM SYNC REPLICA ON CLUSTER " + manager.Config.ClickHouse.Cluster + " dusk.erasure_scope"}); failure != nil {
		return false, fmt.Errorf("wait for every ClickHouse replica to hold the scope: %w", failure)
	}
	counted, failure := clickhouse.Scan[struct {
		Kind  string `json:"kind"`
		Count int64  `json:"count"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT kind, uniqExact(value) AS count FROM dusk.erasure_scope WHERE erasure_id = {erasure:UUID} GROUP BY kind`,
		Parameters: parameters})
	if failure != nil {
		return false, failure
	}
	for _, row := range counted {
		step.Counts[row.Kind+"s"] = row.Count
	}
	step.Detail = fmt.Sprintf("%d devices and %d node namespaces", step.Counts["devices"], step.Counts["hosts"])
	return true, nil
}

func (manager *Manager) deleteRows(operation context.Context, erasure *Erasure, step *Step) (bool, error) {
	if manager.ClickHouse == nil {
		return false, errNoClickHouse
	}
	for _, current := range erasedTables {
		if slices.Contains(step.Issued, current.name) {
			continue
		}
		condition := predicate(current, erasure.ID)
		counted, failure := clickhouse.Scan[struct {
			Rows int64 `json:"rows"`
		}](operation, manager.ClickHouse, clickhouse.Query{Text: "SELECT count() AS rows FROM dusk." + current.name + " WHERE " + condition})
		if failure != nil {
			return false, fmt.Errorf("count the rows of dusk.%s to erase: %w", current.name, failure)
		}
		rows := int64(0)
		if len(counted) == 1 {
			rows = counted[0].Rows
		}
		if rows > 0 {
			statement := "ALTER TABLE dusk." + current.name + " DELETE WHERE " + condition
			if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: statement, Settings: nondeterministic()}); failure != nil {
				return false, fmt.Errorf("delete the rows of dusk.%s: %w", current.name, failure)
			}
		}
		step.Counts[current.name] = rows
		step.Issued = append(step.Issued, current.name)
	}
	pending, failure := clickhouse.Scan[struct {
		Pending int64  `json:"pending"`
		Failure string `json:"failure"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT count() AS pending, anyIf(latest_fail_reason, latest_fail_reason != '') AS failure
		FROM clusterAllReplicas(` + manager.Config.ClickHouse.Cluster + `, system.mutations)
		WHERE database = 'dusk' AND NOT is_done AND position(command, {erasure:String}) > 0`, Parameters: map[string]string{"erasure": erasure.ID.String()}})
	if failure != nil {
		return false, fmt.Errorf("read the progress of the ClickHouse deletes: %w", failure)
	}
	var total int64
	for _, rows := range step.Counts {
		total += rows
	}
	if len(pending) == 1 && pending[0].Pending > 0 {
		step.Detail = fmt.Sprintf("deleting %d rows: %d ClickHouse mutations are running", total, pending[0].Pending)
		if pending[0].Failure != "" {
			return false, fmt.Errorf("a ClickHouse delete keeps failing: %s", pending[0].Failure)
		}
		return false, nil
	}
	step.Detail = fmt.Sprintf("deleted %d rows from %d tables", total, len(erasedTables))
	return true, nil
}

func (manager *Manager) scopeDevices(operation context.Context, erasure *Erasure, after string) ([]string, error) {
	rows, failure := clickhouse.Scan[struct {
		Value string `json:"value"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT DISTINCT value FROM dusk.erasure_scope
		WHERE erasure_id = {erasure:UUID} AND kind = 'device' AND value > {after:String} ORDER BY value LIMIT {limit:UInt32}`,
		Parameters: map[string]string{"erasure": erasure.ID.String(), "after": after, "limit": fmt.Sprint(inventoryPage)}})
	if failure != nil {
		return nil, fmt.Errorf("read the erasure's devices: %w", failure)
	}
	devices := make([]string, 0, len(rows))
	for _, row := range rows {
		if !devicePattern.MatchString(row.Value) {
			return nil, fmt.Errorf("the erasure's scope holds %q, which is not a device id", row.Value)
		}
		devices = append(devices, row.Value)
	}
	return devices, nil
}

func (manager *Manager) deleteFiles(operation context.Context, erasure *Erasure, step *Step) (bool, error) {
	if manager.ClickHouse == nil {
		return false, errNoClickHouse
	}
	if manager.ObjectStore == nil {
		return false, errNoObjectStore
	}
	bucket := manager.Config.ObjectStore.FilesBucket
	for {
		devices, failure := manager.scopeDevices(operation, erasure, step.Cursor)
		if failure != nil {
			return false, failure
		}
		if len(devices) == 0 {
			step.Detail = fmt.Sprintf("deleted %d collected files (%d bytes)", step.Counts["objects"], step.Counts["bytes"])
			return true, nil
		}
		for _, device := range devices {
			var keys []string
			var bytes int64
			if failure := manager.ObjectStore.List(operation, bucket, "files/"+device+"/", func(object objectstore.Object) error {
				keys = append(keys, object.Key)
				bytes += object.Size
				return nil
			}); failure != nil {
				return false, fmt.Errorf("list the collected files of device %s: %w", device, failure)
			}
			if failure := manager.ObjectStore.DeleteObjects(operation, bucket, keys); failure != nil {
				return false, fmt.Errorf("delete the collected files of device %s: %w", device, failure)
			}
			step.Counts["objects"] += int64(len(keys))
			step.Counts["bytes"] += bytes
			step.Cursor = device
		}
	}
}
