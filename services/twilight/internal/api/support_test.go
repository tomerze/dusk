package api

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/santhosh-tekuri/jsonschema/v6"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/tokens"
)

const (
	deviceID       = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	installationID = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
)

var campaignIdentifier = uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e8f")

type recordedCall struct {
	method    string
	arguments []any
}

type fakeBackend struct {
	mutex     sync.Mutex
	calls     []recordedCall
	failure   error
	events    chan engine.FeedEvent
	lifecycle string
	entered   chan struct{}
	hold      chan struct{}
	deadline  time.Time
}

func (fake *fakeBackend) record(method string, arguments ...any) error {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	fake.calls = append(fake.calls, recordedCall{method: method, arguments: arguments})
	return fake.failure
}

func (fake *fakeBackend) last(method string) []any {
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	for index := len(fake.calls) - 1; index >= 0; index-- {
		if fake.calls[index].method == method {
			return fake.calls[index].arguments
		}
	}
	return nil
}

func sampleTime() time.Time {
	return time.Date(2026, 10, 7, 12, 0, 0, 0, time.UTC)
}

func sampleNode() engine.NodeSummary {
	hostname, version := "kiosk-0042", "0.2.0"
	seen := sampleTime()
	return engine.NodeSummary{
		Node:   inventory.Node{DeviceID: deviceID, InstallationID: installationID, Lifecycle: "active", Hostname: &hostname, DuskVersion: &version, FactsJSON: json.RawMessage(`{"dusk.version":"0.2.0"}`), FirstSeenAt: sampleTime(), UpdatedAt: sampleTime()},
		Online: true, LastSeenAt: &seen,
	}
}

func sampleRow() campaign.Row {
	dispatched := sampleTime()
	return campaign.Row{CampaignID: campaignIdentifier, DeviceID: deviceID, InstallationID: installationID, State: campaign.StateDelivered, Attempt: 1,
		Pid: 12808937078074471924, Epoch: 1759838400000000, NamespaceID: "5d2e9a1c7b3f8e04", DispatchedAt: &dispatched, Breakdown: map[string]string{"country": "US"}}
}

func sampleCampaign() *campaign.Campaign {
	definition := campaign.Definition{Name: "upgrade", Selector: `country == "US"`, Action: campaign.Action{Kind: campaign.KindEnsureVersion, Version: "0.2.0", Script: "upgrade"}}
	definition.Normalize()
	return &campaign.Campaign{ID: campaignIdentifier, Name: definition.Name, Status: campaign.StatusDraft, Kind: definition.Action.Kind, Selector: definition.Selector,
		Action: definition.Action, Policy: definition.Policy, CreatedBy: "token:test", CreatedAt: sampleTime(), UpdatedAt: sampleTime(), Version: 1}
}

func sampleAlert() alerts.Alert {
	return alerts.Alert{ID: 7, Time: sampleTime(), LastSeenAt: sampleTime(), Occurrences: 1, Severity: alerts.Critical, Kind: alerts.KindProcessWithoutIntent, Fingerprint: "f", Detail: json.RawMessage(`{"pid":"12808937078074471924"}`)}
}

func (fake *fakeBackend) Overview(operation context.Context) (engine.Overview, error) {
	if fake.hold != nil {
		fake.entered <- struct{}{}
		<-fake.hold
	}
	if deadline, bounded := operation.Deadline(); bounded {
		fake.mutex.Lock()
		fake.deadline = deadline
		fake.mutex.Unlock()
	}
	var overview engine.Overview
	overview.Nodes.Total, overview.Nodes.Online = 3, 2
	overview.Nodes.ByLifecycle = map[string]int64{"active": 3}
	overview.Campaigns = []engine.CampaignSummary{{Campaign: sampleCampaign(), Counters: []campaign.Counter{{Phase: 0, State: campaign.StateSucceeded, Count: 2}}}}
	overview.Alerts = map[alerts.Severity]int64{alerts.Critical: 1}
	overview.AlertsUnacknowledged = map[alerts.Severity]int64{alerts.Critical: 1}
	return overview, fake.record("Overview")
}

func (fake *fakeBackend) Nodes(operation context.Context, source string, online *bool, sort, cursor string, limit int) (engine.NodePage, error) {
	return engine.NodePage{Nodes: []engine.NodeSummary{sampleNode()}, Next: "next-nodes"}, fake.record("Nodes", source, online, sort, cursor, limit)
}

