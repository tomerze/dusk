//go:build integration && scale

package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

type seededNode struct {
	device       string
	installation string
	pid          string
	online       bool
	connected    string
	state        string
	lastStatus   string
	results      []string
	ledger       []string
}

func clickhouseStatement(endpoint, statement string) error {
	request, failure := http.NewRequest(http.MethodPost, endpoint+"/?"+url.Values{"user": {"report"}, "password": {"report"}}.Encode(), strings.NewReader(statement))
	if failure != nil {
		return failure
	}
	response, failure := http.DefaultClient.Do(request)
	if failure != nil {
		return fmt.Errorf("clickhouse: %w", failure)
	}
	defer response.Body.Close()
	answer, _ := io.ReadAll(response.Body)
	if response.StatusCode != http.StatusOK {
		return fmt.Errorf("clickhouse answered %d to %.80q: %s", response.StatusCode, statement, answer)
	}
	return nil
}

func insertStatement(table string, rows []map[string]any) (string, error) {
	var body bytes.Buffer
	body.WriteString("INSERT INTO " + table + " FORMAT JSONEachRow\n")
	for _, row := range rows {
		encoded, failure := json.Marshal(row)
		if failure != nil {
			return "", failure
		}
		body.Write(encoded)
		body.WriteByte('\n')
	}
	return body.String(), nil
}

func insertRows(test *testing.T, endpoint, table string, rows []map[string]any) {
	test.Helper()
	statement, failure := insertStatement(table, rows)
	if failure == nil {
		failure = clickhouseStatement(endpoint, statement)
	}
	if failure != nil {
		test.Fatal(failure)
	}
}

func applyClickHouseSchema(test *testing.T, endpoint string) {
	test.Helper()
	directory := filepath.Join("..", "..", "..", "..", "infra", "k8s", "base", "clickhouse", "schema")
	clustered := regexp.MustCompile(`\s+ON CLUSTER dusk`)
	for _, file := range []string{"001-database.sql", "010-ledger.sql", "013-process-results.sql", "020-otel.sql"} {
		content, failure := os.ReadFile(filepath.Join(directory, file))
		if failure != nil {
			test.Fatal(failure)
		}
		schema := clustered.ReplaceAllString(string(content), "")
		schema = strings.ReplaceAll(schema, "ReplicatedReplacingMergeTree", "ReplacingMergeTree")
		schema = strings.ReplaceAll(schema, "ReplicatedMergeTree", "MergeTree")
		for statement := range strings.SplitSeq(schema, ";") {
			if strings.TrimSpace(statement) == "" {
				continue
			}
			if failure := clickhouseStatement(endpoint, statement); failure != nil {
				test.Fatal(failure)
			}
		}
	}
}

func queueDepth(test *testing.T, endpoint string, depth int) {
	test.Helper()
	insertRows(test, endpoint, "dusk.otel_metrics", []map[string]any{{
		"time": time.Now().UTC().Format("2006-01-02 15:04:05.000000000"), "service_name": "nightfall", "metric_name": "nightfall_ledger_queue_depth",
		"metric_type": "gauge", "value": depth, "resource_attributes": map[string]string{"service.instance.id": "nightfall:9100"},
	}})
}

func produceRecords(client *kgo.Client, count int, transactional bool) ([]int64, error) {
	operation, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	if transactional {
		if failure := client.BeginTransaction(); failure != nil {
			return nil, failure
		}
	}
	offsets := make([]int64, 0, count)
	for index := range count {
		produced, failure := client.ProduceSync(operation, &kgo.Record{Value: []byte(fmt.Sprint(index))}).First()
		if failure != nil {
			return nil, failure
		}
		offsets = append(offsets, produced.Offset)
	}
	if transactional {
		if failure := client.EndTransaction(operation, kgo.TryCommit); failure != nil {
			return nil, failure
		}
	}
	return offsets, nil
}

