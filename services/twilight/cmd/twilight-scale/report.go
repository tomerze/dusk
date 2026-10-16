package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"
)

const (
	defaultShellPid  = "17505437192229758416"
	rowsPerPage      = 400000
	maximumExamples  = 10
	reportRetryDelay = 10 * time.Second
	outcomeStatuses  = "'succeeded', 'failed', 'already_satisfied', 'denied', 'timed_out', 'error'"
)

var unsentStatuses = []string{"intent_unpublished", "dawn_busy", "dawn_unreachable", "dawn_rejected"}

type copiedTopic struct {
	topic   string
	table   string
	markers int64
}

var copiedTopics = []copiedTopic{{"dusk.ledger", "dusk.ledger", 1}, {"dusk.process-results", "dusk.process_results", 0}}

type reportSettings struct {
	run                string
	postgres           string
	clickhouse         string
	clickhouseUser     string
	clickhousePassword string
	kafka              []string
	wait               time.Duration
}

type nodeProcess struct {
	device       string
	installation string
	pid          string
}

type campaignRecord struct {
	id         string
	name       string
	status     string
	createdAt  time.Time
	startedAt  *time.Time
	rows       int64
	states     map[string]int64
	firstRow   *time.Time
	lastRow    *time.Time
	sent       int64
	firstSend  *time.Time
	lastSend   *time.Time
	lastFinish *time.Time
	finishedAt *time.Time
	selector   string
	parameters []string
	selected   int64
	reached    int64
	inFlight   int64
}

type resultCounts struct {
	started  int
	finals   int
	outcomes int
	statuses string
}

type ledgerCounts struct {
	started  int
	repeated int
}

type evidence struct {
	campaigns   []campaignRecord
	identifiers []string
	since       time.Time
	end         time.Time
	intended    map[nodeProcess]bool
	unsent      map[nodeProcess]bool
	results     map[nodeProcess]resultCounts
	ledger      map[nodeProcess]ledgerCounts
}

type copyTarget map[copiedTopic]map[int32]int64

type check struct {
	name     string
	passed   bool
	summary  string
	examples []string
}

func report(operation context.Context, arguments []string, output, diagnostics io.Writer) (int, error) {
	settings := reportSettings{}
	var passwordFile, brokers string
	failure := parse("report", arguments, diagnostics, func(flags *flag.FlagSet) {
		flags.StringVar(&settings.run, "run", "", "the run's name, as given to load")
		flags.StringVar(&settings.postgres, "postgres", "postgres://scale@postgres:5432/inventory?sslmode=disable", "twilight's inventory database, read with a role that may only read it")
		flags.StringVar(&settings.clickhouse, "clickhouse", "http://clickhouse:8123", "ClickHouse's HTTP address")
		flags.StringVar(&settings.clickhouseUser, "clickhouse-user", "grafana", "the ClickHouse user that reads dusk.ledger and dusk.process_results")
		flags.StringVar(&passwordFile, "clickhouse-password-file", "", "a file holding that user's password")
		flags.StringVar(&brokers, "kafka", "kafka:9092", "Kafka's brokers, separated by commas, to check that ClickHouse's copies have caught up")
		flags.DurationVar(&settings.wait, "wait", 2*time.Minute, "how long to wait for rows still on their way and for ClickHouse's copies to catch up")
	})
	if failure != nil {
		return 0, failure
	}
	if settings.run == "" {
		return 0, usageError{"report needs --run"}
	}
	settings.kafka = strings.Split(brokers, ",")
	if passwordFile != "" {
		password, failure := os.ReadFile(passwordFile)
		if failure != nil {
			return 0, fmt.Errorf("read the ClickHouse password: %w", failure)
		}
		settings.clickhousePassword = strings.TrimSpace(string(password))
	}
	database := clickhouseClient{settings: settings, http: &http.Client{Timeout: 200 * time.Second}}
	deadline := time.Now().Add(settings.wait)
	var target copyTarget
	for {
		gathered, failure := readRun(operation, settings)
		if failure != nil {
			return 0, failure
		}
		waiting := gathered.inFlight()
		if gathered.finished() && len(waiting) == 0 {
			if waiting, failure = emptyLedgerQueues(operation, database, gathered.since, gathered.end); failure != nil {
				return 0, failure
			}
			if len(waiting) == 0 && target == nil {
				if target, failure = takeTarget(operation, settings, gathered.since); failure != nil {
					return 0, failure
				}
			}
			if target != nil {
				if waiting, failure = copiesBehind(operation, database, target, gathered.since); failure != nil {
					return 0, failure
				}
			}
		}
		if !gathered.finished() || len(waiting) == 0 {
			if failure := readCopies(operation, database, &gathered); failure != nil {
				return 0, failure
			}
			return printReport(output, settings.run, gathered, judge(gathered)), nil
		}
		if time.Now().Add(reportRetryDelay).After(deadline) {
			return 0, fmt.Errorf("could not count the run: %s", strings.Join(waiting, "; "))
		}
		fmt.Fprintf(diagnostics, "%s; counting again in %s, until %s\n", strings.Join(waiting, "; "), reportRetryDelay, deadline.UTC().Format(time.TimeOnly))
		select {
		case <-operation.Done():
			return 0, operation.Err()
		case <-time.After(reportRetryDelay):
		}
	}
}

