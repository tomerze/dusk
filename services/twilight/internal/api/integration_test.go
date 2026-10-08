//go:build integration

package api

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
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
	"dusk/services/twilight/internal/tokens"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

type fleetNode struct {
	device, installation, namespace string
	epoch                           int64
}

type stack struct {
	test      *testing.T
	pool      *pgxpool.Pool
	settings  config.Config
	producer  *kgo.Client
	validator *kafka.Validator
	api       *harness
	address   string
	mutex     sync.Mutex
	failing   map[string]string
	dispatch  int
}

func (current *stack) produce(topic, key, contract string, message map[string]any) {
	current.test.Helper()
	encoded, _ := json.Marshal(message)
	if failure := current.validator.Validate(contract, encoded); failure != nil {
		current.test.Errorf("a test message fails %s: %v\n%s", contract, failure, encoded)
		return
	}
	producing, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if failure := current.producer.ProduceSync(producing, &kgo.Record{Topic: topic, Key: []byte(key), Value: encoded}).FirstErr(); failure != nil {
		current.test.Errorf("produce to %s: %v", topic, failure)
	}
}

func (current *stack) result(reference dawn.NodeRef, work dawn.Work, status string, errorText *string) {
	now := kafka.FormatTime(time.Now())
	var finished any = now
	if status == "started" {
		finished = nil
	}
	current.produce(current.settings.Kafka.Topics.ProcessResults, reference.Key(), kafka.ContractProcessResults, map[string]any{
		"schema": "dusk.process-results/v1", "id": kafka.NewMessageID(), "time": now, "pid": work.Pid.String(), "campaign_id": work.CampaignID,
		"attempt": work.Attempt, "device_id": reference.DeviceID, "installation_id": reference.InstallationID, "namespace_id": reference.NamespaceID,
		"action_kind": work.Kind, "status": status, "delivered": true, "error": errorText, "started_at": now, "finished_at": finished,
		"dawn_instance": "dawn-0", "output_digest": strings.Repeat("ab", 32), "output_count": 0, "output_truncated": false, "reported": nil,
	})
}

func (current *stack) intended(work dawn.Work, node dawn.NodeRef) bool {
	var found bool
	failure := current.pool.QueryRow(context.Background(), `select exists (select 1 from intended_processes where pid = $1::text::numeric
		and device_id = $2 and installation_id = $3 and campaign_id = $4::uuid and attempt = $5)`,
		work.Pid.String(), node.DeviceID, node.InstallationID, *work.CampaignID, *work.Attempt).Scan(&found)
	return failure == nil && found
}

func (current *stack) dawn() http.Handler {
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
		accepted := []campaign.Pid{}
		for _, work := range body.Work {
			identifier, failure := uuid.Parse(*work.CampaignID)
			if failure != nil || work.Pid != campaign.DerivePid(identifier, body.Node.DeviceID, body.Node.InstallationID, *work.Attempt) || !current.intended(work, body.Node) {
				current.test.Errorf("pid %s of attempt %d is not the derived pid of an intended process", work.Pid, *work.Attempt)
				http.Error(writer, "the pid was not intended", http.StatusForbidden)
				return
			}
			accepted = append(accepted, work.Pid)
		}
		current.mutex.Lock()
		current.dispatch++
		failing, fails := current.failing[body.Node.DeviceID]
		current.mutex.Unlock()
		writer.WriteHeader(http.StatusAccepted)
		_ = json.NewEncoder(writer).Encode(map[string]any{"accepted": accepted})
		go func() {
			for _, work := range body.Work {
				current.result(body.Node, work, "started", nil)
				if fails {
					current.result(body.Node, work, "failed", &failing)
				} else {
					current.result(body.Node, work, "succeeded", nil)
				}
			}
		}()
	})
	mux.HandleFunc("POST /v1/facts", func(writer http.ResponseWriter, request *http.Request) {
		_ = json.NewEncoder(writer).Encode(map[string]any{
			"facts": map[string]any{
				"dusk.version": "0.1.0", "dusk.target.os": "linux", "dusk.target.arch": "x86_64", "dusk.os.time_zone": "America/Chicago",
				"dusk.os.linux.os_release.id": "debian", "dusk.os.nix.uname.release": "6.12.48-amd64",
			},
			"reported": map[string]any{"version_key": "dusk.version", "version": "0.1.0", "config_hash": nil, "services": []string{"nightfall"}, "facts": nil},
		})
	})
	return mux
}

