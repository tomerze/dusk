//go:build integration

package erasure

import (
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"slices"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/clickhouse"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/objectstore"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

const (
	erased       = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	bystander    = "8b1d0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	installation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
	second       = "b41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
)

type fakeClickHouse struct {
	mutex      sync.Mutex
	statements []string
	scope      map[string]map[string]bool
	rows       map[string]int64
	lake       map[string][]lakeFile
	pending    int
}

var (
	countedTable = regexp.MustCompile(`SELECT count\(\) AS rows FROM dusk\.([a-z_]+) WHERE`)
	lakeTable    = regexp.MustCompile(`FROM dusk\.lake_([a-z_]+)_parquet`)
)

func (fake *fakeClickHouse) ServeHTTP(writer http.ResponseWriter, request *http.Request) {
	body, _ := io.ReadAll(request.Body)
	text := request.URL.Query().Get("query")
	if text == "" {
		text = string(body)
	}
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	fake.statements = append(fake.statements, text)
	reply := func(rows ...any) {
		for _, row := range rows {
			encoded, _ := json.Marshal(row)
			fmt.Fprintln(writer, string(encoded))
		}
	}
	switch {
	case strings.HasPrefix(text, "INSERT INTO dusk.erasure_scope (erasure_id, kind, value) FORMAT TSV"):
		for _, line := range strings.Split(strings.TrimSpace(string(body)), "\n") {
			fields := strings.Split(line, "\t")
			if fake.scope[fields[1]] == nil {
				fake.scope[fields[1]] = map[string]bool{}
			}
			fake.scope[fields[1]][fields[2]] = true
		}
	case strings.Contains(text, "uniqExact(value)"):
		reply(map[string]any{"kind": "device", "count": len(fake.scope["device"])}, map[string]any{"kind": "host", "count": len(fake.scope["host"])})
	case countedTable.MatchString(text):
		reply(map[string]any{"rows": fake.rows[countedTable.FindStringSubmatch(text)[1]]})
	case strings.Contains(text, "AS pending"):
		reply(map[string]any{"pending": fake.pending, "failure": ""})
		fake.pending = max(fake.pending-1, 0)
	case strings.Contains(text, "SELECT DISTINCT value FROM dusk.erasure_scope"):
		var devices []string
		for device := range fake.scope["device"] {
			if device > request.URL.Query().Get("param_after") {
				devices = append(devices, device)
			}
		}
		sort.Strings(devices)
		for _, device := range devices {
			reply(map[string]any{"value": device})
		}
	case strings.HasPrefix(text, "SELECT _path AS path"):
		for _, file := range fake.lake[lakeTable.FindStringSubmatch(text)[1]] {
			reply(file)
		}
	case strings.Contains(text, "FROM system.columns"):
		reply(map[string]any{"name": "time", "type": "DateTime64(9, 'UTC')"}, map[string]any{"name": "device_id", "type": "String"})
	case strings.Contains(text, "engine_full"):
		reply(map[string]any{"engine_full": "ReplicatedReplacingMergeTree PARTITION BY toYYYYMM(time) ORDER BY (device_id, installation_id, time, id) TTL time + toIntervalDay(30) SETTINGS index_granularity = 8192"})
	}
}

func (fake *fakeClickHouse) matching(pattern string) []string {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	var found []string
	for _, statement := range fake.statements {
		if strings.Contains(statement, pattern) {
			found = append(found, statement)
		}
	}
	return found
}

type storedObject struct {
	size     int64
	modified time.Time
}

type fakeObjectStore struct {
	mutex   sync.Mutex
	objects map[string]map[string]storedObject
	failing bool
}

func (fake *fakeObjectStore) ServeHTTP(writer http.ResponseWriter, request *http.Request) {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	if fake.failing {
		writer.WriteHeader(http.StatusForbidden)
		fmt.Fprint(writer, `<Error><Code>AccessDenied</Code><Message>denied</Message></Error>`)
		return
	}
	bucket, key, _ := strings.Cut(strings.TrimPrefix(request.URL.Path, "/"), "/")
	query := request.URL.Query()
	switch {
	case query.Has("object-lock"):
		fmt.Fprint(writer, `<ObjectLockConfiguration><Rule><DefaultRetention><Mode>COMPLIANCE</Mode><Days>400</Days></DefaultRetention></Rule></ObjectLockConfiguration>`)
	case query.Has("lifecycle"):
		fmt.Fprint(writer, `<LifecycleConfiguration><Rule><Status>Enabled</Status><Filter><Prefix>ledger/</Prefix></Filter><Expiration><Days>400</Days></Expiration></Rule></LifecycleConfiguration>`)
	case query.Get("list-type") == "2":
		var keys []string
		for stored := range fake.objects[bucket] {
			if strings.HasPrefix(stored, query.Get("prefix")) {
				keys = append(keys, stored)
			}
		}
		sort.Strings(keys)
		fmt.Fprint(writer, "<ListBucketResult><IsTruncated>false</IsTruncated>")
		for _, stored := range keys {
			object := fake.objects[bucket][stored]
			fmt.Fprintf(writer, "<Contents><Key>%s</Key><Size>%d</Size><LastModified>%s</LastModified></Contents>", stored, object.size, object.modified.UTC().Format(time.RFC3339))
		}
		fmt.Fprint(writer, "</ListBucketResult>")
	case request.Method == http.MethodPost && query.Has("delete"):
		var document struct {
			Objects []struct {
				Key string `xml:"Key"`
			} `xml:"Object"`
		}
		body, _ := io.ReadAll(request.Body)
		_ = xml.Unmarshal(body, &document)
		for _, object := range document.Objects {
			delete(fake.objects[bucket], object.Key)
		}
		fmt.Fprint(writer, "<DeleteResult></DeleteResult>")
	case request.Method == http.MethodDelete:
		delete(fake.objects[bucket], key)
		writer.WriteHeader(http.StatusNoContent)
	default:
		http.Error(writer, "unexpected "+request.Method+" "+request.URL.String(), http.StatusBadRequest)
	}
}

func (fake *fakeObjectStore) keys(bucket string) []string {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	var keys []string
	for key := range fake.objects[bucket] {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

type setup struct {
	pool     *pgxpool.Pool
	manager  *Manager
	events   *fakeClickHouse
	objects  *fakeObjectStore
	online   map[inventory.Key]bool
	campaign *campaign.Campaign
	now      time.Time
}

func newSetup(test *testing.T, configure func(*config.Config)) *setup {
	pool, _ := testsupport.Database(test)
	current := &setup{pool: pool, online: map[inventory.Key]bool{}, now: time.Now().UTC().Truncate(time.Second),
		events:  &fakeClickHouse{scope: map[string]map[string]bool{}, rows: map[string]int64{}, lake: map[string][]lakeFile{}},
		objects: &fakeObjectStore{objects: map[string]map[string]storedObject{"dusk-files": {}, "dusk-lake": {}}}}
	eventServer := httptest.NewServer(current.events)
	objectServer := httptest.NewServer(current.objects)
	test.Cleanup(eventServer.Close)
	test.Cleanup(objectServer.Close)
	events, failure := clickhouse.New(clickhouse.Settings{URL: eventServer.URL, User: "twilight", Password: "secret"})
	if failure != nil {
		test.Fatal(failure)
	}
	objects, failure := objectstore.New(objectstore.Settings{Endpoint: objectServer.URL, Region: "us-east-1", AccessKey: "access", SecretKey: "secret", Attempts: 1})
	if failure != nil {
		test.Fatal(failure)
	}
	settings := config.Default()
	if configure != nil {
		configure(&settings)
	}
	current.manager = New(Dependencies{Pool: pool, ClickHouse: events, ObjectStore: objects, Alerts: testsupport.AlertStore(test, pool),
		Topics: func(context.Context) ([]kafka.TopicRetention, error) {
			return []kafka.TopicRetention{{Topic: "dusk.connections", CleanupPolicy: "delete", Retention: 7 * 24 * time.Hour, Segment: 7 * 24 * time.Hour},
				{Topic: "dusk.node-state", CleanupPolicy: "compact"}}, nil
		},
		Online: func(key inventory.Key) bool { return current.online[key] }, Config: settings, Logger: testsupport.Logger(),
		Now: func() time.Time { return current.now }})
	return current
}

func (current *setup) exec(test *testing.T, sql string, arguments ...any) {
	test.Helper()
	if _, failure := current.pool.Exec(context.Background(), sql, arguments...); failure != nil {
		test.Fatalf("%s: %v", sql, failure)
	}
}

func (current *setup) count(test *testing.T, sql string, arguments ...any) int64 {
	test.Helper()
	var found int64
	if failure := current.pool.QueryRow(context.Background(), sql, arguments...).Scan(&found); failure != nil {
		test.Fatalf("%s: %v", sql, failure)
	}
	return found
}

func (current *setup) seed(test *testing.T) {
	operation := context.Background()
	for _, row := range []struct{ device, installation, lifecycle string }{{erased, installation, "retired"}, {erased, second, "revoked"}, {bystander, installation, "active"}} {
		current.exec(test, `insert into nodes (device_id, installation_id, lifecycle, tenant, hostname) values ($1, $2, $3, 'acme', 'kitchen-hub')`, row.device, row.installation, row.lifecycle)
	}
	current.exec(test, `insert into node_presence (device_id, installation_id, sessions) values ($1, $2, '[{"namespace_id": "05d2e9a1c7b3f8e0"}]')`, erased, installation)
	definition := campaign.Definition{Name: "upgrade", Selector: `country == "US"`, Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"},
		Policy: campaign.Policy{Rate: campaign.Rate{PerSecond: 50, Burst: 50}, Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}},
			NodeTimeoutSeconds: 1, Gates: campaign.Gates{MinSample: 1}}}
	created, failure := campaign.NewStore(current.pool).Create(operation, definition, "token:test", current.now)
	if failure != nil {
		test.Fatal(failure)
	}
	current.campaign = created
	current.exec(test, `update campaigns set status = 'completed', finished_at = now() where id = $1`, created.ID)
	for _, device := range []string{erased, bystander} {
		current.exec(test, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state, namespace_id) values ($1, $2, $3, 0, 'succeeded', '05d2e9a1c7b3f8e0')`,
			created.ID, device, installation)
	}
	current.exec(test, `insert into campaign_counters (campaign_id, phase, state, count) values ($1, 0, 'succeeded', 2)`, created.ID)
	current.exec(test, `insert into campaign_events (campaign_id, kind, actor, detail) values ($1, 'conflict', 'twilight', $2::jsonb)`, created.ID,
		`{"campaign_id": "`+created.ID.String()+`", "device_id": "`+erased+`", "installation_id": "`+installation+`", "message": "conflict"}`)
	for index, tombstoned := range []bool{true, false} {
		var at any
		if tombstoned {
			at = current.now
		}
		current.exec(test, `insert into intended_processes (pid, created_at, device_id, installation_id, action_kind, principal, subject, expires_at, max_commands,
				default_shell_commands, last_dispatched_at, tombstoned_at)
			values ($1, $2, $3, $4, 'collect_facts', 'twilight', 'twilight', $5, 1, 1, $2, $6)`,
			70000+index, current.now, erased, installation, current.now.Add(time.Hour), at)
	}
	current.exec(test, `insert into reconcile_default_shell (device_id, installation_id, window_start, commands, last_call_at) values ($1, $2, now(), 1, now())`, erased, installation)
	current.exec(test, `insert into revocations (id, credential_kind, credential, reason, actor, created_at, matched, revoked, cursor_device_id, cursor_installation_id, finished_at)
		values (gen_random_uuid(), 'fleet_token', 'compose', 'leaked', 'token:admin', now(), 2, 2, $1, $2, now())`, erased, installation)
	if _, failure := testsupport.AlertStore(test, current.pool).Raise(operation, alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindProcessWithoutIntent,
		Fingerprint: "process_without_intent:" + erased, Detail: map[string]any{"device_id": erased, "installation_id": installation}}); failure != nil {
		test.Fatal(failure)
	}
	current.objects.objects["dusk-files"]["files/"+erased+"/"+installation+"/70000/0-photo.jpg"] = storedObject{size: 2048, modified: current.now.Add(-48 * time.Hour)}
	current.objects.objects["dusk-files"]["files/"+bystander+"/"+installation+"/70001/0-log.txt"] = storedObject{size: 10, modified: current.now}
	current.objects.objects["dusk-lake"]["connections/dt=2026-10-01/partly.parquet"] = storedObject{size: 100, modified: current.now}
	current.objects.objects["dusk-lake"]["connections/dt=2026-10-01/wholly.parquet"] = storedObject{size: 100, modified: current.now}
	current.events.lake["connections"] = []lakeFile{{Path: "dusk-lake/connections/dt=2026-10-01/partly.parquet", Matched: 2, Total: 9},
		{Path: "dusk-lake/connections/dt=2026-10-01/wholly.parquet", Matched: 4, Total: 4}}
	current.events.rows["connections"] = 6
	current.events.rows["otel_logs"] = 3
	current.events.pending = 2
}

func (current *setup) run(test *testing.T, erasure Erasure) Erasure {
	test.Helper()
	operation := context.Background()
	for range 30 {
		current.manager.advance(operation, erasure)
		current.exec(test, `update intended_processes set tombstoned_at = now() where tombstoned_at is null and expires_at <= now()`)
		reloaded, failure := current.manager.Get(operation, erasure.ID)
		if failure != nil {
			test.Fatal(failure)
		}
		if reloaded.Status == StatusCompleted {
			return reloaded
		}
		erasure = reloaded
	}
	test.Fatalf("the erasure did not finish: %+v", erasure)
	return erasure
}

func TestADeviceIsErasedOnlyOnceItCannotConnect(test *testing.T) {
	current := newSetup(test, nil)
	current.seed(test)
	operation := context.Background()
	var notReady *NotReadyError
	if _, _, failure := current.manager.RequestDevice(operation, bystander, "ticket 1", "token:admin"); !errors.As(failure, &notReady) || notReady.NotWithdrawn != 1 {
		test.Fatalf("an active device: %v", failure)
	}
	current.online[inventory.Key{DeviceID: erased, InstallationID: second}] = true
	if _, _, failure := current.manager.RequestDevice(operation, erased, "ticket 1", "token:admin"); !errors.As(failure, &notReady) || notReady.Online != 1 || notReady.NotWithdrawn != 0 {
		test.Fatalf("an online device: %v", failure)
	}
	current.online = map[inventory.Key]bool{}
	if _, _, failure := current.manager.RequestDevice(operation, erased, " ", "token:admin"); !errors.Is(failure, ErrInvalidReason) {
		test.Fatalf("a blank reason: %v", failure)
	}
	first, created, failure := current.manager.RequestDevice(operation, erased, "erasure request 4711", "token:admin")
	if failure != nil || !created || first.Status != StatusRunning || len(first.Steps) != 5 {
		test.Fatalf("the erasure %+v %v %v", first, created, failure)
	}
	again, created, failure := current.manager.RequestDevice(operation, erased, "asked twice", "token:admin")
	if failure != nil || created || again.ID != first.ID || again.Reason != "erasure request 4711" {
		test.Fatalf("a second request %+v %v %v", again, created, failure)
	}
	current.exec(test, `update nodes set lifecycle = 'active' where device_id = $1`, erased)
	current.exec(test, `insert into devices (device_id, lifecycle, lifecycle_reason, lifecycle_changed_at, actor) values ($1, 'retired', 'returned', now(), 'token:admin')`, erased)
	if _, _, failure := current.manager.RequestDevice(operation, erased, "ticket 2", "token:admin"); failure != nil {
		test.Fatalf("a device retired as a whole: %v", failure)
	}
}

func TestErasingADeviceEverywhere(test *testing.T) {
	current := newSetup(test, nil)
	current.seed(test)
	operation := context.Background()
	requested, _, failure := current.manager.RequestDevice(operation, erased, "erasure request 4711", "token:admin")
	if failure != nil {
		test.Fatal(failure)
	}
	finished := current.run(test, requested)
	for _, step := range finished.Steps {
		if step.State != StepDone || step.StartedAt == nil || step.FinishedAt == nil {
			test.Fatalf("step %+v", step)
		}
	}
	if !current.events.scope["device"][erased] || !current.events.scope["host"]["5d2e9a1c7b3f8e0"] || current.events.scope["device"][bystander] {
		test.Fatalf("the scope %v", current.events.scope)
	}
	for _, name := range []string{"connections", "enrollments", "process_results", "process_output", "files", "otel_logs", "otel_spans"} {
		altered := current.events.matching("ALTER TABLE dusk." + name + " DELETE WHERE")
		wanted := 0
		if current.events.rows[name] > 0 {
			wanted = 1
		}
		if len(altered) != wanted {
			test.Errorf("%d deletes of dusk.%s, want %d", len(altered), name, wanted)
		}
	}
	if ledger := current.events.matching("dusk.ledger DELETE"); len(ledger) != 0 {
		test.Fatalf("the ledger was deleted from: %v", ledger)
	}
	rewrites := current.events.matching("INSERT INTO FUNCTION s3(lake_writer")
	if len(rewrites) != 1 || !strings.Contains(rewrites[0], "filename = 'connections/dt=2026-10-01/partly.parquet'") ||
		!strings.Contains(rewrites[0], "WHERE NOT ifNull(device_id IN (SELECT value FROM dusk.erasure_scope") {
		test.Fatalf("the lake rewrites %v", rewrites)
	}
	if keys := current.objects.keys("dusk-lake"); len(keys) != 0 {
		test.Fatalf("the lake still holds %v", keys)
	}
	if keys := current.objects.keys("dusk-files"); !slices.Equal(keys, []string{"files/" + bystander + "/" + installation + "/70001/0-log.txt"}) {
		test.Fatalf("the files bucket holds %v", keys)
	}
	for table, sql := range map[string]string{
		"nodes":                   `select count(*) from nodes where device_id = $1`,
		"node_presence":           `select count(*) from node_presence where device_id = $1`,
		"campaign_nodes":          `select count(*) from campaign_nodes where device_id = $1`,
		"intended_processes":      `select count(*) from intended_processes where device_id = $1`,
		"reconcile_default_shell": `select count(*) from reconcile_default_shell where device_id = $1`,
		"campaign_events":         `select count(*) from campaign_events where detail ->> 'device_id' = $1`,
		"revocations":             `select count(*) from revocations where cursor_device_id = $1`,
	} {
		if left := current.count(test, sql, erased); left != 0 {
			test.Errorf("%s still holds %d rows of the device", table, left)
		}
	}
	if kept := current.count(test, `select count(*) from nodes where device_id = $1`, bystander); kept != 1 {
		test.Fatalf("the bystander's inventory row is gone")
	}
	if counted := current.count(test, `select count from campaign_counters where campaign_id = $1 and state = 'succeeded'`, current.campaign.ID); counted != 1 {
		test.Fatalf("the finished campaign counts %d succeeded rows, want 1", counted)
	}
	if kept := current.count(test, `select count(*) from campaign_events where kind = 'conflict' and detail ->> 'message' = 'conflict'`); kept != 1 {
		test.Fatal("the scrubbed campaign event is gone")
	}
	if kept := current.count(test, `select count(*) from alerts where detail ->> 'device_id' = $1`, erased); kept != 1 {
		test.Fatal("the security alert naming the device is gone")
	}
	if named := current.count(test, `select count(*) from alert_transitions where hostname is not null`); named != 0 || finished.Steps[4].Counts["alert_hostnames"] != 1 {
		test.Fatalf("%d alert notifications still name a hostname; the step counted %v", named, finished.Steps[4].Counts)
	}
	inventoryStep := finished.Steps[4]
	if inventoryStep.Counts["installations"] != 2 || inventoryStep.Counts["campaign_rows"] != 1 || inventoryStep.Counts["intended_processes"] != 2 || inventoryStep.Counts["alerts_kept"] != 1 {
		test.Fatalf("the inventory step counted %v", inventoryStep.Counts)
	}
	stores := map[string]*time.Time{}
	for _, entry := range finished.Retained {
		stores[entry.Store] = entry.Until
		if entry.Why == "" || entry.Data == "" {
			test.Fatalf("a retained entry without its reason %+v", entry)
		}
	}
	day := 24 * time.Hour
	for store, until := range map[string]time.Duration{"object store dusk-ledger-evidence": 402 * day, "object store dusk-lake": 401 * day, "clickhouse dusk.ledger": 31 * day,
		"kafka dusk.connections": 14 * day} {
		if got, found := stores[store]; !found || got == nil || !got.Equal(current.now.Add(until)) {
			test.Errorf("%s retained until %v, want %v", store, got, current.now.Add(until))
		}
	}
	for _, store := range []string{"postgres devices, kafka dusk.node-state", "postgres alerts", "postgres erasures", "SigNoz"} {
		if _, found := stores[store]; !found {
			test.Errorf("the report does not name %s: %v", store, stores)
		}
	}
	if cleaned := current.events.matching("ALTER TABLE dusk.erasure_scope DELETE WHERE erasure_id = '" + requested.ID.String() + "'"); len(cleaned) != 1 {
		test.Fatalf("the scope was cleaned %d times", len(cleaned))
	}
}

func TestAFailingStepIsRetriedAndRaisesAnAlert(test *testing.T) {
	current := newSetup(test, nil)
	current.seed(test)
	operation := context.Background()
	requested, _, failure := current.manager.RequestDevice(operation, erased, "erasure request 4711", "token:admin")
	if failure != nil {
		test.Fatal(failure)
	}
	current.objects.failing = true
	current.events.pending = 0
	erasure := requested
	for attempt := 1; attempt <= stalledAttempts; attempt++ {
		current.manager.advance(operation, erasure)
		if erasure, failure = current.manager.Get(operation, requested.ID); failure != nil {
			test.Fatal(failure)
		}
		if erasure.Attempts != attempt || erasure.LastError == nil || !strings.Contains(*erasure.LastError, "AccessDenied") || !erasure.NextAttemptAt.After(current.now) {
			test.Fatalf("attempt %d: %+v", attempt, erasure)
		}
	}
	if erasure.Steps[0].State != StepDone || erasure.Steps[1].State != StepDone || erasure.Steps[2].State != StepRunning {
		test.Fatalf("the steps %+v", erasure.Steps)
	}
	current.manager.raiseStalled(operation)
	if raised := current.count(test, `select count(*) from alerts where kind = 'erasure_stalled' and fingerprint = $1`, "erasure_stalled:"+requested.ID.String()); raised != 1 {
		test.Fatalf("%d stalled alerts", raised)
	}
	current.objects.failing = false
	if finished := current.run(test, erasure); finished.Attempts != 0 || finished.LastError != nil {
		test.Fatalf("the recovered erasure %+v", finished)
	}
	if open := current.count(test, `select count(*) from alerts where kind = 'erasure_stalled' and resolved_at is null`); open != 0 {
		test.Fatalf("%d stalled alerts stayed open after the erasure finished", open)
	}
}

func TestATenantIsErasedOnceAllItsNodesAreWithdrawn(test *testing.T) {
	current := newSetup(test, nil)
	current.seed(test)
	operation := context.Background()
	var notReady *NotReadyError
	current.exec(test, `update campaigns set status = 'paused' where id = $1`, current.campaign.ID)
	current.exec(test, `update campaigns set tenant = 'acme' where id = $1`, current.campaign.ID)
	if _, _, failure := current.manager.RequestTenant(operation, "acme", "contract ended", "token:admin"); !errors.As(failure, &notReady) || notReady.NotWithdrawn != 1 || notReady.RunningCampaigns != 1 {
		test.Fatalf("a tenant with an active node and a paused campaign: %v", failure)
	}
	current.exec(test, `update nodes set lifecycle = 'revoked' where device_id = $1`, bystander)
	current.exec(test, `update campaigns set status = 'aborted' where id = $1`, current.campaign.ID)
	requested, created, failure := current.manager.RequestTenant(operation, "acme", "contract ended", "token:admin")
	if failure != nil || !created {
		test.Fatalf("the tenant erasure %v %v", created, failure)
	}
	finished := current.run(test, requested)
	if left := current.count(test, `select count(*) from nodes where tenant = 'acme'`); left != 0 {
		test.Fatalf("%d of the tenant's nodes remain", left)
	}
	if left := current.count(test, `select count(*) from campaigns where id = $1`, current.campaign.ID); left != 0 || finished.Steps[4].Counts["campaigns"] != 1 {
		test.Fatalf("the tenant's campaign remains: %d, counted %v", left, finished.Steps[4].Counts)
	}
	if keys := current.objects.keys("dusk-files"); len(keys) != 0 {
		test.Fatalf("the tenant's files remain: %v", keys)
	}
	if tenants := current.events.matching("WHERE tenant = {tenant:String}"); len(tenants) != 4 {
		test.Fatalf("the tenant's devices were looked up in %d ClickHouse tables, want 4", len(tenants))
	}
}