func readRun(operation context.Context, settings reportSettings) (evidence, error) {
	gathered := evidence{intended: map[nodeProcess]bool{}, unsent: map[nodeProcess]bool{}, results: map[nodeProcess]resultCounts{}, ledger: map[nodeProcess]ledgerCounts{}}
	connection, failure := pgx.Connect(operation, settings.postgres)
	if failure != nil {
		return gathered, fmt.Errorf("connect to the inventory database: %w", failure)
	}
	defer connection.Close(context.WithoutCancel(operation))
	transaction, failure := connection.BeginTx(operation, pgx.TxOptions{AccessMode: pgx.ReadOnly})
	if failure != nil {
		return gathered, fmt.Errorf("begin a read-only transaction: %w", failure)
	}
	defer transaction.Rollback(context.WithoutCancel(operation))
	if gathered.campaigns, failure = runCampaigns(operation, transaction, settings.run); failure != nil {
		return gathered, failure
	}
	if len(gathered.campaigns) == 0 {
		return gathered, fmt.Errorf("the run %s has no campaign: none is named %s/ and a number", settings.run, settings.run)
	}
	identifiers := make([]string, len(gathered.campaigns))
	gathered.since = gathered.campaigns[0].createdAt
	for index, current := range gathered.campaigns {
		identifiers[index] = current.id
		if current.createdAt.Before(gathered.since) {
			gathered.since = current.createdAt
		}
		for _, finished := range []*time.Time{current.finishedAt, current.lastFinish} {
			if finished != nil && finished.After(gathered.end) {
				gathered.end = *finished
			}
		}
	}
	gathered.identifiers = identifiers
	rows, failure := transaction.Query(operation, `SELECT DISTINCT device_id, installation_id, pid::text FROM intended_processes
		WHERE campaign_id = ANY($1::uuid[]) AND action_kind <> 'reap'`, identifiers)
	if failure != nil {
		return gathered, fmt.Errorf("read the intended processes: %w", failure)
	}
	var key nodeProcess
	if _, failure := pgx.ForEachRow(rows, []any{&key.device, &key.installation, &key.pid}, func() error {
		gathered.intended[key] = true
		return nil
	}); failure != nil {
		return gathered, fmt.Errorf("read the intended processes: %w", failure)
	}
	rows, failure = transaction.Query(operation, `SELECT device_id, installation_id, pid::text FROM campaign_nodes
		WHERE campaign_id = ANY($1::uuid[]) AND pid IS NOT NULL
			AND ((state IN ('pending', 'cancelled', 'excluded') AND last_status = ANY($2)) OR (state = 'failed' AND last_status = 'denied'))`,
		identifiers, unsentStatuses)
	if failure != nil {
		return gathered, fmt.Errorf("read the rows dawn never took: %w", failure)
	}
	if _, failure := pgx.ForEachRow(rows, []any{&key.device, &key.installation, &key.pid}, func() error {
		gathered.unsent[key] = true
		return nil
	}); failure != nil {
		return gathered, fmt.Errorf("read the rows dawn never took: %w", failure)
	}
	return gathered, nil
}