func producer(test *testing.T, brokers, topic string, transactional bool) *kgo.Client {
	test.Helper()
	options := []kgo.Opt{kgo.SeedBrokers(brokers), kgo.DefaultProduceTopic(topic)}
	if transactional {
		options = append(options, kgo.TransactionalID("report-test-"+topic+"-"+uuid.NewString()))
	}
	client, failure := kgo.NewClient(options...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(client.Close)
	return client
}

func TestTheReportCountsARunFromPostgresClickHouseAndKafka(test *testing.T) {
	operation := context.Background()
	pool, postgresURL := testsupport.Database(test)
	clickhouse := testsupport.ClickHouse(test, "report", "report")
	brokers := testsupport.Brokers(test)
	applyClickHouseSchema(test, clickhouse)

	administration, failure := kgo.NewClient(kgo.SeedBrokers(brokers))
	if failure != nil {
		test.Fatal(failure)
	}
	defer administration.Close()
	if _, failure := kadm.NewClient(administration).CreateTopics(operation, 1, 1, nil, "dusk.ledger", "dusk.process-results"); failure != nil {
		test.Fatal(failure)
	}
	ledgerProducer := producer(test, brokers, "dusk.ledger", true)
	resultsProducer := producer(test, brokers, "dusk.process-results", false)

	installation := func(letter string) string { return strings.Repeat(letter, 32) }
	before := "4 hours"
	nodes := []seededNode{
		{"00000000000000000000000000000001", installation("a"), "17000000000000000001", true, before, "succeeded", "succeeded", []string{"started", "succeeded"}, []string{"process", "script"}},
		{"00000000000000000000000000000002", installation("b"), "17000000000000000002", true, before, "unknown", "deadline", nil, nil},
		{"00000000000000000000000000000003", installation("c"), "17000000000000000003", true, before, "succeeded", "succeeded", []string{"started", "succeeded"}, []string{"process", "process", "script"}},
		{"00000000000000000000000000000004", installation("d"), "17000000000000000004", true, before, "succeeded", "succeeded", []string{"started", "succeeded"}, []string{"process", "script", "script"}},
		{"00000000000000000000000000000005", installation("e"), "17000000000000000005", true, before, "unknown", "deadline", []string{"started", "running"}, []string{"process"}},
		{"00000000000000000000000000000006", installation("f"), "17000000000000000006", true, before, "cancelled", "dawn_busy", nil, nil},
		{"00000000000000000000000000000007", installation("9"), "", false, "", "", "", nil, nil},
		{"00000000000000000000000000000008", installation("8"), "", true, "30 minutes", "", "", nil, nil},
	}
	campaign := uuid.Must(uuid.NewV7())
	transaction, failure := pool.Begin(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	defer transaction.Rollback(operation)
	for _, name := range []string{"scale-test/x", "scale-test-other/0"} {
		if _, failure := transaction.Exec(operation, `INSERT INTO campaigns (id, name, status, kind, selector, action, policy, salt, created_by)
			VALUES ($1, $2, 'completed', 'run_script', 'has(device_id)', '{}', '{}', $3, 'test')`, uuid.Must(uuid.NewV7()), name, make([]byte, 32)); failure != nil {
			test.Fatal(failure)
		}
	}
	if _, failure := transaction.Exec(operation, `INSERT INTO campaigns (id, name, status, kind, selector, selector_sql_snapshot, selector_sql_parameters, action, policy, salt, created_by,
			created_at, started_at, finished_at)
		VALUES ($1, 'scale-test/0', 'completed', 'run_script', 'device_id >= "0" and device_id < "1"', '(n.device_id >= $1 and n.device_id < $2)', '["0", "1"]', '{}', '{}', $2, 'test',
			now() - interval '3 hours', now() - interval '2 hours', now() - interval '1 hour')`,
		campaign, make([]byte, 32)); failure != nil {
		test.Fatal(failure)
	}
	if failure := database.CreateCampaignPartitions(operation, transaction, campaign); failure != nil {
		test.Fatal(failure)
	}
	var ledgerRows, resultRows []map[string]any
	now := time.Now().UTC().Format("2006-01-02 15:04:05.000000000")
	for _, node := range nodes {
		if _, failure := transaction.Exec(operation, `INSERT INTO nodes (device_id, installation_id) VALUES ($1, $2)`, node.device, node.installation); failure != nil {
			test.Fatal(failure)
		}
		if _, failure := transaction.Exec(operation, `INSERT INTO node_presence (device_id, installation_id, online, connected_at)
			VALUES ($1, $2, $3, CASE WHEN $4 = '' THEN NULL ELSE now() - $4::interval END)`, node.device, node.installation, node.online, node.connected); failure != nil {
			test.Fatal(failure)
		}
		if node.pid == "" {
			continue
		}
		if _, failure := transaction.Exec(operation, `INSERT INTO campaign_nodes (campaign_id, device_id, installation_id, phase, state, pid, last_status)
			VALUES ($1, $2, $3, 0, $4, $5::numeric, $6)`, campaign, node.device, node.installation, node.state, node.pid, node.lastStatus); failure != nil {
			test.Fatal(failure)
		}
		if _, failure := transaction.Exec(operation, `INSERT INTO intended_processes (pid, created_at, campaign_id, attempt, device_id, installation_id, action_kind,
				principal, subject, expires_at, max_commands, default_shell_commands, last_dispatched_at)
			VALUES ($1::numeric, now(), $2, 1, $3, $4, 'run_script', 'token:test', 'campaign:test', now() + interval '1 hour', 1, 4, now())`,
			node.pid, campaign, node.device, node.installation); failure != nil {
			test.Fatal(failure)
		}
		for _, status := range node.results {
			resultRows = append(resultRows, map[string]any{"time": now, "id": uuid.Must(uuid.NewV7()).String(), "pid": node.pid, "campaign_id": campaign.String(),
				"device_id": node.device, "installation_id": node.installation, "action_kind": "run_script", "status": status, "delivered": true,
				"started_at": now, "kafka_partition": 0})
		}
		for _, entry := range node.ledger {
			row := map[string]any{"time": now, "id": uuid.Must(uuid.NewV7()).String(), "instance": "nightfall-0", "pid": node.pid,
				"device_id": node.device, "installation_id": node.installation, "principal": "dawn-0", "kafka_partition": 0}
			if entry == "process" {
				row["kind"], row["action"], row["result_code"] = "result", "Dusk.process", "ok"
			} else {
				row["kind"], row["action"], row["param_hash"] = "call", "ShPortal.sh", "the-script"
			}
			ledgerRows = append(ledgerRows, row)
		}
	}
	if failure := transaction.Commit(operation); failure != nil {
		test.Fatal(failure)
	}
	resultOffsets, failure := produceRecords(resultsProducer, len(resultRows), false)
	if failure != nil {
		test.Fatal(failure)
	}
	ledgerOffsets, failure := produceRecords(ledgerProducer, len(ledgerRows), true)
	if failure != nil {
		test.Fatal(failure)
	}
	for index, offset := range resultOffsets {
		resultRows[index]["kafka_offset"] = offset
	}
	for index, offset := range ledgerOffsets {
		ledgerRows[index]["kafka_offset"] = offset
	}
	insertRows(test, clickhouse, "dusk.process_results", resultRows)
	insertRows(test, clickhouse, "dusk.ledger", ledgerRows)

	password := filepath.Join(test.TempDir(), "password")
	if failure := os.WriteFile(password, []byte("report\n"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	arguments := func(wait string) []string {
		return []string{"report", "--run", "scale-test", "--postgres", postgresURL, "--clickhouse", clickhouse,
			"--clickhouse-user", "report", "--clickhouse-password-file", password, "--kafka", brokers, "--wait", wait}
	}
	var output, diagnostics bytes.Buffer

	queueDepth(test, clickhouse, 3)
	if code := run(operation, arguments("0s"), &output, &diagnostics); code != 2 {
		test.Fatalf("exit %d while nightfall's ledger queue holds entries, want 2\n%s%s", code, output.String(), diagnostics.String())
	}
	if want := "nightfall nightfall:9100 has not reported an empty ledger queue since the run's last row finished"; !strings.Contains(diagnostics.String(), want) {
		test.Errorf("diagnostics lack %q:\n%s", want, diagnostics.String())
	}

	queueDepth(test, clickhouse, 0)
	stop := make(chan struct{})
	var inserting sync.WaitGroup
	failures := make(chan error, 100)
	inserting.Add(1)
	go func() {
		defer inserting.Done()
		for {
			select {
			case <-stop:
				return
			case <-time.After(200 * time.Millisecond):
			}
			stamp := time.Now().UTC().Format("2006-01-02 15:04:05.000000000")
			resultOffsets, failure := produceRecords(resultsProducer, 1, false)
			if failure != nil {
				failures <- failure
				return
			}
			ledgerOffsets, failure := produceRecords(ledgerProducer, 1, true)
			if failure != nil {
				failures <- failure
				return
			}
			result := []map[string]any{{"time": stamp, "id": uuid.Must(uuid.NewV7()).String(), "pid": "70000", "campaign_id": uuid.NewString(),
				"device_id": "00000000000000000000000000000099", "installation_id": installation("0"), "action_kind": "collect_facts", "status": "succeeded",
				"started_at": stamp, "kafka_partition": 0, "kafka_offset": resultOffsets[0]}}
			entry := []map[string]any{{"time": stamp, "id": uuid.Must(uuid.NewV7()).String(), "instance": "nightfall-0", "kind": "checkpoint", "pid": "0",
				"principal": "nightfall", "kafka_partition": 0, "kafka_offset": ledgerOffsets[0]}}
			inserting.Add(1)
			time.AfterFunc(2*time.Second, func() {
				defer inserting.Done()
				for _, table := range []struct {
					name string
					rows []map[string]any
				}{{"dusk.process_results", result}, {"dusk.ledger", entry}} {
					statement, failure := insertStatement(table.name, table.rows)
					if failure == nil {
						failure = clickhouseStatement(clickhouse, statement)
					}
					if failure != nil {
						failures <- failure
					}
				}
			})
		}
	}()
	time.Sleep(time.Second)
	output.Reset()
	diagnostics.Reset()
	code := run(operation, arguments("90s"), &output, &diagnostics)
	close(stop)
	inserting.Wait()
	close(failures)
	for failure := range failures {
		test.Errorf("the traffic beside the report failed: %v", failure)
	}
	if code != 1 {
		test.Fatalf("exit %d while the topics kept being written, want 1 (FAIL)\n%s%s", code, output.String(), diagnostics.String())
	}
	if !strings.Contains(diagnostics.String(), "counting again") {
		test.Errorf("the report never waited for ClickHouse while the topics kept being written:\n%s", diagnostics.String())
	}
	for _, line := range []string{
		"run scale-test: 1 campaign, 6 intended processes on 6 nodes",
		"PASS  every campaign of the run finished: 1 of 1 (completed 1)",
		"FAIL  every campaign reached the nodes it selected while it ran: 5 reached of 6 selected",
		"FAIL  every intended process has a result: 4 of 5, leaving out 1 that never reached dawn",
		"device 00000000000000000000000000000002 installation " + installation("b") + " pid 17000000000000000002: no result",
		"FAIL  every started process reported its outcome: 3 of 4",
		"pid 17000000000000000005: started (dawn: 1, ledger: 1), no outcome from the dispatch that started it (results: running)",
		"PASS  no pid was started twice on its node, by dawn's started results: 0 pids",
		"FAIL  no pid's Dusk.process was answered ok twice on its node, by the ledger: 1 pid",
		"pid 17000000000000000003: 2 Dusk.process results",
		"FAIL  no command was forwarded twice under one pid, by the ledger: 1 pid",
		"pid 17000000000000000004: one ShPortal.sh forwarded 2 times",
		"verdict: FAIL",
	} {
		if !strings.Contains(output.String(), line) {
			test.Errorf("the report lacks %q:\n%s", line, output.String())
		}
	}

	extra, failure := produceRecords(resultsProducer, 1, false)
	if failure != nil {
		test.Fatal(failure)
	}
	output.Reset()
	diagnostics.Reset()
	if code := run(operation, arguments("0s"), &output, &diagnostics); code != 2 {
		test.Fatalf("exit %d with a result ClickHouse does not hold, want 2\n%s%s", code, output.String(), diagnostics.String())
	}
	if want := fmt.Sprintf("could not count the run: dusk.process_results holds dusk.process-results partition 0 up to offset %d of %d", extra[0]-1, extra[0]); !strings.Contains(diagnostics.String(), want) {
		test.Errorf("diagnostics lack %q:\n%s", want, diagnostics.String())
	}
	if output.Len() > 0 {
		test.Errorf("a run ClickHouse has not caught up with printed a verdict:\n%s", output.String())
	}
}