func newStack(test *testing.T) *stack {
	pool, url := testsupport.Database(test)
	brokers := testsupport.Brokers(test)
	current := &stack{test: test, pool: pool, failing: map[string]string{}}
	suffix := strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	settings := config.Default()
	settings.Instance = "twilight-api-test"
	settings.Database.URL, settings.Database.LeaderURL = url, url
	settings.Kafka.Brokers = []string{brokers}
	settings.Kafka.AllowPlaintext = true
	settings.Kafka.Topics = config.Topics{
		Connections: "dusk.connections." + suffix, Census: "dusk.census." + suffix, Ledger: "dusk.ledger." + suffix,
		Enrollments: "dusk.enrollments." + suffix, NodeState: "dusk.node-state." + suffix, ProcessResults: "dusk.process-results." + suffix,
	}
	settings.Kafka.ResultsGroup += "-" + suffix
	settings.Kafka.InventoryGroup += "-" + suffix
	settings.Engine.SweepIntervalSeconds = 2
	settings.Engine.GateIntervalSeconds = 1
	settings.Engine.PresenceFlushMillis = 200
	settings.Engine.CountersFlushSeconds = 1
	settings.Engine.LastSeenBucketSeconds = 1
	settings.Dawn.AllowPlaintext = true
	settings.Dawn.RequestTimeoutSeconds = 5
	options, failure := kafka.Options(settings.Kafka, settings.Instance)
	if failure != nil {
		test.Fatal(failure)
	}
	if current.producer, failure = kgo.NewClient(options...); failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(current.producer.Close)
	compact, remove := "compact", "delete"
	for topic, policy := range map[string]*string{
		settings.Kafka.Topics.Connections: &remove, settings.Kafka.Topics.Census: &compact, settings.Kafka.Topics.Ledger: &remove,
		settings.Kafka.Topics.Enrollments: &remove, settings.Kafka.Topics.NodeState: &compact, settings.Kafka.Topics.ProcessResults: &remove,
	} {
		if _, failure := kadm.NewClient(current.producer).CreateTopic(context.Background(), 1, 1, map[string]*string{"cleanup.policy": policy}, topic); failure != nil {
			test.Fatal(failure)
		}
	}
	if current.validator, failure = kafka.NewValidator(); failure != nil {
		test.Fatal(failure)
	}
	fakeDawn := httptest.NewServer(current.dawn())
	test.Cleanup(fakeDawn.Close)
	settings.Dawn.Endpoints = []string{strings.TrimPrefix(fakeDawn.URL, "http://")}
	current.settings = settings

	logger := testsupport.Logger()
	dawnClient, failure := dawn.New(settings.Dawn, nil, logger)
	if failure != nil {
		test.Fatal(failure)
	}
	nodeState, failure := kgo.NewClient(options...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(nodeState.Close)
	core := engine.New(engine.Dependencies{
		Config: settings, Pool: pool, Campaigns: campaign.NewStore(pool), Inventory: inventory.NewStore(pool), Alerts: alerts.NewStore(pool, "", logger),
		Dawn: dawnClient, KafkaOptions: options, Validator: current.validator,
		NodeState: kafka.NewNodeStateProducer(nodeState, current.validator, settings.Kafka.Topics.NodeState), Logger: logger,
	})
	current.api = newHarness(test, func(options *Options) {
		options.Backend = core.Service()
		options.Tokens = tokens.NewStore(pool)
		options.Sessions = NewPostgresSessions(pool)
		options.Development = true
		options.Now = time.Now
		options.Logger = logger
	})
	served := httptest.NewServer(current.api.server)
	test.Cleanup(served.Close)
	current.address = strings.TrimPrefix(served.URL, "http://")

	running, cancel := context.WithCancel(context.Background())
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		core.Run(running)
	}()
	test.Cleanup(func() {
		current.api.server.Drain()
		cancel()
		select {
		case <-stopped:
		case <-time.After(30 * time.Second):
			test.Error("the engine did not stop within 30 s")
		}
	})
	return current
}

