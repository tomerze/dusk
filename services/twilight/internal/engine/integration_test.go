//go:build integration

package engine_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime/pprof"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

type harness struct {
	test      *testing.T
	pool      *pgxpool.Pool
	settings  config.Config
	producer  *kgo.Client
	validator *kafka.Validator
	fake      *fakeDawn
	engine    *engine.Engine
	cancel    context.CancelFunc
	stopped   chan struct{}
}

type node struct {
	device, installation, namespace string
	epoch                           int64
	version                         string
}

type fakeDawn struct {
	harness    *harness
	mutex      sync.Mutex
	nodes      map[string]*node
	processes  map[string]map[campaign.Pid]bool
	dispatches []dispatchRecord
	reaps      map[string][]campaign.Pid
	facts      int
	fail       map[string]string
}

type dispatchRecord struct {
	node dawn.NodeRef
	work []dawn.Work
}

func (fake *fakeDawn) intended(pid campaign.Pid, node dawn.NodeRef, actionKind string) bool {
	var found bool
	failure := fake.harness.pool.QueryRow(context.Background(), `select exists (select 1 from intended_processes
		where pid = $1::text::numeric and device_id = $2 and installation_id = $3 and action_kind = $4)`,
		pid.String(), node.DeviceID, node.InstallationID, actionKind).Scan(&found)
	return failure == nil && found
}

func (fake *fakeDawn) handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/dispatch", func(writer http.ResponseWriter, request *http.Request) {
		var body struct {
			Node dawn.NodeRef `json:"node"`
			Work []dawn.Work  `json:"work"`
		}
		if failure := json.NewDecoder(request.Body).Decode(&body); failure != nil {
			http.Error(writer, failure.Error(), http.StatusBadRequest)
			return
		}
		if len(body.Work) == 0 {
			http.Error(writer, "work holds no entry", http.StatusBadRequest)
			return
		}
		accepted := []campaign.Pid{}
		for _, work := range body.Work {
			identifier, failure := uuid.Parse(*work.CampaignID)
			if failure != nil || work.Pid != campaign.DerivePid(identifier, body.Node.DeviceID, body.Node.InstallationID, *work.Attempt) || !fake.intended(work.Pid, body.Node, work.Kind) {
				http.Error(writer, "the pid is not the intended process of this campaign attempt", http.StatusForbidden)
				return
			}
			accepted = append(accepted, work.Pid)
		}
		fake.mutex.Lock()
		fake.dispatches = append(fake.dispatches, dispatchRecord{node: body.Node, work: body.Work})
		fake.mutex.Unlock()
		writer.WriteHeader(http.StatusAccepted)
		_ = json.NewEncoder(writer).Encode(map[string]any{"accepted": accepted})
		go fake.run(body.Node, body.Work)
	})
	mux.HandleFunc("POST /v1/reap", func(writer http.ResponseWriter, request *http.Request) {
		var body struct {
			Node dawn.NodeRef   `json:"node"`
			Pids []campaign.Pid `json:"pids"`
		}
		if failure := json.NewDecoder(request.Body).Decode(&body); failure != nil || len(body.Pids) == 0 {
			http.Error(writer, "a reap names pids", http.StatusBadRequest)
			return
		}
		fake.mutex.Lock()
		fake.reaps[body.Node.DeviceID] = append(fake.reaps[body.Node.DeviceID], body.Pids...)
		for _, pid := range body.Pids {
			delete(fake.processes[body.Node.DeviceID], pid)
		}
		fake.mutex.Unlock()
		writer.WriteHeader(http.StatusAccepted)
		go func() {
			for _, pid := range body.Pids {
				fake.result(body.Node, dawn.Work{Pid: pid, Kind: "reap"}, "reaped", false, nil, nil)
			}
		}()
	})
	mux.HandleFunc("POST /v1/facts", func(writer http.ResponseWriter, request *http.Request) {
		var body struct {
			Node dawn.NodeRef `json:"node"`
			Pid  campaign.Pid `json:"pid"`
		}
		_ = json.NewDecoder(request.Body).Decode(&body)
		if !fake.intended(body.Pid, body.Node, "collect_facts") {
			http.Error(writer, "the facts pid was not intended", http.StatusForbidden)
			return
		}
		fake.mutex.Lock()
		fake.facts++
		current := fake.nodes[body.Node.DeviceID]
		fake.mutex.Unlock()
		if current == nil {
			http.Error(writer, "unknown node", http.StatusNotFound)
			return
		}
		_ = json.NewEncoder(writer).Encode(map[string]any{
			"facts": map[string]any{
				"dusk.version": current.version, "dusk.target.os": "linux", "dusk.target.arch": "x86_64", "dusk.os.time_zone": "America/New_York",
				"dusk.os.linux.os_release.id": "debian", "dusk.os.nix.uname.release": "6.12.48-amd64", "dusk.device.id": "never stored",
			},
			"reported": map[string]any{"version_key": "dusk.version", "version": current.version, "config_hash": nil, "services": []string{"nightfall"}, "facts": nil},
		})
	})
	return mux
}