func (fake *fakeBackend) Node(operation context.Context, key inventory.Key) (engine.NodeDetail, error) {
	detail := engine.NodeDetail{NodeSummary: sampleNode(), Device: &inventory.DeviceLifecycle{DeviceID: deviceID, Lifecycle: "active", Reason: "recovered", ChangedAt: sampleTime(), Actor: "token:admin"}, Sessions: []engine.SessionView{{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 7, Instance: "nightfall-0", InnerAddress: "nightfall-0:8444", ConnectedAt: sampleTime(), LastSeen: sampleTime()}},
		Executions: []campaign.Row{sampleRow()}}
	return detail, fake.record("Node", key)
}

func (fake *fakeBackend) SetLifecycle(operation context.Context, key inventory.Key, lifecycle, reason, actor string, administrator bool) error {
	if failure := fake.record("SetLifecycle", key, lifecycle, reason, actor, administrator); failure != nil {
		return failure
	}
	fake.mutex.Lock()
	defer fake.mutex.Unlock()
	if failure := engine.CheckLifecycleChange(fake.lifecycle, lifecycle, administrator); failure != nil {
		return failure
	}
	fake.lifecycle = lifecycle
	return nil
}

func (fake *fakeBackend) SetDeviceLifecycle(operation context.Context, device, lifecycle, reason, actor string) (inventory.DeviceLifecycle, error) {
	return inventory.DeviceLifecycle{DeviceID: device, Lifecycle: lifecycle, Reason: reason, ChangedAt: sampleTime(), Actor: actor}, fake.record("SetDeviceLifecycle", device, lifecycle, reason, actor)
}

func (fake *fakeBackend) OpenSession(operation context.Context, key inventory.Key, reason string, lifetimeSeconds int, actor string) (engine.OperatorProcess, error) {
	nightfall := "nightfall-0:8444"
	return engine.OperatorProcess{Pid: 17352987455937552665,
		Node: dawn.NodeRef{DeviceID: deviceID, InstallationID: installationID, NamespaceID: "5d2e9a1c7b3f8e04", Nightfall: &nightfall}}, fake.record("OpenSession", key, reason, lifetimeSeconds, actor)
}

func (fake *fakeBackend) StreamLogs(operation context.Context, key inventory.Key, level string, durationSeconds int, actor string) (string, error) {
	return "stream-1", fake.record("StreamLogs", key, level, durationSeconds, actor)
}

func (fake *fakeBackend) CollectFile(operation context.Context, key inventory.Key, path, actor string) (string, error) {
	return "upload-1", fake.record("CollectFile", key, path, actor)
}

func (fake *fakeBackend) ValidateSelector(operation context.Context, source string) (engine.SelectorValidation, error) {
	return engine.SelectorValidation{OK: true, Matched: 1, Sample: []engine.NodeSummary{sampleNode()}}, fake.record("ValidateSelector", source)
}

func (fake *fakeBackend) Campaigns(operation context.Context, statuses []campaign.Status, cursor string, limit int) (engine.CampaignPage, error) {
	return engine.CampaignPage{Campaigns: []engine.CampaignSummary{{Campaign: sampleCampaign(), Counters: []campaign.Counter{}}}}, fake.record("Campaigns", statuses, cursor, limit)
}

func (fake *fakeBackend) Campaign(operation context.Context, identifier uuid.UUID) (engine.CampaignSummary, error) {
	return engine.CampaignSummary{Campaign: sampleCampaign(), Counters: []campaign.Counter{{Phase: 0, State: campaign.StatePending, Count: 4}}}, fake.record("Campaign", identifier)
}

func (fake *fakeBackend) CreateCampaign(operation context.Context, definition campaign.Definition, actor string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("CreateCampaign", definition, actor)
}

func (fake *fakeBackend) UpdateCampaign(operation context.Context, identifier uuid.UUID, definition campaign.Definition, version int, actor string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("UpdateCampaign", identifier, definition, version, actor)
}

func (fake *fakeBackend) StartCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("StartCampaign", identifier, actor)
}

func (fake *fakeBackend) PauseCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("PauseCampaign", identifier, actor, reason)
}

func (fake *fakeBackend) ResumeCampaign(operation context.Context, identifier uuid.UUID, actor string, overrideGate bool, reason string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("ResumeCampaign", identifier, actor, overrideGate, reason)
}