func (current *stack) token(role string) string {
	current.test.Helper()
	_, secret, failure := tokens.NewStore(current.pool).Create(context.Background(), role+" for the API test", role, "test", time.Now())
	if failure != nil {
		current.test.Fatal(failure)
	}
	return secret
}

func (current *stack) eventually(what string, check func() (bool, string)) {
	current.test.Helper()
	deadline := time.Now().Add(90 * time.Second)
	last := ""
	for time.Now().Before(deadline) {
		done, detail := check()
		if done {
			return
		}
		last = detail
		time.Sleep(250 * time.Millisecond)
	}
	current.test.Fatalf("%s did not happen within 90 s; last: %s", what, last)
}

func (current *stack) enroll(target fleetNode) {
	now := kafka.FormatTime(time.Now())
	current.produce(current.settings.Kafka.Topics.Enrollments, target.device+"/"+target.installation, kafka.ContractEnrollments, map[string]any{
		"schema": "dusk.enrollments/v1", "id": kafka.NewMessageID(), "time": now, "operation": "enroll", "outcome": "issued", "reason": nil,
		"device_id": target.device, "installation_id": target.installation, "tenant": nil, "credential_kind": "fleet_token", "credential_ref": "test",
		"hardware_fingerprint_hash": strings.Repeat("d", 64), "remote_address": "203.0.113.5:40000", "cert_serial": "01", "cert_fingerprint": strings.Repeat("c", 64),
		"cert_not_after": now, "dusk_version": "0.1.0", "impl": "nix", "target_os": "linux", "target_arch": "x86_64", "hostname": "node", "instance": "nightfall-0",
	})
}

func (current *stack) census(targets []fleetNode) {
	now := time.Now()
	generation := now.UnixMicro()
	sessions := []map[string]any{}
	for _, target := range targets {
		sessions = append(sessions, map[string]any{"device_id": target.device, "installation_id": target.installation, "namespace_id": target.namespace,
			"epoch": target.epoch, "connected_at": kafka.FormatTime(now.Add(-time.Minute)), "last_seen": kafka.FormatTime(now), "tenant": nil})
	}
	topic := current.settings.Kafka.Topics.Census
	current.produce(topic, fmt.Sprintf("nightfall-0/%d/0", generation), kafka.ContractCensus, map[string]any{
		"schema": "dusk.census/v1", "id": kafka.NewMessageID(), "time": kafka.FormatTime(now), "record": "chunk", "instance": "nightfall-0",
		"generation": generation, "index": 0, "sessions": sessions,
	})
	current.produce(topic, "nightfall-0/header", kafka.ContractCensus, map[string]any{
		"schema": "dusk.census/v1", "id": kafka.NewMessageID(), "time": kafka.FormatTime(now), "record": "header", "instance": "nightfall-0",
		"inner_address": "nightfall-0.nightfall-inner:8444", "relay_address": "nightfall-0.nightfall-inner:8445", "generation": generation,
		"snapshot_epoch": now.UnixMicro() + 1000000, "started_at": kafka.FormatTime(now.Add(-time.Hour)), "full": true, "chunk_count": 1,
		"session_count": len(targets), "heartbeat_seconds": 600,
	})
}

func object(result response) map[string]any {
	decoded, _ := result.decoded.(map[string]any)
	return decoded
}

func campaignEvents(result response) ([]string, map[string]map[string]any) {
	var kinds []string
	details := map[string]map[string]any{}
	for _, item := range items(result) {
		event := item.(map[string]any)
		kind := event["kind"].(string)
		if kind == "gate_holding" {
			continue
		}
		kinds = append(kinds, kind)
		details[kind], _ = event["detail"].(map[string]any)
	}
	return kinds, details
}

