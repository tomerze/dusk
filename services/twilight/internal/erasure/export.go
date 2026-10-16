package erasure

import (
	"context"
	"encoding/json"
	"io"
	"slices"
	"strings"
	"time"

	"dusk/services/twilight/internal/clickhouse"
	"dusk/services/twilight/internal/objectstore"
)

const exportTimeout = 30 * time.Minute

type exportLine struct {
	Store  string          `json:"store"`
	Table  string          `json:"table"`
	Record json.RawMessage `json:"record,omitempty"`
	Error  string          `json:"error,omitempty"`
}

type exportSummary struct {
	Summary struct {
		DeviceID    string           `json:"device_id"`
		ExportedAt  time.Time        `json:"exported_at"`
		Complete    bool             `json:"complete"`
		Records     map[string]int64 `json:"records"`
		NotExported []string         `json:"not_exported"`
	} `json:"summary"`
}

var exportedPostgres = []struct{ table, query string }{
	{"nodes", `select row_to_json(row) from nodes row where device_id = $1`},
	{"node_presence", `select row_to_json(row) from node_presence row where device_id = $1`},
	{"devices", `select row_to_json(row) from devices row where device_id = $1`},
	{"campaign_nodes", `select row_to_json(row) from campaign_nodes row where device_id = $1`},
	{"campaign_events", `select row_to_json(row) from campaign_events row where detail ->> 'device_id' = $1`},
	{"intended_processes", `select row_to_json(row) from intended_processes row where device_id = $1`},
	{"alerts", `select row_to_json(row) from alerts row where detail ->> 'device_id' = $1`},
	{"alert_transitions", `select row_to_json(row) from alert_transitions row where alert_id in (select id from alerts where detail ->> 'device_id' = $1)`},
	{"erasures", `select row_to_json(row) from erasures row where device_id = $1`},
}