func (fake *fakeDawn) result(reference dawn.NodeRef, work dawn.Work, status string, delivered bool, reported map[string]any, errorText *string) {
	now := kafka.FormatTime(time.Now())
	var finished any = now
	if status == "started" {
		finished = nil
	}
	message := map[string]any{
		"schema": "dusk.process-results/v1", "id": kafka.NewMessageID(), "time": now, "pid": work.Pid.String(), "campaign_id": work.CampaignID,
		"attempt": work.Attempt, "device_id": reference.DeviceID, "installation_id": reference.InstallationID, "namespace_id": reference.NamespaceID,
		"action_kind": work.Kind, "status": status, "delivered": delivered, "error": errorText, "started_at": now, "finished_at": finished,
		"dawn_instance": "dawn-0", "output_digest": strings.Repeat("ab", 32), "output_count": 0, "output_truncated": false, "reported": reported,
	}
	encoded, _ := json.Marshal(message)
	if failure := fake.harness.validator.Validate(kafka.ContractProcessResults, encoded); failure != nil {
		fake.harness.test.Errorf("the fake dawn made an invalid process result: %v\n%s", failure, encoded)
		return
	}
	fake.harness.produce(fake.harness.settings.Kafka.Topics.ProcessResults, reference.DeviceID+"/"+reference.InstallationID, encoded)
}

func (fake *fakeDawn) run(reference dawn.NodeRef, work []dawn.Work) {
	for _, item := range work {
		fake.mutex.Lock()
		if fake.processes[reference.DeviceID] == nil {
			fake.processes[reference.DeviceID] = map[campaign.Pid]bool{}
		}
		duplicate := fake.processes[reference.DeviceID][item.Pid]
		fake.processes[reference.DeviceID][item.Pid] = true
		failure, failing := fake.fail[reference.DeviceID]
		current := fake.nodes[reference.DeviceID]
		if item.Kind == "ensure_version" && !failing && !duplicate && current != nil {
			current.version = *item.DesiredVersion
		}
		fake.mutex.Unlock()
		if duplicate {
			var reported map[string]any
			if item.Kind == "ensure_version" && current != nil {
				reported = map[string]any{"version_key": *item.VersionKey, "version": current.version, "config_hash": nil, "services": nil, "facts": nil}
			}
			fake.result(reference, item, "duplicate", false, reported, nil)
			continue
		}
		fake.result(reference, item, "started", true, nil, nil)
		for _, path := range item.CollectFiles {
			step := item
			step.Kind = "collect_file"
			if path == "/missing" {
				missing := "no such file: " + path
				fake.result(reference, step, "error", true, nil, &missing)
				continue
			}
			fake.result(reference, step, "succeeded", true, nil, nil)
		}
		if failing {
			fake.result(reference, item, "failed", true, nil, &failure)
			continue
		}
		var reported map[string]any
		if item.Kind == "ensure_version" {
			reported = map[string]any{"version_key": *item.VersionKey, "version": *item.DesiredVersion, "config_hash": nil, "services": nil, "facts": nil}
		}
		fake.result(reference, item, "succeeded", true, reported, nil)
	}
}

func (current *harness) produce(topic, key string, value []byte) {
	current.test.Helper()
	operation, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if failure := current.producer.ProduceSync(operation, &kgo.Record{Topic: topic, Key: []byte(key), Value: value}).FirstErr(); failure != nil {
		current.test.Errorf("produce to %s: %v", topic, failure)
	}
}