func TestACampaignThroughTheAPI(test *testing.T) {
	current := newStack(test)
	operator, viewer := current.token("operator"), current.token("viewer")
	var fleet []fleetNode
	for index := range 4 {
		target := fleetNode{device: fmt.Sprintf("%032x", 0xa9100+index), installation: fmt.Sprintf("%032x", 0x19100+index), namespace: fmt.Sprintf("%016x", 0xb100+index),
			epoch: time.Now().UnixMicro() + int64(index)}
		fleet = append(fleet, target)
		current.enroll(target)
	}
	current.census(fleet)
	broken := fleet[3]
	current.mutex.Lock()
	current.failing[broken.device] = "the script exited 3"
	current.mutex.Unlock()

	api := current.api
	current.eventually("the fleet is online and leads", func() (bool, string) {
		result := api.call(http.MethodGet, "/api/v1/overview", nil, viewer)
		nodes, _ := object(result)["nodes"].(map[string]any)
		return result.Code == http.StatusOK && nodes["online"] == float64(4) && object(result)["leader"] == true, result.Body.String()
	})
	selector := `country == "US" and os_name == "debian"`
	current.eventually("facts reach the inventory", func() (bool, string) {
		result := api.call(http.MethodGet, "/api/v1/nodes?online=true&selector="+url.QueryEscape(selector), nil, viewer)
		return result.Code == http.StatusOK && len(items(result)) == 4, result.Body.String()
	})
	validation := api.call(http.MethodPost, "/api/v1/selectors/validate", map[string]any{"selector": selector}, viewer)
	if validation.Code != http.StatusOK || field(validation, "ok") != true || field(validation, "matched") != float64(4) {
		test.Fatalf("selector validation: %d %s", validation.Code, validation.Body.String())
	}
	invalidSelector := api.call(http.MethodPost, "/api/v1/selectors/validate", map[string]any{"selector": `country = "US"`}, viewer)
	if invalidSelector.Code != http.StatusOK || field(invalidSelector, "ok") != false {
		test.Fatalf("an invalid selector: %d %s", invalidSelector.Code, invalidSelector.Body.String())
	}

	definition := map[string]any{
		"name":     "collect diagnostics",
		"selector": selector,
		"action":   map[string]any{"kind": "run_script", "script": "ps"},
		"policy": map[string]any{
			"rate":                 map[string]any{"per_second": 50, "burst": 50},
			"phases":               []any{map[string]any{"name": "everyone", "percent": 100, "bake_seconds": 2}},
			"node_timeout_seconds": 2,
			"gates":                map[string]any{"min_sample": 4, "max_failure_rate": 0.2, "max_silent_rate": 1, "silent_window_seconds": 0},
			"abort":                map[string]any{"on_gate_failure": "pause"},
		},
	}
	if refused := api.call(http.MethodPost, "/api/v1/campaigns", definition, viewer); refused.Code != http.StatusForbidden {
		test.Fatalf("a viewer created a campaign: %d", refused.Code)
	}
	created := api.call(http.MethodPost, "/api/v1/campaigns", definition, operator)
	if created.Code != http.StatusCreated || field(created, "status") != "draft" {
		test.Fatalf("create: %d %s", created.Code, created.Body.String())
	}
	identifier := field(created, "id").(string)
	if created.Header().Get("Location") != "/api/v1/campaigns/"+identifier {
		test.Fatalf("Location %q", created.Header().Get("Location"))
	}
	listed := api.call(http.MethodGet, "/api/v1/campaigns?status=draft", nil, viewer)
	if listed.Code != http.StatusOK || len(items(listed)) != 1 {
		test.Fatalf("the draft list: %d %s", listed.Code, listed.Body.String())
	}

	login := newRequest(http.MethodGet, LoginPath+"?return_to=/campaigns/"+identifier, nil, "")
	login.RemoteAddr = "127.0.0.1:50000"
	loggedIn := api.do(login)
	if loggedIn.Code != http.StatusSeeOther {
		test.Fatalf("the development login: %d %s", loggedIn.Code, loggedIn.Body.String())
	}
	var sessionCookie, csrfCookie *http.Cookie
	for _, cookie := range loggedIn.Result().Cookies() {
		switch cookie.Name {
		case SessionCookie:
			sessionCookie = cookie
		case CSRFCookie:
			csrfCookie = cookie
		}
	}
	start := func(csrf string) response {
		request := newRequest(http.MethodPost, "/api/v1/campaigns/"+identifier+"/start", nil, "")
		request.AddCookie(sessionCookie)
		request.AddCookie(csrfCookie)
		if csrf != "" {
			request.Header.Set(CSRFHeader, csrf)
		}
		return api.do(request)
	}
	if refused := start(""); refused.Code != http.StatusForbidden || errorCode(refused) != "csrf_failed" {
		test.Fatalf("a cookie session started a campaign without its CSRF token: %d %s", refused.Code, refused.Body.String())
	}
	started := start(csrfCookie.Value)
	if started.Code != http.StatusOK || field(started, "status") != "running" {
		test.Fatalf("start: %d %s", started.Code, started.Body.String())
	}

	campaignPath := "/api/v1/campaigns/" + identifier
	current.eventually("the failure rate gate pauses the campaign", func() (bool, string) {
		result := api.call(http.MethodGet, campaignPath, nil, viewer)
		return field(result, "status") == "paused", result.Body.String()
	})
	gates := api.call(http.MethodGet, campaignPath+"/gates", nil, viewer)
	overall, _ := field(gates, "overall").(map[string]any)
	if gates.Code != http.StatusOK || field(gates, "verdict") != "fail" || field(gates, "reason") != "failure rate 0.25 overall, 1 of 4" ||
		field(gates, "required") != float64(4) || overall["succeeded"] != float64(3) || overall["failed"] != float64(1) {
		test.Fatalf("gates: %d %s", gates.Code, gates.Body.String())
	}
	failed := api.call(http.MethodGet, campaignPath+"/nodes?state=failed", nil, viewer)
	rows := items(failed)
	if failed.Code != http.StatusOK || len(rows) != 1 || rows[0].(map[string]any)["device_id"] != broken.device || rows[0].(map[string]any)["last_error"] != "the script exited 3" {
		test.Fatalf("failed rows: %d %s", failed.Code, failed.Body.String())
	}
	succeeded := api.call(http.MethodGet, campaignPath+"/nodes?state=succeeded&limit=2", nil, viewer)
	next, _ := field(succeeded, "next_cursor").(string)
	rest := api.call(http.MethodGet, campaignPath+"/nodes?state=succeeded&limit=2&cursor="+url.QueryEscape(next), nil, viewer)
	if len(items(succeeded)) != 2 || next == "" || len(items(rest)) != 1 {
		test.Fatalf("succeeded rows by page: %s then %s", succeeded.Body.String(), rest.Body.String())
	}
	kinds, details := campaignEvents(api.call(http.MethodGet, campaignPath+"/events", nil, viewer))
	if strings.Join(kinds, ",") != "created,started,paused" || details["paused"]["reason"] != "failure rate 0.25 overall, 1 of 4" || details["paused"]["pause_kind"] != "gate" {
		test.Fatalf("events before the resume: %v %v", kinds, details)
	}

	if refused := api.call(http.MethodPost, campaignPath+"/resume", map[string]any{"reason": "looks fine"}, operator); refused.Code != http.StatusConflict || errorCode(refused) != "gate_override_required" {
		test.Fatalf("a gate pause resumed without an override: %d %s", refused.Code, refused.Body.String())
	}
	current.mutex.Lock()
	delete(current.failing, broken.device)
	current.mutex.Unlock()
	retried := api.call(http.MethodPost, campaignPath+"/nodes/retry", map[string]any{"states": []string{"failed"}, "reason": "the script is fixed"}, operator)
	if retried.Code != http.StatusOK || field(retried, "count") != float64(1) {
		test.Fatalf("retry: %d %s", retried.Code, retried.Body.String())
	}
	resumed := api.call(http.MethodPost, campaignPath+"/resume", map[string]any{"override_gate": true, "reason": "the failure was the script, now fixed"}, operator)
	if resumed.Code != http.StatusOK || field(resumed, "status") != "running" {
		test.Fatalf("resume with an override: %d %s", resumed.Code, resumed.Body.String())
	}
	current.eventually("the retried node succeeds", func() (bool, string) {
		result := api.call(http.MethodGet, campaignPath+"/nodes?state=succeeded", nil, viewer)
		return len(items(result)) == 4, result.Body.String()
	})
	parsedCampaign := uuid.MustParse(identifier)
	for _, row := range items(api.call(http.MethodGet, campaignPath+"/nodes", nil, viewer)) {
		fields := row.(map[string]any)
		attempt := int(fields["attempt"].(float64))
		if fields["device_id"] == broken.device && attempt != 2 {
			test.Fatalf("the retried row: %v", fields)
		}
		if fields["pid"] != campaign.DerivePid(parsedCampaign, fields["device_id"].(string), fields["installation_id"].(string), attempt).String() {
			test.Fatalf("a row's pid is not derived from its campaign, node and attempt: %v", fields)
		}
	}
	current.eventually("the gate counts only what ran after the override", func() (bool, string) {
		result := api.call(http.MethodGet, campaignPath+"/gates", nil, viewer)
		return field(result, "verdict") == "hold" && field(result, "reason") == "waiting for sample 1 of 4", result.Body.String()
	})
	completed := api.call(http.MethodPost, campaignPath+"/complete", map[string]any{"reason": "every node ran it"}, operator)
	if completed.Code != http.StatusOK || field(completed, "status") != "completed" {
		test.Fatalf("complete: %d %s", completed.Code, completed.Body.String())
	}
	kinds, details = campaignEvents(api.call(http.MethodGet, campaignPath+"/events", nil, viewer))
	if strings.Join(kinds, ",") != "created,started,paused,nodes_retried,resumed,completed" || details["resumed"]["override_gate"] != true ||
		details["completed"]["reason"] != "every node ran it" {
		test.Fatalf("events: %v %v", kinds, details)
	}
	current.eventually("the counters count every node succeeded", func() (bool, string) {
		result := api.call(http.MethodGet, campaignPath, nil, viewer)
		counted := map[string]float64{}
		for _, counter := range field(result, "counters").([]any) {
			entry := counter.(map[string]any)
			counted[entry["state"].(string)] += entry["count"].(float64)
		}
		return counted["succeeded"] == 4 && len(counted) == 1, result.Body.String()
	})
	current.mutex.Lock()
	dispatches := current.dispatch
	current.mutex.Unlock()
	if dispatches != 5 {
		test.Fatalf("dawn was asked %d times, not once per node and once for the retry", dispatches)
	}

	request, _ := http.NewRequest(http.MethodGet, "http://"+current.address+"/api/v1/stream", nil)
	request.Header.Set("Authorization", "Bearer "+viewer)
	streamed, failure := http.DefaultClient.Do(request)
	if failure != nil {
		test.Fatal(failure)
	}
	defer streamed.Body.Close()
	reader := bufio.NewReader(streamed.Body)
	seen := map[string]bool{}
	for deadline := time.Now().Add(15 * time.Second); time.Now().Before(deadline) && !seen["presence"]; {
		line, failure := reader.ReadString('\n')
		if failure != nil {
			test.Fatal(failure)
		}
		if name, found := strings.CutPrefix(strings.TrimSpace(line), "event: "); found {
			seen[name] = true
		}
	}
	if streamed.StatusCode != http.StatusOK || !seen["presence"] {
		test.Fatalf("the event stream answered %d and sent %v", streamed.StatusCode, seen)
	}
}