func (fake *fakeBackend) AbortCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("AbortCampaign", identifier, actor, reason)
}

func (fake *fakeBackend) CompleteCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("CompleteCampaign", identifier, actor, reason)
}

func (fake *fakeBackend) ArchiveCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error) {
	return sampleCampaign(), fake.record("ArchiveCampaign", identifier, actor)
}

func (fake *fakeBackend) CampaignNodes(operation context.Context, identifier uuid.UUID, query campaign.RowQuery) (campaign.RowPage, error) {
	return campaign.RowPage{Rows: []campaign.Row{sampleRow()}}, fake.record("CampaignNodes", identifier, query)
}

func (fake *fakeBackend) RetryNodes(operation context.Context, identifier uuid.UUID, selection engine.NodeSelection, actor string) (int, error) {
	return 3, fake.record("RetryNodes", identifier, selection, actor)
}

func (fake *fakeBackend) ResolveNodes(operation context.Context, identifier uuid.UUID, selection engine.NodeSelection, outcome string, actor string) (int, error) {
	return 1, fake.record("ResolveNodes", identifier, selection, outcome, actor)
}

func (fake *fakeBackend) CampaignEvents(operation context.Context, identifier uuid.UUID, after int64, limit int) ([]campaign.Event, error) {
	events := make([]campaign.Event, 0, limit)
	for index := range min(limit, 2) {
		events = append(events, campaign.Event{ID: after + int64(index) + 1, Time: sampleTime(), Kind: "created", Actor: "token:test", Detail: json.RawMessage(`{}`)})
	}
	return events, fake.record("CampaignEvents", identifier, after, limit)
}

func (fake *fakeBackend) CampaignGates(operation context.Context, identifier uuid.UUID) (engine.GateStatus, error) {
	status := engine.GateStatus{GateReport: engine.GateReport{Overall: engine.Tally{Succeeded: 3, Failed: 1}, Groups: map[string]map[string]engine.Tally{"country": {"US": {Succeeded: 3, Failed: 1}}},
		Verdict: engine.VerdictHold, Reason: "waiting for sample 4 of 10", MinSample: 10, Required: 10, MaxFailureRate: 0.05, MaxSilentRate: 0.05}, PhaseName: "canary", BakeSeconds: 900, BakeAccruedSeconds: 12.5}
	return status, fake.record("CampaignGates", identifier)
}

func (fake *fakeBackend) Alerts(operation context.Context, open bool, cursor string, limit int) (alerts.Page, error) {
	return alerts.Page{Alerts: []alerts.Alert{sampleAlert()}}, fake.record("Alerts", open, cursor, limit)
}

func (fake *fakeBackend) AcknowledgeAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error) {
	alert := sampleAlert()
	alert.AcknowledgedBy = &actor
	return alert, fake.record("AcknowledgeAlert", identifier, actor)
}

func (fake *fakeBackend) ResolveAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error) {
	return sampleAlert(), fake.record("ResolveAlert", identifier, actor)
}