func readCopies(operation context.Context, database clickhouseClient, gathered *evidence) error {
	quoted := make([]string, len(gathered.identifiers))
	for index, identifier := range gathered.identifiers {
		quoted[index] = "'" + identifier + "'"
	}
	pages := max(1, (len(gathered.intended)+rowsPerPage-1)/rowsPerPage)
	for page := range pages {
		partition := fmt.Sprintf("pid %% %d = %d", pages, page)
		failure := clickhouseRows(operation, database, `SELECT device_id, installation_id, toString(pid) AS process,
				uniqExactIf(id, status = 'started') AS started,
				uniqExactIf(id, status != 'started') AS finals,
				uniqExactIf(id, delivered AND status IN (`+outcomeStatuses+`)) AS outcomes,
				arrayStringConcat(arraySort(groupUniqArrayIf(status, status != 'started')), ',') AS statuses
			FROM dusk.process_results
			WHERE campaign_id IN (`+strings.Join(quoted, ", ")+`) AND action_kind != 'reap' AND `+partition+`
			GROUP BY device_id, installation_id, pid`,
			func(row struct {
				Device       string `json:"device_id"`
				Installation string `json:"installation_id"`
				Pid          string `json:"process"`
				Started      int    `json:"started"`
				Finals       int    `json:"finals"`
				Outcomes     int    `json:"outcomes"`
				Statuses     string `json:"statuses"`
			}) {
				gathered.results[nodeProcess{row.Device, row.Installation, row.Pid}] = resultCounts{row.Started, row.Finals, row.Outcomes, row.Statuses}
			})
		if failure != nil {
			return fmt.Errorf("read dusk.process_results: %w", failure)
		}
		failure = clickhouseRows(operation, database, `SELECT device_id, installation_id, toString(pid) AS process,
				sumIf(entries, kind = 'result') AS started,
				maxIf(entries, kind = 'call') AS repeated
			FROM (
				SELECT device_id, installation_id, pid, kind, param_hash, uniqExact(id) AS entries
				FROM dusk.ledger
				WHERE time >= `+clickhouseTime(gathered.since.Add(-time.Minute))+`
					AND pid NOT IN (0, `+defaultShellPid+`) AND `+partition+`
					AND ((kind = 'result' AND action = 'Dusk.process' AND result_code = 'ok')
						OR (kind = 'call' AND action = 'ShPortal.sh' AND result_code IS NULL))
				GROUP BY device_id, installation_id, pid, kind, param_hash)
			GROUP BY device_id, installation_id, pid`,
			func(row struct {
				Device       string `json:"device_id"`
				Installation string `json:"installation_id"`
				Pid          string `json:"process"`
				Started      int    `json:"started"`
				Repeated     int    `json:"repeated"`
			}) {
				key := nodeProcess{row.Device, row.Installation, row.Pid}
				if gathered.intended[key] {
					gathered.ledger[key] = ledgerCounts{row.Started, row.Repeated}
				}
			})
		if failure != nil {
			return fmt.Errorf("read dusk.ledger: %w", failure)
		}
	}
	return nil
}

func clickhouseTime(moment time.Time) string {
	return fmt.Sprintf("fromUnixTimestamp64Milli(toInt64(%d))", moment.UnixMilli())
}