func (current *stack) nodeStates(key string) []string {
	current.test.Helper()
	client, failure := kgo.NewClient(kgo.SeedBrokers(current.settings.Kafka.Brokers...), kgo.ConsumeTopics(current.settings.Kafka.Topics.NodeState),
		kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))
	if failure != nil {
		current.test.Fatal(failure)
	}
	defer client.Close()
	var lifecycles []string
	for {
		polling, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		fetches := client.PollFetches(polling)
		cancel()
		if fetches.Empty() {
			return lifecycles
		}
		fetches.EachRecord(func(record *kgo.Record) {
			if string(record.Key) != key {
				return
			}
			if record.Value == nil {
				lifecycles = append(lifecycles, "tombstone")
				return
			}
			var state kafka.NodeState
			if failure := json.Unmarshal(record.Value, &state); failure != nil {
				current.test.Fatal(failure)
			}
			lifecycles = append(lifecycles, state.Lifecycle)
		})
	}
}

func TestAnOperatorCannotUndoARevocation(test *testing.T) {
	current := newStack(test)
	operator, administrator, viewer := current.token("operator"), current.token("admin"), current.token("viewer")
	target := fleetNode{device: fmt.Sprintf("%032x", 0xa9200), installation: fmt.Sprintf("%032x", 0x19200), namespace: fmt.Sprintf("%016x", 0xb200), epoch: time.Now().UnixMicro()}
	current.enroll(target)
	path := "/api/v1/nodes/" + target.device + "/" + target.installation
	current.eventually("the node is in the inventory", func() (bool, string) {
		result := current.api.call(http.MethodGet, path, nil, viewer)
		return result.Code == http.StatusOK, result.Body.String()
	})
	if revoked := current.api.call(http.MethodPost, path+"/lifecycle", map[string]any{"lifecycle": "revoked", "reason": "stolen"}, administrator); revoked.Code != http.StatusOK || field(revoked, "lifecycle") != "revoked" {
		test.Fatalf("an administrator revoked the node: %d %s", revoked.Code, revoked.Body.String())
	}
	for _, lifecycle := range []string{"active", "quarantined"} {
		refused := current.api.call(http.MethodPost, path+"/lifecycle", map[string]any{"lifecycle": lifecycle, "reason": "found it"}, operator)
		if refused.Code != http.StatusForbidden || errorCode(refused) != "forbidden" {
			test.Fatalf("an operator moved a revoked node to %s: %d %s", lifecycle, refused.Code, refused.Body.String())
		}
	}
	var lifecycle string
	if failure := current.pool.QueryRow(context.Background(), `select lifecycle from nodes where device_id = $1 and installation_id = $2`, target.device, target.installation).Scan(&lifecycle); failure != nil || lifecycle != "revoked" {
		test.Fatalf("the inventory holds %q (%v) after an operator's refused change", lifecycle, failure)
	}
	key := "installation/" + target.device + "/" + target.installation
	if states := current.nodeStates(key); strings.Join(states, ",") != "revoked" {
		test.Fatalf("dusk.node-state for the node holds %v; only the administrator's revocation belongs there", states)
	}
	if restored := current.api.call(http.MethodPost, path+"/lifecycle", map[string]any{"lifecycle": "active", "reason": "recovered"}, administrator); restored.Code != http.StatusOK {
		test.Fatalf("an administrator restored the node: %d %s", restored.Code, restored.Body.String())
	}
	if states := current.nodeStates(key); strings.Join(states, ",") != "revoked,active" {
		test.Fatalf("dusk.node-state for the node holds %v", states)
	}
}

