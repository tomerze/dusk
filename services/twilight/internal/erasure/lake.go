package erasure

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"path"
	"slices"
	"strings"

	"dusk/services/twilight/internal/clickhouse"
)

type lakeFile struct {
	Path    string `json:"path"`
	Matched int64  `json:"matched"`
	Total   int64  `json:"total"`
}

func (manager *Manager) structure(operation context.Context, name string) (string, error) {
	columns, failure := clickhouse.Scan[struct {
		Name string `json:"name"`
		Type string `json:"type"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT name, type FROM system.columns WHERE database = 'dusk' AND table = {table:String} ORDER BY position`,
		Parameters: map[string]string{"table": name}})
	if failure != nil {
		return "", fmt.Errorf("read the columns of dusk.%s: %w", name, failure)
	}
	if len(columns) == 0 {
		return "", fmt.Errorf("dusk.%s has no columns", name)
	}
	described := make([]string, len(columns))
	for index, column := range columns {
		described[index] = "`" + column.Name + "` " + column.Type
	}
	return strings.Join(described, ", "), nil
}

func rewrittenKey(key, erasure string) string {
	digest := sha256.Sum256([]byte(key + "/" + erasure))
	return path.Dir(key) + "/" + hex.EncodeToString(digest[:16]) + ".parquet"
}

func (manager *Manager) rewriteLake(operation context.Context, erasure *Erasure, step *Step) (bool, error) {
	if manager.ClickHouse == nil {
		return false, errNoClickHouse
	}
	if manager.ObjectStore == nil {
		return false, errNoObjectStore
	}
	bucket := manager.Config.ObjectStore.LakeBucket
	for _, current := range erasedTables {
		if slices.Contains(step.Issued, current.name) {
			continue
		}
		condition := "ifNull(" + predicate(current, erasure.ID, erasure.Before) + ", false)"
		window := ""
		if erasure.Before != nil {
			window = fmt.Sprintf(" WHERE extract(_path, '/dt=([0-9]{4}-[0-9]{2}-[0-9]{2})/') <= '%s'", erasure.Before.UTC().Format("2006-01-02"))
		}
		files, failure := clickhouse.Scan[lakeFile](operation, manager.ClickHouse, clickhouse.Query{Text: "SELECT _path AS path, countIf(" + condition + ") AS matched, count() AS total FROM dusk.lake_" +
			current.name + "_parquet" + window + " GROUP BY path HAVING matched > 0 ORDER BY path", Settings: map[string]string{"max_execution_time": "3600"}})
		if failure != nil {
			return false, fmt.Errorf("find the lake files of %s that hold the erased rows: %w", current.name, failure)
		}
		columns := ""
		for _, file := range files {
			key, found := strings.CutPrefix(file.Path, bucket+"/")
			if !found {
				return false, fmt.Errorf("the lake file %s is not in bucket %s", file.Path, bucket)
			}
			if file.Matched < file.Total {
				if columns == "" {
					if columns, failure = manager.structure(operation, current.name); failure != nil {
						return false, failure
					}
				}
				statement := "INSERT INTO FUNCTION s3(" + manager.Config.ClickHouse.LakeCollection + ", filename = " + clickhouse.Quote(rewrittenKey(key, erasure.ID.String())) +
					", format = 'Parquet', structure = " + clickhouse.Quote(columns) + ") SELECT * FROM s3(lake, filename = " + clickhouse.Quote(key) +
					", format = 'Parquet', structure = " + clickhouse.Quote(columns) + ") WHERE NOT " + condition
				if failure := manager.ClickHouse.Exec(operation, clickhouse.Query{Text: statement, Settings: map[string]string{"s3_truncate_on_insert": "1"}}); failure != nil {
					return false, fmt.Errorf("rewrite the lake file %s without the erased rows: %w", key, failure)
				}
				step.Counts["files_rewritten"]++
			} else {
				step.Counts["files_deleted"]++
			}
			if failure := manager.ObjectStore.DeleteObject(operation, bucket, key); failure != nil {
				return false, fmt.Errorf("delete the lake file %s: %w", key, failure)
			}
			step.Counts["rows"] += file.Matched
		}
		step.Issued = append(step.Issued, current.name)
	}
	step.Detail = fmt.Sprintf("removed %d rows: rewrote %d lake files and deleted %d", step.Counts["rows"], step.Counts["files_rewritten"], step.Counts["files_deleted"])
	return true, nil
}