func runCampaigns(operation context.Context, transaction pgx.Tx, run string) ([]campaignRecord, error) {
	rows, failure := transaction.Query(operation, `SELECT c.id::text, c.name, c.status, c.created_at, c.started_at, c.finished_at,
			count(n.device_id), min(n.created_at), max(n.created_at), count(n.dispatched_at), min(n.dispatched_at), max(n.dispatched_at), max(n.finished_at),
			count(n.device_id) FILTER (WHERE n.state IN ('succeeded', 'failed', 'unknown')),
			count(n.device_id) FILTER (WHERE n.state IN ('dispatching', 'dispatched', 'delivered', 'verifying')),
			coalesce(c.selector_sql_snapshot, ''), coalesce(c.selector_sql_parameters, '[]'::jsonb)
		FROM campaigns c LEFT JOIN campaign_nodes n ON n.campaign_id = c.id
		WHERE left(c.name, length($1) + 1) = $1 || '/' AND substr(c.name, length($1) + 2) ~ '^[0-9]+$'
		GROUP BY c.id ORDER BY c.created_at, c.name`, run)
	if failure != nil {
		return nil, fmt.Errorf("read the run's campaigns: %w", failure)
	}
	var campaigns []campaignRecord
	var current campaignRecord
	if _, failure := pgx.ForEachRow(rows, []any{&current.id, &current.name, &current.status, &current.createdAt, &current.startedAt, &current.finishedAt,
		&current.rows, &current.firstRow, &current.lastRow, &current.sent, &current.firstSend, &current.lastSend, &current.lastFinish,
		&current.reached, &current.inFlight, &current.selector, &current.parameters}, func() error {
		current.states = map[string]int64{}
		campaigns = append(campaigns, current)
		return nil
	}); failure != nil {
		return nil, fmt.Errorf("read the run's campaigns: %w", failure)
	}
	for index := range campaigns {
		rows, failure := transaction.Query(operation, `SELECT state, count(*) FROM campaign_nodes WHERE campaign_id = $1::uuid GROUP BY state`, campaigns[index].id)
		if failure != nil {
			return nil, fmt.Errorf("count the rows of campaign %s: %w", campaigns[index].name, failure)
		}
		var state string
		var count int64
		if _, failure := pgx.ForEachRow(rows, []any{&state, &count}, func() error {
			campaigns[index].states[state] = count
			return nil
		}); failure != nil {
			return nil, fmt.Errorf("count the rows of campaign %s: %w", campaigns[index].name, failure)
		}
		if campaigns[index].selector == "" || campaigns[index].startedAt == nil {
			continue
		}
		window := []any{}
		for _, parameter := range campaigns[index].parameters {
			window = append(window, parameter)
		}
		ended := time.Now()
		if campaigns[index].finishedAt != nil {
			ended = *campaigns[index].finishedAt
		}
		window = append(window, *campaigns[index].startedAt, ended)
		started, finished := fmt.Sprintf("$%d", len(window)-1), fmt.Sprintf("$%d", len(window))
		if failure := transaction.QueryRow(operation, `SELECT count(*) FROM nodes n
			JOIN node_presence p ON p.device_id = n.device_id AND p.installation_id = n.installation_id
			WHERE (`+campaigns[index].selector+`)
				AND ((p.connected_at <= `+finished+` AND (p.online OR p.disconnected_at >= `+started+`))
					OR p.disconnected_at BETWEEN `+started+` AND `+finished+`)`, window...).Scan(&campaigns[index].selected); failure != nil {
			return nil, fmt.Errorf("count the nodes campaign %s selected while it ran: %w", campaigns[index].name, failure)
		}
	}
	return campaigns, nil
}

func (gathered evidence) finished() bool {
	for _, current := range gathered.campaigns {
		if !slices.Contains(terminalStatuses, current.status) {
			return false
		}
	}
	return true
}

func (gathered evidence) inFlight() []string {
	rows := int64(0)
	for _, current := range gathered.campaigns {
		rows += current.inFlight
	}
	if rows == 0 {
		return nil
	}
	return []string{fmt.Sprintf("%d rows of the run are still on their way", rows)}
}