func TestAnAdministratorBlocksEveryInstallationOfADevice(test *testing.T) {
	current := newStack(test)
	operator, administrator, viewer := current.token("operator"), current.token("admin"), current.token("viewer")
	target := fleetNode{device: fmt.Sprintf("%032x", 0xa9300), installation: fmt.Sprintf("%032x", 0x19300), namespace: fmt.Sprintf("%016x", 0xb300), epoch: time.Now().UnixMicro()}
	current.enroll(target)
	node := "/api/v1/nodes/" + target.device + "/" + target.installation
	current.eventually("the node is in the inventory", func() (bool, string) {
		result := current.api.call(http.MethodGet, node, nil, viewer)
		return result.Code == http.StatusOK, result.Body.String()
	})
	path := "/api/v1/devices/" + target.device + "/lifecycle"
	if refused := current.api.call(http.MethodPost, path, map[string]any{"lifecycle": "revoked", "reason": "stolen"}, operator); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator revoked a device: %d %s", refused.Code, refused.Body.String())
	}
	if revoked := current.api.call(http.MethodPost, path, map[string]any{"lifecycle": "revoked", "reason": "stolen"}, administrator); revoked.Code != http.StatusOK || field(revoked, "lifecycle") != "revoked" {
		test.Fatalf("an administrator revoked the device: %d %s", revoked.Code, revoked.Body.String())
	}
	if detail := current.api.call(http.MethodGet, node, nil, viewer); field(detail, "device").(map[string]any)["lifecycle"] != "revoked" || field(detail, "lifecycle") != "enrolled" {
		test.Fatalf("the node after its device was revoked: %s", detail.Body.String())
	}
	if restored := current.api.call(http.MethodPost, path, map[string]any{"lifecycle": "active", "reason": "recovered"}, administrator); restored.Code != http.StatusOK {
		test.Fatalf("an administrator lifted the device's block: %d %s", restored.Code, restored.Body.String())
	}
	if states := current.nodeStates("device/" + target.device); strings.Join(states, ",") != "revoked,tombstone" {
		test.Fatalf("dusk.node-state for the device holds %v", states)
	}
	var lifecycle, actor string
	if failure := current.pool.QueryRow(context.Background(), `select lifecycle, actor from devices where device_id = $1`, target.device).Scan(&lifecycle, &actor); failure != nil || lifecycle != "active" || actor == "" {
		test.Fatalf("the inventory holds the device as %q by %q (%v)", lifecycle, actor, failure)
	}
}