func (fake *fakeBackend) Overlap(operation context.Context, identifier uuid.UUID) (*engine.OverlapError, error) {
	return &engine.OverlapError{Count: 12, Campaigns: []uuid.UUID{uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e91")}}, fake.record("Overlap", identifier)
}

func (fake *fakeBackend) Subscribe(operation context.Context) <-chan engine.FeedEvent {
	fake.mutex.Lock()
	events := fake.events
	fake.mutex.Unlock()
	if events == nil {
		closed := make(chan engine.FeedEvent)
		close(closed)
		return closed
	}
	return events
}

type fakeTokens map[string]tokens.Token

func (fake fakeTokens) Authenticate(operation context.Context, secret string) (tokens.Token, error) {
	token, found := fake[secret]
	if !found || token.RevokedAt != nil {
		return tokens.Token{}, tokens.ErrInvalid
	}
	return token, nil
}

type memorySessions struct {
	mutex    sync.Mutex
	sessions map[string]Session
	key      []byte
}

func newMemorySessions() *memorySessions {
	return &memorySessions{sessions: map[string]Session{}, key: bytes.Repeat([]byte{7}, 32)}
}

func (store *memorySessions) CreateSession(operation context.Context, session Session, idleSince time.Time) error {
	store.mutex.Lock()
	defer store.mutex.Unlock()
	for key, existing := range store.sessions {
		if !existing.ExpiresAt.After(session.CreatedAt) || existing.LastSeenAt.Before(idleSince) {
			delete(store.sessions, key)
		}
	}
	store.sessions[string(session.Digest)] = session
	return nil
}

func (store *memorySessions) Session(operation context.Context, digest []byte) (Session, error) {
	store.mutex.Lock()
	defer store.mutex.Unlock()
	session, found := store.sessions[string(digest)]
	if !found {
		return Session{}, ErrSessionNotFound
	}
	return session, nil
}

func (store *memorySessions) TouchSession(operation context.Context, digest []byte, at time.Time) error {
	store.mutex.Lock()
	defer store.mutex.Unlock()
	if session, found := store.sessions[string(digest)]; found && session.LastSeenAt.Before(at) {
		session.LastSeenAt = at
		store.sessions[string(digest)] = session
	}
	return nil
}

func (store *memorySessions) DeleteSession(operation context.Context, digest []byte) error {
	store.mutex.Lock()
	defer store.mutex.Unlock()
	delete(store.sessions, string(digest))
	return nil
}

func (store *memorySessions) LoginKey(operation context.Context) ([]byte, error) {
	return store.key, nil
}

const (
	viewerToken   = "twilight_viewer"
	operatorToken = "twilight_operator"
	adminToken    = "twilight_admin"
	revokedToken  = "twilight_revoked"
)

type harness struct {
	test     *testing.T
	server   *Server
	backend  *fakeBackend
	sessions *memorySessions
	now      time.Time
	logs     *bytes.Buffer
	document map[string]any
	schemas  *jsonschema.Compiler
}

func newHarness(test *testing.T, configure func(*Options)) *harness {
	test.Helper()
	revoked := sampleTime()
	current := &harness{test: test, backend: &fakeBackend{}, sessions: newMemorySessions(), now: sampleTime(), logs: &bytes.Buffer{}}
	options := Options{
		Backend: current.backend,
		Tokens: fakeTokens{
			viewerToken:   {ID: uuid.MustParse("00000000-0000-7000-8000-000000000001"), Name: "dashboards", Role: "viewer"},
			operatorToken: {ID: uuid.MustParse("00000000-0000-7000-8000-000000000002"), Name: "release automation", Role: "operator"},
			adminToken:    {ID: uuid.MustParse("00000000-0000-7000-8000-000000000003"), Name: "break glass", Role: "admin"},
			revokedToken:  {ID: uuid.MustParse("00000000-0000-7000-8000-000000000004"), Name: "old", Role: "admin", RevokedAt: &revoked},
		},
		Sessions:        current.sessions,
		Principals:      PrincipalRoles{"automation-*": RoleOperator, "readers-*": RoleViewer},
		SessionLifetime: 12 * time.Hour,
		SessionIdle:     time.Hour,
		MaxStreams:      4,
		Logger:          slog.New(slog.NewJSONHandler(current.logs, &slog.HandlerOptions{Level: slog.LevelDebug})),
		Now:             func() time.Time { return current.now },
	}
	if configure != nil {
		configure(&options)
	}
	server, failure := New(options)
	if failure != nil {
		test.Fatal(failure)
	}
	current.server = server
	if failure := json.Unmarshal(openAPIDocument, &current.document); failure != nil {
		test.Fatal(failure)
	}
	if current.schemas, failure = compiler(); failure != nil {
		test.Fatal(failure)
	}
	return current
}

type response struct {
	*httptest.ResponseRecorder
	decoded any
}

func (current *harness) do(request *http.Request) response {
	current.test.Helper()
	recorder := httptest.NewRecorder()
	current.server.ServeHTTP(recorder, request)
	result := response{ResponseRecorder: recorder}
	if strings.HasPrefix(recorder.Header().Get("Content-Type"), "application/json") {
		if failure := json.Unmarshal(recorder.Body.Bytes(), &result.decoded); failure != nil {
			current.test.Fatalf("%s %s answered JSON that does not decode: %v\n%s", request.Method, request.URL, failure, recorder.Body.String())
		}
	}
	current.conforms(request, recorder)
	return result
}

func newRequest(method, target string, body any, token string) *http.Request {
	var reader io.Reader
	if body != nil {
		switch typed := body.(type) {
		case string:
			reader = strings.NewReader(typed)
		default:
			encoded, _ := json.Marshal(typed)
			reader = bytes.NewReader(encoded)
		}
	}
	request := httptest.NewRequest(method, target, reader)
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	return request
}

func (current *harness) call(method, target string, body any, token string) response {
	current.test.Helper()
	return current.do(newRequest(method, target, body, token))
}

func escapePointer(segment string) string {
	return strings.ReplaceAll(strings.ReplaceAll(segment, "~", "~0"), "/", "~1")
}

func (current *harness) conforms(request *http.Request, recorder *httptest.ResponseRecorder) {
	current.test.Helper()
	_, pattern := current.server.mux.Handler(request)
	method, path, found := strings.Cut(pattern, " ")
	if !found {
		return
	}
	operation, found := current.document["paths"].(map[string]any)[path].(map[string]any)[strings.ToLower(method)].(map[string]any)
	if !found {
		current.test.Fatalf("%s is not in the OpenAPI document", pattern)
	}
	status := strconv.Itoa(recorder.Code)
	documented, found := operation["responses"].(map[string]any)[status].(map[string]any)
	if !found {
		current.test.Fatalf("%s answered %d, which its operation does not document: %s", pattern, recorder.Code, recorder.Body.String())
	}
	location := pointer("paths", path, strings.ToLower(method), "responses", status)
	if reference, isReference := documented["$ref"].(string); isReference {
		name := strings.TrimPrefix(reference, "#/components/responses/")
		documented = current.document["components"].(map[string]any)["responses"].(map[string]any)[name].(map[string]any)
		location = documentLocation + "#/components/responses/" + escapePointer(name)
	}
	content, hasContent := documented["content"].(map[string]any)
	if !hasContent {
		if recorder.Body.Len() > 0 && recorder.Code != http.StatusFound && recorder.Code != http.StatusSeeOther {
			current.test.Fatalf("%s answered %d with a body its operation does not document", pattern, recorder.Code)
		}
		return
	}
	mediaType := strings.Split(recorder.Header().Get("Content-Type"), ";")[0]
	if _, found := content[mediaType]; !found {
		current.test.Fatalf("%s answered %d as %q, which its operation does not document", pattern, recorder.Code, mediaType)
	}
	if mediaType != "application/json" {
		return
	}
	schema, failure := current.schemas.Compile(location + "/content/application~1json/schema")
	if failure != nil {
		current.test.Fatalf("compile the response schema of %s %d: %v", pattern, recorder.Code, failure)
	}
	instance, failure := jsonschema.UnmarshalJSON(bytes.NewReader(recorder.Body.Bytes()))
	if failure != nil {
		current.test.Fatalf("%s %d: %v", pattern, recorder.Code, failure)
	}
	if failure := schema.Validate(instance); failure != nil {
		current.test.Fatalf("%s answered %d with a body that does not match its documented schema: %v\n%s", pattern, recorder.Code, failure, recorder.Body.String())
	}
}

func errorCode(result response) string {
	body, _ := result.decoded.(map[string]any)
	problem, _ := body["error"].(map[string]any)
	code, _ := problem["code"].(string)
	return code
}

func errorDetails(result response) map[string]any {
	body, _ := result.decoded.(map[string]any)
	problem, _ := body["error"].(map[string]any)
	details, _ := problem["details"].(map[string]any)
	return details
}

func field(result response, name string) any {
	body, _ := result.decoded.(map[string]any)
	return body[name]
}

func sessionCookieFor(current *harness, role Role, csrf string) (*http.Cookie, string) {
	secret := randomSecret()
	if csrf == "" {
		csrf = randomSecret()
	}
	name := "Ada Operator"
	sum := sha256.Sum256([]byte(secret))
	csrfSum := sha256.Sum256([]byte(csrf))
	_ = current.sessions.CreateSession(context.Background(), Session{Digest: sum[:], CSRFDigest: csrfSum[:], Subject: "user-" + role.String(), Name: &name, Role: role,
		Authentication: AuthenticationOIDC, CreatedAt: current.now, LastSeenAt: current.now, ExpiresAt: current.now.Add(12 * time.Hour)}, current.now.Add(-time.Hour))
	return &http.Cookie{Name: SessionCookie, Value: secret}, csrf
}