func emptyLedgerQueues(operation context.Context, database clickhouseClient, since, end time.Time) ([]string, error) {
	var waiting []string
	failure := clickhouseRows(operation, database, `SELECT resource_attributes['service.instance.id'] AS instance,
			countIf(time > `+clickhouseTime(end)+` AND value = 0) AS empty, toString(max(time)) AS newest
		FROM dusk.otel_metrics
		WHERE service_name = 'nightfall' AND metric_name = 'nightfall_ledger_queue_depth' AND time >= `+clickhouseTime(since.Add(-time.Minute))+`
		GROUP BY instance ORDER BY instance`,
		func(row struct {
			Instance string `json:"instance"`
			Empty    int    `json:"empty"`
			Newest   string `json:"newest"`
		}) {
			if row.Empty == 0 {
				waiting = append(waiting, fmt.Sprintf("nightfall %s has not reported an empty ledger queue since the run's last row finished (its newest report: %s)", row.Instance, row.Newest))
			}
		})
	if failure != nil {
		return nil, fmt.Errorf("read nightfall's ledger queues from dusk.otel_metrics: %w", failure)
	}
	return waiting, nil
}

func takeTarget(operation context.Context, settings reportSettings, since time.Time) (copyTarget, error) {
	client, failure := kgo.NewClient(kgo.SeedBrokers(settings.kafka...))
	if failure != nil {
		return nil, fmt.Errorf("connect to Kafka: %w", failure)
	}
	defer client.Close()
	admin := kadm.NewClient(client)
	asking, cancel := context.WithTimeout(operation, 30*time.Second)
	defer cancel()
	target := copyTarget{}
	for _, copied := range copiedTopics {
		after, failure := admin.ListOffsetsAfterMilli(asking, since.Add(-time.Minute).UnixMilli(), copied.topic)
		if failure == nil {
			failure = after.Error()
		}
		if failure != nil {
			return nil, fmt.Errorf("read the offsets of %s from Kafka at the run's start: %w", copied.topic, failure)
		}
		ends, failure := admin.ListCommittedOffsets(asking, copied.topic)
		if failure == nil {
			failure = ends.Error()
		}
		if failure != nil {
			return nil, fmt.Errorf("read the last committed offsets of %s from Kafka: %w", copied.topic, failure)
		}
		target[copied] = map[int32]int64{}
		for partition, end := range ends[copied.topic] {
			if last := end.Offset - 1 - copied.markers; last >= after[copied.topic][partition].Offset {
				target[copied][partition] = last
			}
		}
	}
	return target, nil
}

func copiesBehind(operation context.Context, database clickhouseClient, target copyTarget, since time.Time) ([]string, error) {
	var behind []string
	for _, copied := range copiedTopics {
		held := map[int32]int64{}
		var parsing error
		if failure := clickhouseRows(operation, database, `SELECT kafka_partition AS partition, toString(max(kafka_offset)) AS offset
			FROM `+copied.table+` WHERE time >= `+clickhouseTime(since.Add(-time.Minute))+` GROUP BY kafka_partition`,
			func(row struct {
				Partition int32  `json:"partition"`
				Offset    string `json:"offset"`
			}) {
				offset, failure := strconv.ParseInt(row.Offset, 10, 64)
				if failure != nil {
					parsing = failure
				}
				held[row.Partition] = offset
			}); failure != nil {
			return nil, fmt.Errorf("read the offsets %s holds: %w", copied.table, failure)
		}
		if parsing != nil {
			return nil, fmt.Errorf("read the offsets %s holds: %w", copied.table, parsing)
		}
		for partition, last := range target[copied] {
			offset, present := held[partition]
			switch {
			case !present:
				behind = append(behind, fmt.Sprintf("%s holds nothing of %s partition %d since the run began, where Kafka has records up to offset %d", copied.table, copied.topic, partition, last))
			case offset < last:
				behind = append(behind, fmt.Sprintf("%s holds %s partition %d up to offset %d of %d", copied.table, copied.topic, partition, offset, last))
			}
		}
	}
	slices.Sort(behind)
	return behind, nil
}

type clickhouseClient struct {
	settings reportSettings
	http     *http.Client
}