func (manager *Manager) Export(operation context.Context, device, reason, actor string, output io.Writer, flush func()) error {
	reason, failure := ValidReason(reason)
	if failure != nil {
		return failure
	}
	operation, cancel := context.WithTimeout(operation, exportTimeout)
	defer cancel()
	manager.Logger.Info("device data export started", "device_id", device, "principal", actor, "reason", reason)
	started := manager.Now()
	encoder := json.NewEncoder(output)
	var summary exportSummary
	summary.Summary.DeviceID = device
	summary.Summary.Complete = true
	summary.Summary.Records = map[string]int64{}
	emit := func(line exportLine) error {
		if line.Error != "" {
			summary.Summary.Complete = false
		} else {
			summary.Summary.Records[line.Store+" "+line.Table]++
		}
		return encoder.Encode(line)
	}
	hosts := map[string]bool{}
	for _, current := range exportedPostgres {
		rows, failure := manager.Pool.Query(operation, current.query, device)
		if failure == nil {
			for rows.Next() {
				var record json.RawMessage
				if failure = rows.Scan(&record); failure != nil {
					break
				}
				if current.table == "node_presence" || current.table == "campaign_nodes" || current.table == "nodes" {
					collectHosts(record, hosts)
				}
				if failure = emit(exportLine{Store: "postgres", Table: current.table, Record: record}); failure != nil {
					rows.Close()
					return failure
				}
			}
			rows.Close()
			if failure == nil {
				failure = rows.Err()
			}
		}
		if failure != nil {
			if operation.Err() != nil {
				return operation.Err()
			}
			if failure := emit(exportLine{Store: "postgres", Table: current.table, Error: failure.Error()}); failure != nil {
				return failure
			}
		}
	}
	flush()
	parameters := map[string]string{"device": device}
	connections, failure := clickhouse.Scan[struct {
		Namespace string `json:"namespace_id"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT DISTINCT namespace_id FROM dusk.connections WHERE device_id = {device:String}`, Parameters: parameters})
	if failure != nil {
		if failure := emit(exportLine{Store: "clickhouse", Table: "connections", Error: "the node namespaces were not read: " + failure.Error()}); failure != nil {
			return failure
		}
	}
	for _, connection := range connections {
		hosts[hostOf(connection.Namespace)] = true
	}
	hostList := make([]string, 0, len(hosts))
	for host := range hosts {
		hostList = append(hostList, host)
	}
	slices.Sort(hostList)
	quoted := make([]string, len(hostList))
	for index, host := range hostList {
		quoted[index] = clickhouse.Quote(host)
	}
	parameters["hosts"] = "[" + strings.Join(quoted, ",") + "]"
	for _, current := range append([]table{{"ledger", false}}, erasedTables...) {
		condition := "device_id = {device:String}"
		if current.telemetry {
			condition = "(attributes['device_id'] = {device:String} OR has({hosts:Array(String)}, resource_attributes['host.id']))"
		}
		for _, source := range []struct{ store, relation string }{{"clickhouse", "dusk." + current.name}, {"lake", "dusk.lake_" + current.name}} {
			failure := manager.ClickHouse.Rows(operation, clickhouse.Query{Text: "SELECT * FROM " + source.relation + " WHERE " + condition, Parameters: parameters,
				Settings: map[string]string{"max_execution_time": "1800", "output_format_json_quote_64bit_integers": "1"}}, func(line []byte) error {
				return emit(exportLine{Store: source.store, Table: current.name, Record: append(json.RawMessage(nil), line...)})
			})
			if failure != nil {
				if operation.Err() != nil {
					return operation.Err()
				}
				if failure := emit(exportLine{Store: source.store, Table: current.name, Error: failure.Error()}); failure != nil {
					return failure
				}
			}
			flush()
		}
	}
	failure = manager.ObjectStore.List(operation, manager.Config.ObjectStore.FilesBucket, "files/"+device+"/", func(object objectstore.Object) error {
		record, _ := json.Marshal(map[string]any{"bucket": manager.Config.ObjectStore.FilesBucket, "key": object.Key, "size_bytes": object.Size, "last_modified": object.LastModified})
		return emit(exportLine{Store: "object store", Table: manager.Config.ObjectStore.FilesBucket, Record: record})
	})
	if failure != nil {
		if operation.Err() != nil {
			return operation.Err()
		}
		if failure := emit(exportLine{Store: "object store", Table: manager.Config.ObjectStore.FilesBucket, Error: failure.Error()}); failure != nil {
			return failure
		}
	}
	summary.Summary.ExportedAt = manager.Now().UTC()
	summary.Summary.NotExported = []string{
		"the Kafka topics, which hold the same records as the ClickHouse tables above for their retention",
		"the ledger's evidence copy, which holds the same entries as the ledger rows above and is readable only with the evidence bucket's own credentials",
		"the collected files' contents, which the object store records above name; download them from the files bucket",
		"SigNoz, which holds the node logs and traces it read from dusk.otel-logs and dusk.otel-spans",
	}
	if failure := encoder.Encode(summary); failure != nil {
		return failure
	}
	flush()
	var total int64
	for _, count := range summary.Summary.Records {
		total += count
	}
	manager.Logger.Info("device data export finished", "device_id", device, "principal", actor, "records", total, "complete", summary.Summary.Complete,
		"seconds", manager.Now().Sub(started).Seconds())
	return nil
}

func collectHosts(record json.RawMessage, hosts map[string]bool) {
	var fields struct {
		NamespaceID     *string `json:"namespace_id"`
		FactsNamespaces *string `json:"facts_namespace_id"`
		Sessions        []struct {
			NamespaceID string `json:"namespace_id"`
		} `json:"sessions"`
	}
	if json.Unmarshal(record, &fields) != nil {
		return
	}
	for _, namespace := range []*string{fields.NamespaceID, fields.FactsNamespaces} {
		if namespace != nil && *namespace != "" {
			hosts[hostOf(*namespace)] = true
		}
	}
	for _, session := range fields.Sessions {
		if session.NamespaceID != "" {
			hosts[hostOf(session.NamespaceID)] = true
		}
	}
}