func (current *harness) produceJSON(topic, key, contract string, message map[string]any) {
	current.test.Helper()
	encoded, _ := json.Marshal(message)
	if failure := current.validator.Validate(contract, encoded); failure != nil {
		current.test.Fatalf("test message fails %s: %v\n%s", contract, failure, encoded)
	}
	current.produce(topic, key, encoded)
}

func newHarness(test *testing.T) *harness {
	pool, url := testsupport.Database(test)
	brokers := testsupport.Brokers(test)
	current := &harness{test: test, pool: pool}
	suffix := strings.ToLower(strings.ReplaceAll(uuid.NewString()[:8], "-", ""))
	settings := config.Default()
	settings.Instance = "twilight-test"
	settings.Database.URL, settings.Database.LeaderURL = url, url
	settings.Kafka.Brokers = []string{brokers}
	settings.Kafka.AllowPlaintext = true
	settings.Kafka.Topics = config.Topics{
		Connections: "dusk.connections." + suffix, Census: "dusk.census." + suffix, Ledger: "dusk.ledger." + suffix,
		Enrollments: "dusk.enrollments." + suffix, NodeState: "dusk.node-state." + suffix, ProcessResults: "dusk.process-results." + suffix,
	}
	settings.Kafka.ResultsGroup += "-" + suffix
	settings.Kafka.InventoryGroup += "-" + suffix
	settings.Engine.SweepIntervalSeconds = 5
	settings.Engine.GateIntervalSeconds = 1
	settings.Engine.PresenceFlushMillis = 200
	settings.Engine.CountersFlushSeconds = 1
	settings.Engine.LastSeenBucketSeconds = 1
	settings.Dawn.AllowPlaintext = true
	settings.Dawn.RequestTimeoutSeconds = 5
	current.settings = settings
	options, failure := kafka.Options(settings.Kafka, "test")
	if failure != nil {
		test.Fatal(failure)
	}
	current.producer, failure = kgo.NewClient(options...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(current.producer.Close)
	admin := kadm.NewClient(current.producer)
	operation := context.Background()
	compact := "compact"
	remove := "delete"
	for topic, layout := range map[string]struct {
		partitions int32
		policy     *string
	}{
		settings.Kafka.Topics.Connections: {3, &remove}, settings.Kafka.Topics.Census: {1, &compact}, settings.Kafka.Topics.Ledger: {3, &remove},
		settings.Kafka.Topics.Enrollments: {3, &remove}, settings.Kafka.Topics.NodeState: {1, &compact}, settings.Kafka.Topics.ProcessResults: {3, &remove},
	} {
		if _, failure := admin.CreateTopic(operation, layout.partitions, 1, map[string]*string{"cleanup.policy": layout.policy}, topic); failure != nil {
			test.Fatal(failure)
		}
	}
	current.validator, failure = kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	current.fake = &fakeDawn{harness: current, nodes: map[string]*node{}, processes: map[string]map[campaign.Pid]bool{}, reaps: map[string][]campaign.Pid{}, fail: map[string]string{}}
	server := httptest.NewServer(current.fake.handler())
	test.Cleanup(server.Close)
	current.settings.Dawn.Endpoints = []string{strings.TrimPrefix(server.URL, "http://")}
	return current
}

func (current *harness) start() {
	logger := testsupport.Logger()
	dawnClient, failure := dawn.New(current.settings.Dawn, nil, logger)
	if failure != nil {
		current.test.Fatal(failure)
	}
	options, _ := kafka.Options(current.settings.Kafka, current.settings.Instance)
	nodeStateClient, failure := kgo.NewClient(options...)
	if failure != nil {
		current.test.Fatal(failure)
	}
	current.test.Cleanup(nodeStateClient.Close)
	current.engine = engine.New(engine.Dependencies{
		Config:       current.settings,
		Pool:         current.pool,
		Campaigns:    campaign.NewStore(current.pool),
		Inventory:    inventory.NewStore(current.pool),
		Alerts:       alerts.NewStore(current.pool, "", logger),
		Dawn:         dawnClient,
		KafkaOptions: options,
		Validator:    current.validator,
		NodeState:    kafka.NewNodeStateProducer(nodeStateClient, current.validator, current.settings.Kafka.Topics.NodeState),
		Logger:       logger,
	})
	operation, cancel := context.WithCancel(context.Background())
	current.cancel = cancel
	current.stopped = make(chan struct{})
	go func() {
		defer close(current.stopped)
		current.engine.Run(operation)
	}()
	current.test.Cleanup(current.stop)
}

func (current *harness) stop() {
	if current.cancel == nil {
		return
	}
	current.cancel()
	current.cancel = nil
	select {
	case <-current.stopped:
	case <-time.After(30 * time.Second):
		_ = pprof.Lookup("goroutine").WriteTo(os.Stderr, 1)
		current.test.Error("the engine did not stop within 30 s")
	}
}

func (current *harness) eventually(what string, timeout time.Duration, check func() (bool, string)) {
	current.test.Helper()
	deadline := time.Now().Add(timeout)
	last := ""
	for time.Now().Before(deadline) {
		done, detail := check()
		if done {
			return
		}
		last = detail
		time.Sleep(200 * time.Millisecond)
	}
	current.test.Fatalf("%s did not happen within %s; last: %s", what, timeout, last)
}

func identity(index int) node {
	return node{device: fmt.Sprintf("%032x", 0xd0000+index), installation: fmt.Sprintf("%032x", 0x10000+index), namespace: fmt.Sprintf("%016x", 0xa000+index), epoch: time.Now().UnixMicro() + int64(index), version: "0.1.0"}
}

func (current *harness) enroll(target node) {
	now := kafka.FormatTime(time.Now())
	fingerprint, hash := strings.Repeat("c", 64), strings.Repeat("d", 64)
	current.produceJSON(current.settings.Kafka.Topics.Enrollments, target.device+"/"+target.installation, kafka.ContractEnrollments, map[string]any{
		"schema": "dusk.enrollments/v1", "id": kafka.NewMessageID(), "time": now, "operation": "enroll", "outcome": "issued", "reason": nil,
		"device_id": target.device, "installation_id": target.installation, "tenant": nil, "credential_kind": "fleet_token", "credential_ref": "test",
		"hardware_fingerprint_hash": hash, "remote_address": "203.0.113.5:40000", "cert_serial": "01", "cert_fingerprint": fingerprint, "cert_not_after": now,
		"dusk_version": target.version, "impl": "nix", "target_os": "linux", "target_arch": "x86_64", "hostname": "node", "instance": "nightfall-0",
	})
}

func (current *harness) connect(target node, event string) {
	now := kafka.FormatTime(time.Now())
	var reason any
	if event == "disconnected" {
		reason = "node_closed"
	}
	current.produceJSON(current.settings.Kafka.Topics.Connections, target.device+"/"+target.installation+"/"+target.namespace, kafka.ContractConnections, map[string]any{
		"schema": "dusk.connections/v1", "id": kafka.NewMessageID(), "time": now, "event": event, "device_id": target.device, "installation_id": target.installation,
		"namespace_id": target.namespace, "epoch": target.epoch, "instance": "nightfall-0", "inner_address": "nightfall-0.nightfall-inner:8444",
		"remote_address": "203.0.113.5:40000", "tenant": nil, "cert_fingerprint": strings.Repeat("c", 64), "cert_not_after": now, "connected_at": now,
		"disconnect_reason": reason,
	})
}

func (current *harness) census(targets ...node) {
	now := time.Now()
	generation := now.UnixMicro()
	sessions := []map[string]any{}
	for _, target := range targets {
		sessions = append(sessions, map[string]any{"device_id": target.device, "installation_id": target.installation, "namespace_id": target.namespace,
			"epoch": target.epoch, "connected_at": kafka.FormatTime(now.Add(-time.Minute)), "last_seen": kafka.FormatTime(now), "tenant": nil})
	}
	topic := current.settings.Kafka.Topics.Census
	current.produceJSON(topic, fmt.Sprintf("nightfall-0/%d/0", generation), kafka.ContractCensus, map[string]any{
		"schema": "dusk.census/v1", "id": kafka.NewMessageID(), "time": kafka.FormatTime(now), "record": "chunk", "instance": "nightfall-0",
		"generation": generation, "index": 0, "sessions": sessions,
	})
	current.produceJSON(topic, "nightfall-0/header", kafka.ContractCensus, map[string]any{
		"schema": "dusk.census/v1", "id": kafka.NewMessageID(), "time": kafka.FormatTime(now), "record": "header", "instance": "nightfall-0",
		"inner_address": "nightfall-0.nightfall-inner:8444", "relay_address": "nightfall-0.nightfall-inner:8445", "generation": generation,
		"snapshot_epoch": now.UnixMicro() + 1000000, "started_at": kafka.FormatTime(now.Add(-time.Hour)), "full": true, "chunk_count": 1,
		"session_count": len(targets), "heartbeat_seconds": 600,
	})
}

func (current *harness) rows(identifier uuid.UUID) map[campaign.NodeState]int {
	counts := map[campaign.NodeState]int{}
	rows, failure := current.pool.Query(context.Background(), `select state, count(*) from campaign_nodes where campaign_id = $1 group by state`, identifier)
	if failure != nil {
		return counts
	}
	defer rows.Close()
	for rows.Next() {
		var state campaign.NodeState
		var count int
		_ = rows.Scan(&state, &count)
		counts[state] = count
	}
	return counts
}

func TestEngineEndToEnd(test *testing.T) {
	current := newHarness(test)
	first, second, third, offline := identity(1), identity(2), identity(3), identity(4)
	for _, target := range []node{first, second, third, offline} {
		copied := target
		current.fake.nodes[target.device] = &copied
		current.enroll(target)
	}
	current.census(first, second)
	current.connect(third, "connected")
	current.start()
	service := current.engine.Service()
	current.eventually("the online view loads the census and replays the connection produced before twilight started", time.Minute, func() (bool, string) {
		return current.engine.Ready() && current.engine.View().OnlineCount() == 3, fmt.Sprintf("ready %v online %d", current.engine.Ready(), current.engine.View().OnlineCount())
	})
	current.eventually("enrollments, presence and facts reach inventory", time.Minute, func() (bool, string) {
		var active, withCountry int
		_ = current.pool.QueryRow(context.Background(), `select count(*) filter (where lifecycle = 'active'), count(*) filter (where country = 'US') from nodes`).Scan(&active, &withCountry)
		return active == 3 && withCountry == 3, fmt.Sprintf("active %d with country %d", active, withCountry)
	})
	var leaked int
	_ = current.pool.QueryRow(context.Background(), `select count(*) from nodes where facts ? 'dusk.device.id'`).Scan(&leaked)
	if leaked != 0 {
		test.Fatal("dusk.device.id reached inventory")
	}
	var presenceRows int
	_ = current.pool.QueryRow(context.Background(), `select count(*) from node_presence where online`).Scan(&presenceRows)
	if presenceRows != 3 {
		test.Fatalf("%d online presence rows", presenceRows)
	}

	definition := campaign.Definition{
		Name:     "upgrade to 0.2.0",
		Selector: `country == "US" and dusk_version < "0.2.0"`,
		Action:   campaign.Action{Kind: campaign.KindEnsureVersion, Version: "0.2.0", Script: "upgrade"},
		Policy: campaign.Policy{
			Rate:               campaign.Rate{PerSecond: 50, Burst: 50},
			Phases:             []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 5}},
			NodeTimeoutSeconds: 5,
			Gates:              campaign.Gates{MinSample: 1},
		},
	}
	upgrade, failure := service.CreateCampaign(context.Background(), definition, "operator@example.org")
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), upgrade.ID, "operator@example.org"); failure != nil {
		test.Fatal(failure)
	}
	current.eventually("every online node converges", 90*time.Second, func() (bool, string) {
		counts := current.rows(upgrade.ID)
		return counts[campaign.StateSucceeded] == 3, fmt.Sprint(counts)
	})
	var intendedRecorded, versions int
	_ = current.pool.QueryRow(context.Background(), `select count(*) from intended_processes where campaign_id = $1 and action_kind = 'ensure_version' and attempt = 1
		and max_commands = 1 and default_shell_commands = 8`, upgrade.ID).Scan(&intendedRecorded)
	_ = current.pool.QueryRow(context.Background(), `select count(*) from nodes where reported_version = '0.2.0'`).Scan(&versions)
	if intendedRecorded != 3 || versions != 3 {
		test.Fatalf("%d intended processes, %d nodes report 0.2.0", intendedRecorded, versions)
	}
	gates, failure := service.CampaignGates(context.Background(), upgrade.ID)
	if failure != nil || gates.Overall.Succeeded != 3 {
		test.Fatalf("gates %+v %v", gates, failure)
	}
	current.eventually("counters are flushed", 30*time.Second, func() (bool, string) {
		overview, failure := service.Overview(context.Background())
		if failure != nil || len(overview.Campaigns) == 0 {
			return false, fmt.Sprint(failure)
		}
		for _, counter := range overview.Campaigns[0].Counters {
			if counter.State == campaign.StateSucceeded && counter.Count == 3 {
				return true, ""
			}
		}
		return false, fmt.Sprint(overview.Campaigns[0].Counters)
	})
	time.Sleep(6 * time.Second)
	still, _ := service.Campaign(context.Background(), upgrade.ID)
	if still.Status != campaign.StatusRunning {
		test.Fatalf("an ensure_version campaign must keep converging, not %s", still.Status)
	}

	current.fake.mutex.Lock()
	current.fake.fail[second.device] = "the script exited 3"
	current.fake.mutex.Unlock()
	script := campaign.Definition{
		Name:     "collect diagnostics",
		Selector: `has(device_id)`,
		Action:   campaign.Action{Kind: campaign.KindRunScript, Script: "ps", CollectFiles: []string{"/var/log/syslog", "/missing"}},
		Policy: campaign.Policy{
			Rate:               campaign.Rate{PerSecond: 50, Burst: 50},
			Phases:             []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 2}},
			NodeTimeoutSeconds: 2,
			Gates:              campaign.Gates{MinSample: 2, MaxFailureRate: 0.5, MaxSilentRate: 1},
		},
	}
	diagnostics, failure := service.CreateCampaign(context.Background(), script, "operator@example.org")
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), diagnostics.ID, "operator@example.org"); failure != nil {
		test.Fatal(failure)
	}
	current.eventually("the run_script campaign completes", 90*time.Second, func() (bool, string) {
		loaded, _ := service.Campaign(context.Background(), diagnostics.ID)
		return loaded.Status == campaign.StatusCompleted, fmt.Sprintf("%s %v", loaded.Status, current.rows(diagnostics.ID))
	})
	counts := current.rows(diagnostics.ID)
	if counts[campaign.StateSucceeded] != 2 || counts[campaign.StateFailed] != 1 {
		test.Fatalf("run_script rows %v", counts)
	}
	failedRow, _ := campaign.NewStore(current.pool).Row(context.Background(), diagnostics.ID, second.device, second.installation)
	if failedRow.LastError != "the script exited 3" || failedRow.LastStatus != "failed" {
		test.Fatalf("a failed script whose files were collected must stay failed: %+v", failedRow)
	}
	var firstAttempts, intendedAttempts int
	_ = current.pool.QueryRow(context.Background(), `select count(*) from campaign_nodes where campaign_id = $1 and attempt = 1`, diagnostics.ID).Scan(&firstAttempts)
	_ = current.pool.QueryRow(context.Background(), `select count(*) from intended_processes where campaign_id = $1`, diagnostics.ID).Scan(&intendedAttempts)
	if firstAttempts != 3 || intendedAttempts != 3 {
		test.Fatalf("a script that succeeded but whose file was missing ran again: %d rows on their first attempt, %d intended processes", firstAttempts, intendedAttempts)
	}
	events, _ := service.CampaignEvents(context.Background(), diagnostics.ID, 0, 100)
	kinds := []string{}
	for _, event := range events {
		if event.Kind != "gate_holding" {
			kinds = append(kinds, event.Kind)
		}
	}
	if strings.Join(kinds, ",") != "created,started,completed" {
		test.Fatalf("events %v", kinds)
	}

	quarantine := campaign.Definition{
		Name:     "isolate the offline node",
		Selector: fmt.Sprintf(`device_id == "%s"`, offline.device),
		Action:   campaign.Action{Kind: campaign.KindQuarantine},
		Policy:   campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, Gates: campaign.Gates{MinSample: 1}},
	}
	isolate, failure := service.CreateCampaign(context.Background(), quarantine, "operator@example.org")
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), isolate.ID, "operator@example.org"); failure != nil {
		test.Fatal(failure)
	}
	current.eventually("the offline node is quarantined", 90*time.Second, func() (bool, string) {
		var lifecycle string
		_ = current.pool.QueryRow(context.Background(), `select lifecycle from nodes where device_id = $1`, offline.device).Scan(&lifecycle)
		counts := current.rows(isolate.ID)
		return lifecycle == "quarantined" && counts[campaign.StateSucceeded] == 1, fmt.Sprintf("%s %v", lifecycle, counts)
	})
	consumer, _ := kgo.NewClient(append(must(kafka.Options(current.settings.Kafka, "check")), kgo.ConsumeTopics(current.settings.Kafka.Topics.NodeState), kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))...)
	defer consumer.Close()
	polling, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	fetches := consumer.PollRecords(polling, 1)
	records := fetches.Records()
	if len(records) != 1 || string(records[0].Key) != "installation/"+offline.device+"/"+offline.installation {
		test.Fatalf("node-state records %d", len(records))
	}
	if failure := current.validator.Validate(kafka.ContractNodeState, records[0].Value); failure != nil {
		test.Fatal(failure)
	}

	paused, failure := service.PauseCampaign(context.Background(), upgrade.ID, "operator@example.org", "maintenance window")
	if failure != nil || paused.Status != campaign.StatusPaused {
		test.Fatalf("pause %+v %v", paused, failure)
	}
	resumed, failure := service.ResumeCampaign(context.Background(), upgrade.ID, "operator@example.org", false, "")
	if failure != nil || resumed.Status != campaign.StatusRunning {
		test.Fatalf("resume %+v %v", resumed, failure)
	}
	validation, failure := service.ValidateSelector(context.Background(), `country == "US"`)
	if failure != nil || !validation.OK || validation.Matched != 3 {
		test.Fatalf("validation %+v %v", validation, failure)
	}
	broken, _ := service.ValidateSelector(context.Background(), `country = "US"`)
	if broken.OK || broken.Error == nil || broken.Error.Position != 8 {
		test.Fatalf("broken selector %+v", broken)
	}
	detail, failure := service.Node(context.Background(), inventory.Key{DeviceID: first.device, InstallationID: first.installation})
	if failure != nil || !detail.Online || len(detail.Sessions) != 1 || len(detail.Executions) != 2 {
		test.Fatalf("node detail %+v %v", detail, failure)
	}
	session, failure := service.OpenSession(context.Background(), inventory.Key{DeviceID: first.device, InstallationID: first.installation}, "debugging", 600, "operator@example.org")
	if failure != nil || session.Node.NamespaceID != first.namespace || campaign.Reserved(session.Pid) {
		test.Fatalf("session %+v %v", session, failure)
	}
	var sessionCommands int
	if failure := current.pool.QueryRow(context.Background(), `select max_commands from intended_processes where pid = $1::text::numeric and action_kind = 'interactive'
		and principal = 'operator@example.org' and campaign_id is null and attempt is null`, session.Pid.String()).Scan(&sessionCommands); failure != nil || sessionCommands != current.settings.Reconcile.CommandsPerSession {
		test.Fatalf("the interactive session's intended process: %d %v", sessionCommands, failure)
	}
	if _, failure := service.OpenSession(context.Background(), inventory.Key{DeviceID: offline.device, InstallationID: offline.installation}, "debugging", 600, "operator@example.org"); failure != engine.ErrNodeOffline {
		test.Fatalf("a session to an offline node: %v", failure)
	}
	page, failure := service.Nodes(context.Background(), "", new(true), "", "", 2)
	if failure != nil || len(page.Nodes) != 2 || page.Next == "" {
		test.Fatalf("online nodes page %+v %v", page, failure)
	}
	rest, failure := service.Nodes(context.Background(), "", new(true), "", page.Next, 2)
	if failure != nil || len(rest.Nodes) != 1 {
		test.Fatalf("second online page %+v %v", rest, failure)
	}

	current.connect(third, "disconnected")
	current.eventually("the disconnect is recorded for the silent-rate gate", time.Minute, func() (bool, string) {
		var marked int
		_ = current.pool.QueryRow(context.Background(), `select count(*) from campaign_nodes where campaign_id = $1 and device_id = $2 and event_at is not null`, upgrade.ID, third.device).Scan(&marked)
		return marked == 1 && current.engine.View().OnlineCount() == 2, fmt.Sprintf("marked %d online %d", marked, current.engine.View().OnlineCount())
	})
}