func TestPostgresSessionsEndAndShareTheLoginKey(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := NewPostgresSessions(pool)
	operation := context.Background()
	now := time.Now().UTC().Truncate(time.Microsecond)
	digest := func(label string) []byte {
		sum := sha256.Sum256([]byte(label))
		return sum[:]
	}
	create := func(label string, created, lastSeen, expires time.Time, idleSince time.Time) {
		test.Helper()
		session := Session{Digest: digest(label), CSRFDigest: digest(label + " csrf"), Subject: label, Role: RoleOperator, Authentication: "oidc",
			CreatedAt: created, LastSeenAt: lastSeen, ExpiresAt: expires}
		if failure := store.CreateSession(operation, session, idleSince); failure != nil {
			test.Fatal(failure)
		}
	}
	create("expired", now.Add(-13*time.Hour), now.Add(-2*time.Minute), now.Add(-time.Hour), now.Add(-24*time.Hour))
	create("idle", now.Add(-2*time.Hour), now.Add(-90*time.Minute), now.Add(10*time.Hour), now.Add(-24*time.Hour))
	create("live", now.Add(-time.Hour), now.Add(-time.Minute), now.Add(11*time.Hour), now.Add(-24*time.Hour))
	create("fresh", now, now, now.Add(12*time.Hour), now.Add(-time.Hour))
	for label, kept := range map[string]bool{"expired": false, "idle": false, "live": true, "fresh": true} {
		_, failure := store.Session(operation, digest(label))
		if kept && failure != nil {
			test.Errorf("the %s session was deleted: %v", label, failure)
		}
		if !kept && !errors.Is(failure, ErrSessionNotFound) {
			test.Errorf("creating a session left the %s session: %v", label, failure)
		}
	}
	if failure := store.TouchSession(operation, digest("live"), now); failure != nil {
		test.Fatal(failure)
	}
	if failure := store.TouchSession(operation, digest("live"), now.Add(-time.Hour)); failure != nil {
		test.Fatal(failure)
	}
	live, failure := store.Session(operation, digest("live"))
	if failure != nil || !live.LastSeenAt.Equal(now) || live.Role != RoleOperator || live.Subject != "live" {
		test.Fatalf("a touched session: %+v %v", live, failure)
	}
	if failure := store.DeleteSession(operation, digest("live")); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Session(operation, digest("live")); !errors.Is(failure, ErrSessionNotFound) {
		test.Fatalf("a deleted session: %v", failure)
	}
	keys := make(chan []byte, 8)
	var waiting sync.WaitGroup
	for range 8 {
		waiting.Add(1)
		go func() {
			defer waiting.Done()
			key, failure := NewPostgresSessions(pool).LoginKey(operation)
			if failure != nil {
				test.Error(failure)
			}
			keys <- key
		}()
	}
	waiting.Wait()
	close(keys)
	var first []byte
	for key := range keys {
		if len(key) != 32 || (first != nil && !bytes.Equal(key, first)) {
			test.Fatalf("instances read different login keys: %x and %x", first, key)
		}
		first = key
	}
}