func clickhouseRows[Row any](operation context.Context, database clickhouseClient, query string, each func(Row)) error {
	address := strings.TrimRight(database.settings.clickhouse, "/") + "/?" + url.Values{"database": {"dusk"}, "wait_end_of_query": {"1"}}.Encode()
	request, failure := http.NewRequestWithContext(operation, http.MethodPost, address, strings.NewReader(query+"\nFORMAT JSONEachRow"))
	if failure != nil {
		return failure
	}
	request.Header.Set("X-ClickHouse-User", database.settings.clickhouseUser)
	request.Header.Set("X-ClickHouse-Key", database.settings.clickhousePassword)
	response, failure := database.http.Do(request)
	if failure != nil {
		return failure
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		answer, _ := io.ReadAll(io.LimitReader(response.Body, 4096))
		return fmt.Errorf("ClickHouse answered %d: %s", response.StatusCode, strings.TrimSpace(string(answer)))
	}
	decoder := json.NewDecoder(response.Body)
	for {
		var row Row
		if failure := decoder.Decode(&row); errors.Is(failure, io.EOF) {
			return nil
		} else if failure != nil {
			return fmt.Errorf("decode ClickHouse's answer: %w", failure)
		}
		each(row)
	}
}

func judge(gathered evidence) []check {
	finished := check{name: "every campaign of the run finished"}
	statuses := map[string]int{}
	for _, current := range gathered.campaigns {
		statuses[current.status]++
		if !slices.Contains(terminalStatuses, current.status) {
			finished.examples = append(finished.examples, fmt.Sprintf("campaign %s (%s) is %s", current.name, current.id, current.status))
		}
	}
	finished.passed = len(finished.examples) == 0
	finished.summary = fmt.Sprintf("%d of %d (%s)", len(gathered.campaigns)-len(finished.examples), len(gathered.campaigns), summarize(statuses, append([]string{"running", "paused"}, terminalStatuses...)))

	coverage := check{name: "every campaign reached the nodes it selected while it ran"}
	reachedTotal, selectedTotal := int64(0), int64(0)
	for _, current := range gathered.campaigns {
		reachedTotal += current.reached
		selectedTotal += current.selected
		if current.reached < current.selected {
			coverage.examples = append(coverage.examples, fmt.Sprintf("campaign %s reached %d of the %d nodes it selected that were online while it ran", current.name, current.reached, current.selected))
		}
	}
	coverage.passed = len(coverage.examples) == 0
	coverage.summary = fmt.Sprintf("%d reached of %d selected", reachedTotal, selectedTotal)

	answered := check{name: "every intended process has a result"}
	reported := check{name: "every started process reported its outcome"}
	dawnOnce := check{name: "no pid was started twice on its node, by dawn's started results"}
	ledgerOnce := check{name: "no pid's Dusk.process was answered ok twice on its node, by the ledger"}
	commandOnce := check{name: "no command was forwarded twice under one pid, by the ledger"}
	started, unsent := 0, 0
	for _, key := range sortedKeys(gathered.intended) {
		results, ledger := gathered.results[key], gathered.ledger[key]
		where := fmt.Sprintf("device %s installation %s pid %s", key.device, key.installation, key.pid)
		if results.finals == 0 && results.started == 0 && ledger.started == 0 && gathered.unsent[key] {
			unsent++
		} else if results.finals == 0 {
			answered.examples = append(answered.examples, fmt.Sprintf("%s: no result (dawn's started results: %d, ledger's Dusk.process results: %d)", where, results.started, ledger.started))
		}
		if results.started > 0 || ledger.started > 0 {
			started++
			if results.outcomes == 0 {
				reported.examples = append(reported.examples, fmt.Sprintf("%s: started (dawn: %d, ledger: %d), no outcome from the dispatch that started it (results: %s)", where, results.started, ledger.started, orNone(results.statuses)))
			}
		}
		if results.started > 1 {
			dawnOnce.examples = append(dawnOnce.examples, fmt.Sprintf("%s: %d started results (results: %s)", where, results.started, orNone(results.statuses)))
		}
		if ledger.started > 1 {
			ledgerOnce.examples = append(ledgerOnce.examples, fmt.Sprintf("%s: %d Dusk.process results", where, ledger.started))
		}
		if ledger.repeated > 1 {
			commandOnce.examples = append(commandOnce.examples, fmt.Sprintf("%s: one ShPortal.sh forwarded %d times", where, ledger.repeated))
		}
	}
	answered.summary = fmt.Sprintf("%d of %d", len(gathered.intended)-unsent-len(answered.examples), len(gathered.intended)-unsent)
	if unsent > 0 {
		answered.summary += fmt.Sprintf(", leaving out %d that never reached dawn: twilight could not write their intent to Kafka, or dawn turned their dispatch away", unsent)
	}
	reported.summary = fmt.Sprintf("%d of %d", started-len(reported.examples), started)
	for _, current := range []*check{&dawnOnce, &ledgerOnce, &commandOnce} {
		current.summary = counted(len(current.examples), "pid", "pids")
	}
	checks := []check{finished, coverage, answered, reported, dawnOnce, ledgerOnce, commandOnce}
	for index := range checks[2:] {
		checks[index+2].passed = len(checks[index+2].examples) == 0
	}
	return checks
}