func must[Value any](value Value, failure error) Value {
	if failure != nil {
		panic(failure)
	}
	return value
}

func TestWithdrawnNodesKeepTheirLifecycle(test *testing.T) {
	current := newHarness(test)
	stolen, suspect := identity(41), identity(42)
	for _, target := range []node{stolen, suspect} {
		current.enroll(target)
	}
	current.census()
	current.start()
	service := current.engine.Service()
	stolenKey := inventory.Key{DeviceID: stolen.device, InstallationID: stolen.installation}
	current.eventually("both enrollments reach inventory", time.Minute, func() (bool, string) {
		var enrolled int
		_ = current.pool.QueryRow(context.Background(), `select count(*) from nodes where device_id = any($1)`, []string{stolen.device, suspect.device}).Scan(&enrolled)
		return enrolled == 2, fmt.Sprintf("%d enrolled", enrolled)
	})
	var refused *engine.LifecycleRefused
	if failure := service.SetLifecycle(context.Background(), stolenKey, "revoked", "stolen", "operator@example.org", false); !errors.As(failure, &refused) {
		test.Fatalf("an operator revoked a node: %v", failure)
	}
	if failure := service.SetLifecycle(context.Background(), stolenKey, "revoked", "stolen", "admin@example.org", true); failure != nil {
		test.Fatal(failure)
	}
	for _, lifecycle := range []string{"active", "quarantined"} {
		if failure := service.SetLifecycle(context.Background(), stolenKey, lifecycle, "found it", "operator@example.org", false); !errors.As(failure, &refused) || refused.From != "revoked" {
			test.Fatalf("an operator moved a revoked node to %s: %v", lifecycle, failure)
		}
	}
	quarantine := campaign.Definition{
		Name:     "isolate both",
		Selector: fmt.Sprintf(`device_id in ["%s", "%s"]`, stolen.device, suspect.device),
		Action:   campaign.Action{Kind: campaign.KindQuarantine},
		Policy:   campaign.Policy{Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}}, Gates: campaign.Gates{MinSample: 1}},
	}
	isolate, failure := service.CreateCampaign(context.Background(), quarantine, "operator@example.org")
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := service.StartCampaign(context.Background(), isolate.ID, "operator@example.org"); failure != nil {
		test.Fatal(failure)
	}
	current.eventually("both rows finish", 90*time.Second, func() (bool, string) {
		counts := current.rows(isolate.ID)
		return counts[campaign.StateSucceeded] == 2, fmt.Sprint(counts)
	})
	store := campaign.NewStore(current.pool)
	withdrawn, _ := store.Row(context.Background(), isolate.ID, stolen.device, stolen.installation)
	if withdrawn.LastStatus != "withdrawn" {
		test.Fatalf("the revoked node's row %+v", withdrawn)
	}
	lifecycles := map[string]string{}
	rows, _ := current.pool.Query(context.Background(), `select device_id, lifecycle from nodes where device_id = any($1)`, []string{stolen.device, suspect.device})
	for rows.Next() {
		var device, lifecycle string
		_ = rows.Scan(&device, &lifecycle)
		lifecycles[device] = lifecycle
	}
	rows.Close()
	if lifecycles[stolen.device] != "revoked" || lifecycles[suspect.device] != "quarantined" {
		test.Fatalf("lifecycles after the campaign: %v", lifecycles)
	}
	consumer, _ := kgo.NewClient(append(must(kafka.Options(current.settings.Kafka, "check")), kgo.ConsumeTopics(current.settings.Kafka.Topics.NodeState), kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))...)
	defer consumer.Close()
	polling, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	latest := map[string]string{}
	for len(latest) < 2 && polling.Err() == nil {
		consumer.PollRecords(polling, 10).EachRecord(func(record *kgo.Record) {
			var state struct {
				Lifecycle string `json:"lifecycle"`
			}
			_ = json.Unmarshal(record.Value, &state)
			latest[string(record.Key)] = state.Lifecycle
		})
	}
	if latest["installation/"+stolen.device+"/"+stolen.installation] != "revoked" || latest["installation/"+suspect.device+"/"+suspect.installation] != "quarantined" {
		test.Fatalf("the node-state records %v", latest)
	}
}