func printReport(output io.Writer, run string, gathered evidence, checks []check) int {
	nodes := map[[2]string]bool{}
	for key := range gathered.intended {
		nodes[[2]string{key.device, key.installation}] = true
	}
	fmt.Fprintf(output, "run %s: %s, %s on %s\n", run, counted(len(gathered.campaigns), "campaign", "campaigns"), counted(len(gathered.intended), "intended process", "intended processes"), counted(len(nodes), "node", "nodes"))
	for _, current := range gathered.campaigns {
		fmt.Fprintf(output, "  campaign %s (%s): %d rows (%s)%s\n", current.name, current.status, current.rows, summarize(current.states, rowStates), timings(current))
	}
	verdict := 0
	for _, current := range checks {
		mark := "PASS"
		if !current.passed {
			mark, verdict = "FAIL", 1
		}
		fmt.Fprintf(output, "%s  %s: %s\n", mark, current.name, current.summary)
		for _, example := range current.examples[:min(len(current.examples), maximumExamples)] {
			fmt.Fprintf(output, "        %s\n", example)
		}
		if len(current.examples) > maximumExamples {
			fmt.Fprintf(output, "        and %d more\n", len(current.examples)-maximumExamples)
		}
	}
	if verdict == 0 {
		fmt.Fprintln(output, "verdict: PASS")
	} else {
		fmt.Fprintln(output, "verdict: FAIL")
	}
	return verdict
}

func timings(current campaignRecord) string {
	if current.startedAt == nil || current.rows == 0 {
		return ""
	}
	text := ""
	if current.firstRow != nil && current.lastRow != nil {
		text += "; rows created over " + span(*current.firstRow, *current.lastRow, current.rows)
	}
	if current.firstSend != nil && current.lastSend != nil {
		text += fmt.Sprintf("; %d dispatched over %s", current.sent, span(*current.firstSend, *current.lastSend, current.sent))
	}
	if current.lastFinish != nil {
		text += fmt.Sprintf("; last row finished %.1f s after the start", current.lastFinish.Sub(*current.startedAt).Seconds())
	}
	return text
}

func span(first, last time.Time, count int64) string {
	seconds := last.Sub(first).Seconds()
	if seconds <= 0 {
		return "0 s"
	}
	return fmt.Sprintf("%.1f s (%.1f a second)", seconds, float64(count)/seconds)
}

func counted(count int, one, many string) string {
	if count == 1 {
		return "1 " + one
	}
	return strconv.Itoa(count) + " " + many
}

func orNone(text string) string {
	if text == "" {
		return "none"
	}
	return text
}

func sortedKeys(set map[nodeProcess]bool) []nodeProcess {
	keys := make([]nodeProcess, 0, len(set))
	for key := range set {
		keys = append(keys, key)
	}
	slices.SortFunc(keys, func(left, right nodeProcess) int {
		return strings.Compare(left.device+left.installation+left.pid, right.device+right.installation+right.pid)
	})
	return keys
}
