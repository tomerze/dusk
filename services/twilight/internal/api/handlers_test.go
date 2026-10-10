package api

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/selector"
)

func items(result response) []any {
	listed, _ := field(result, "items").([]any)
	return listed
}

func TestOverview(test *testing.T) {
	current := newHarness(test, nil)
	result := current.call(http.MethodGet, "/api/v1/overview", nil, viewerToken)
	if result.Code != http.StatusOK || !strings.Contains(result.Body.String(), `"by_lifecycle":{"active":3}`) {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
}

func TestListingNodes(test *testing.T) {
	current := newHarness(test, nil)
	result := current.call(http.MethodGet, `/api/v1/nodes?selector=country+%3D%3D+%22US%22&online=true&sort=-hostname&cursor=abc&limit=50`, nil, viewerToken)
	if result.Code != http.StatusOK || len(items(result)) != 1 || field(result, "next_cursor") != "next-nodes" {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
	arguments := current.backend.last("Nodes")
	if arguments[0] != `country == "US"` || !*(arguments[1].(*bool)) || arguments[2] != "-hostname" || arguments[3] != "abc" || arguments[4] != 50 {
		test.Fatalf("Nodes %v", arguments)
	}
	if current.call(http.MethodGet, "/api/v1/nodes", nil, viewerToken); current.backend.last("Nodes")[1].(*bool) != nil || current.backend.last("Nodes")[4] != 100 {
		test.Fatalf("defaults %v", current.backend.last("Nodes"))
	}
	for _, query := range []string{"limit=0", "limit=501", "limit=many", "online=sometimes"} {
		if refused := current.call(http.MethodGet, "/api/v1/nodes?"+query, nil, viewerToken); refused.Code != http.StatusBadRequest || errorCode(refused) != "invalid_argument" {
			test.Errorf("%s answered %d", query, refused.Code)
		}
	}
	current.backend.failure = &selector.Error{Position: 8, End: 10, Message: "expected a value"}
	broken := current.call(http.MethodGet, "/api/v1/nodes?selector=country+%3D%3D", nil, viewerToken)
	if broken.Code != http.StatusBadRequest || errorCode(broken) != "invalid_selector" || errorDetails(broken)["position"] != 8.0 || errorDetails(broken)["end"] != 10.0 {
		test.Fatalf("a broken selector: %d %s", broken.Code, broken.Body.String())
	}
}

func TestANode(test *testing.T) {
	current := newHarness(test, nil)
	path := "/api/v1/nodes/" + deviceID + "/" + installationID
	if result := current.call(http.MethodGet, path, nil, viewerToken); result.Code != http.StatusOK || len(field(result, "sessions").([]any)) != 1 {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
	if key := current.backend.last("Node")[0].(inventory.Key); key.DeviceID != deviceID || key.InstallationID != installationID {
		test.Fatalf("key %+v", key)
	}
	for _, malformed := range []string{"/api/v1/nodes/XYZ/" + installationID, "/api/v1/nodes/" + strings.ToUpper(deviceID) + "/" + installationID, "/api/v1/nodes/" + deviceID + "/" + installationID[:31]} {
		if result := current.call(http.MethodGet, malformed, nil, viewerToken); result.Code != http.StatusBadRequest {
			test.Errorf("%s answered %d", malformed, result.Code)
		}
	}
	current.backend.failure = inventory.ErrNotFound
	if result := current.call(http.MethodGet, path, nil, viewerToken); result.Code != http.StatusNotFound || errorCode(result) != "not_found" {
		test.Fatalf("an unknown node answered %d", result.Code)
	}
}

func TestAnAdministratorRevokesADevice(test *testing.T) {
	current := newHarness(test, nil)
	path := "/api/v1/devices/" + deviceID + "/lifecycle"
	revoked := current.call(http.MethodPost, path, map[string]any{"lifecycle": "revoked", "reason": "stolen"}, adminToken)
	if revoked.Code != http.StatusOK || field(revoked, "lifecycle") != "revoked" || field(revoked, "device_id") != deviceID {
		test.Fatalf("revoke %d %s", revoked.Code, revoked.Body.String())
	}
	if arguments := current.backend.last("SetDeviceLifecycle"); arguments[0] != deviceID || arguments[1] != "revoked" || arguments[2] != "stolen" || arguments[3] != "token:00000000-0000-7000-8000-000000000003" {
		test.Fatalf("SetDeviceLifecycle %v", arguments)
	}
	if refused := current.call(http.MethodPost, path, map[string]any{"lifecycle": "revoked", "reason": "stolen"}, operatorToken); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator revoked a device: %d", refused.Code)
	}
	for _, body := range []map[string]any{{"lifecycle": "quarantined", "reason": "x"}, {"lifecycle": "revoked"}, {"lifecycle": "revoked", "reason": ""}} {
		if invalid := current.call(http.MethodPost, path, body, adminToken); invalid.Code != http.StatusBadRequest {
			test.Errorf("%v answered %d", body, invalid.Code)
		}
	}
	if malformed := current.call(http.MethodPost, "/api/v1/devices/XYZ/lifecycle", map[string]any{"lifecycle": "revoked", "reason": "stolen"}, adminToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("a malformed device id answered %d", malformed.Code)
	}
	node := current.call(http.MethodGet, "/api/v1/nodes/"+deviceID+"/"+installationID, nil, viewerToken)
	if device, _ := field(node, "device").(map[string]any); device["lifecycle"] != "active" {
		test.Fatalf("the node's device record %s", node.Body.String())
	}
}

func TestAnAdministratorRevokesEverythingOneCredentialEnrolled(test *testing.T) {
	current := newHarness(test, nil)
	body := map[string]any{"credential_kind": "fleet_token", "credential": "retail-eu-2026", "enrolled_after": "2026-10-01T02:00:00Z", "reason": "leaked token"}
	if refused := current.call(http.MethodPost, "/api/v1/revocations", body, operatorToken); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator started a revocation: %d", refused.Code)
	}
	started := current.call(http.MethodPost, "/api/v1/revocations", body, adminToken)
	if started.Code != http.StatusAccepted || field(started, "matched") != float64(41) {
		test.Fatalf("revocation %d %s", started.Code, started.Body.String())
	}
	arguments := current.backend.last("StartRevocation")
	request := arguments[0].(engine.RevocationRequest)
	if request.Credential != "retail-eu-2026" || request.EnrolledAfter == nil || !request.EnrolledAfter.Equal(time.Date(2026, 10, 1, 2, 0, 0, 0, time.UTC)) || arguments[1] != "token:00000000-0000-7000-8000-000000000003" {
		test.Fatalf("StartRevocation %v", arguments)
	}
	body["dry_run"] = true
	if counted := current.call(http.MethodPost, "/api/v1/revocations", body, adminToken); counted.Code != http.StatusOK || field(counted, "revocation") != nil {
		test.Fatalf("dry run %d %s", counted.Code, counted.Body.String())
	}
	if listed := current.call(http.MethodGet, "/api/v1/revocations", nil, viewerToken); listed.Code != http.StatusOK {
		test.Fatalf("list %d %s", listed.Code, listed.Body.String())
	}
	if found := current.call(http.MethodGet, "/api/v1/revocations/0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e90", nil, viewerToken); found.Code != http.StatusOK || field(found, "credential") != "retail-eu-2026" {
		test.Fatalf("get %d %s", found.Code, found.Body.String())
	}
	if malformed := current.call(http.MethodGet, "/api/v1/revocations/7", nil, viewerToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("a malformed revocation id answered %d", malformed.Code)
	}
}

func TestOperatorActionsOnANode(test *testing.T) {
	current := newHarness(test, nil)
	base := "/api/v1/nodes/" + deviceID + "/" + installationID
	session := current.call(http.MethodPost, base+"/sessions", map[string]any{"reason": "debugging a crash", "ttl_seconds": 900}, operatorToken)
	if session.Code != http.StatusCreated || field(session, "pid") != "17352987455937552665" || field(session, "node").(map[string]any)["nightfall"] != "nightfall-0:8444" {
		test.Fatalf("session %d %s", session.Code, session.Body.String())
	}
	if arguments := current.backend.last("OpenSession"); arguments[1] != "debugging a crash" || arguments[2] != 900 {
		test.Fatalf("OpenSession %v", arguments)
	}
	if logs := current.call(http.MethodPost, base+"/logs", map[string]any{"level": "debug", "duration_seconds": 600}, operatorToken); logs.Code != http.StatusAccepted || field(logs, "stream_id") != "stream-1" {
		test.Fatalf("logs %d %s", logs.Code, logs.Body.String())
	}
	if files := current.call(http.MethodPost, base+"/files", map[string]any{"path": "/var/log/syslog"}, operatorToken); files.Code != http.StatusAccepted || field(files, "upload_id") != "upload-1" {
		test.Fatalf("files %d %s", files.Code, files.Body.String())
	}
	if arguments := current.backend.last("CollectFile"); arguments[1] != "/var/log/syslog" {
		test.Fatalf("CollectFile %v", arguments)
	}
	if invalid := current.call(http.MethodPost, base+"/logs", map[string]any{"level": "verbose", "duration_seconds": 600}, operatorToken); invalid.Code != http.StatusBadRequest {
		test.Fatalf("an unknown level answered %d", invalid.Code)
	}
	cases := []struct {
		failure error
		status  int
		code    string
	}{
		{engine.ErrNodeOffline, http.StatusConflict, "node_offline"},
		{&dawn.Error{Outcome: dawn.Busy}, http.StatusServiceUnavailable, "dawn_busy"},
		{&dawn.Error{Outcome: dawn.NotDelivered}, http.StatusServiceUnavailable, "dawn_unavailable"},
		{&dawn.Error{Outcome: dawn.Rejected, Status: 400, Message: "path refused"}, http.StatusBadGateway, "dawn_rejected"},
		{&dawn.Error{Outcome: dawn.Ambiguous}, http.StatusGatewayTimeout, "dawn_unanswered"},
		{fmt.Errorf("%w: duration_seconds must be between 1 and 86400", engine.ErrInvalidArgument), http.StatusBadRequest, "invalid_argument"},
	}
	for _, scenario := range cases {
		current.backend.failure = scenario.failure
		result := current.call(http.MethodPost, base+"/logs", map[string]any{"level": "info", "duration_seconds": 60}, operatorToken)
		if result.Code != scenario.status || errorCode(result) != scenario.code {
			test.Errorf("%v answered %d %s", scenario.failure, result.Code, result.Body.String())
		}
		if scenario.code == "dawn_busy" && result.Header().Get("Retry-After") != "5" {
			test.Error("dawn_busy without Retry-After")
		}
		if scenario.code == "invalid_argument" && !strings.HasPrefix(field(result, "error").(map[string]any)["message"].(string), "duration_seconds") {
			test.Errorf("message %s", result.Body.String())
		}
	}
	current.backend.failure = fmt.Errorf("the node-state record was not acknowledged: %w", context.DeadlineExceeded)
	if timeout := current.call(http.MethodPost, base+"/lifecycle", map[string]any{"lifecycle": "quarantined", "reason": "x"}, operatorToken); timeout.Code != http.StatusServiceUnavailable || errorCode(timeout) != "timeout" {
		test.Fatalf("a timeout answered %d %s", timeout.Code, timeout.Body.String())
	}
}

func TestSelectorValidation(test *testing.T) {
	current := newHarness(test, nil)
	result := current.call(http.MethodPost, "/api/v1/selectors/validate", map[string]any{"selector": `os_name in ["debian"]`}, viewerToken)
	if result.Code != http.StatusOK || field(result, "ok") != true || current.backend.last("ValidateSelector")[0] != `os_name in ["debian"]` {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
	if missing := current.call(http.MethodPost, "/api/v1/selectors/validate", map[string]any{}, viewerToken); missing.Code != http.StatusBadRequest {
		test.Fatalf("a missing selector answered %d", missing.Code)
	}
}

func TestCampaignLifecycleThroughTheAPI(test *testing.T) {
	current := newHarness(test, nil)
	listed := current.call(http.MethodGet, "/api/v1/campaigns?status=running,paused&status=draft&limit=20&cursor=c1", nil, viewerToken)
	if listed.Code != http.StatusOK || len(items(listed)) != 1 || field(listed, "next_cursor") != nil {
		test.Fatalf("list %d %s", listed.Code, listed.Body.String())
	}
	if arguments := current.backend.last("Campaigns"); fmt.Sprint(arguments[0]) != "[running paused draft]" || arguments[1] != "c1" || arguments[2] != 20 {
		test.Fatalf("Campaigns %v", arguments)
	}
	if refused := current.call(http.MethodGet, "/api/v1/campaigns?status=sleeping", nil, viewerToken); refused.Code != http.StatusBadRequest {
		test.Fatalf("an unknown status answered %d", refused.Code)
	}
	definition := map[string]any{
		"name": "upgrade the US", "selector": `country == "US"`,
		"action": map[string]any{"kind": "ensure_version", "version": "0.2.0", "script": "upgrade"},
		"policy": map[string]any{"rate": map[string]any{"per_second": 5}, "phases": []any{map[string]any{"name": "canary", "percent": 1, "bake_seconds": 900}, map[string]any{"name": "all", "percent": 100, "bake_seconds": 900}}},
	}
	created := current.call(http.MethodPost, "/api/v1/campaigns", definition, operatorToken)
	if created.Code != http.StatusCreated || created.Header().Get("Location") != "/api/v1/campaigns/"+campaignIdentifier.String() || len(field(created, "counters").([]any)) != 1 {
		test.Fatalf("create %d %s %s", created.Code, created.Header(), created.Body.String())
	}
	sent := current.backend.last("CreateCampaign")[0].(campaign.Definition)
	if sent.Policy.Rate.PerSecond != 5 || len(sent.Policy.Phases) != 2 || sent.Action.Version != "0.2.0" || current.backend.last("CreateCampaign")[1] != "token:00000000-0000-7000-8000-000000000002" {
		test.Fatalf("CreateCampaign %+v", sent)
	}
	typo := map[string]any{"name": "x", "selector": "has(device_id)", "action": map[string]any{"kind": "run_script", "script": "ps"}, "policy": map[string]any{"gates": map[string]any{"max_failure_rat": 0.5}}}
	refused := current.call(http.MethodPost, "/api/v1/campaigns", typo, operatorToken)
	violations, _ := errorDetails(refused)["violations"].([]any)
	if refused.Code != http.StatusBadRequest || len(violations) == 0 || !strings.Contains(refused.Body.String(), "/policy/gates") || !strings.Contains(refused.Body.String(), "max_failure_rat") {
		test.Fatalf("a misspelled gate key: %d %s", refused.Code, refused.Body.String())
	}
	current.backend.failure = &campaign.ValidationError{Field: "policy.phases[1].bake_seconds", Message: "must be at least node_timeout_seconds (900)"}
	if invalid := current.call(http.MethodPost, "/api/v1/campaigns", definition, operatorToken); invalid.Code != http.StatusBadRequest || errorDetails(invalid)["field"] != "policy.phases[1].bake_seconds" {
		test.Fatalf("a validation error: %d %s", invalid.Code, invalid.Body.String())
	}
	current.backend.failure = nil
	path := "/api/v1/campaigns/" + campaignIdentifier.String()
	if read := current.call(http.MethodGet, path, nil, viewerToken); read.Code != http.StatusOK || field(read, "id") != campaignIdentifier.String() {
		test.Fatalf("get %d %s", read.Code, read.Body.String())
	}
	if noVersion := current.call(http.MethodPut, path, definition, operatorToken); noVersion.Code != http.StatusBadRequest {
		test.Fatalf("an edit without its version answered %d", noVersion.Code)
	}
	edit := map[string]any{"version": 3}
	for key, value := range definition {
		edit[key] = value
	}
	if edited := current.call(http.MethodPut, path, edit, operatorToken); edited.Code != http.StatusOK || current.backend.last("UpdateCampaign")[2] != 3 {
		test.Fatalf("edit %d %s", edited.Code, edited.Body.String())
	}
	current.backend.failure = campaign.ErrVersionConflict
	if conflict := current.call(http.MethodPut, path, edit, operatorToken); conflict.Code != http.StatusConflict || errorCode(conflict) != "version_conflict" {
		test.Fatalf("a stale edit answered %d", conflict.Code)
	}
	current.backend.failure = nil
	for _, scenario := range []struct {
		action, method string
		body           any
	}{
		{"start", "StartCampaign", nil},
		{"pause", "PauseCampaign", map[string]any{"reason": "investigating"}},
		{"resume", "ResumeCampaign", map[string]any{"override_gate": true, "reason": "known flaky hardware"}},
		{"abort", "AbortCampaign", map[string]any{"reason": "wrong script"}},
		{"complete", "CompleteCampaign", nil},
		{"archive", "ArchiveCampaign", map[string]any{}},
	} {
		result := current.call(http.MethodPost, path+"/"+scenario.action, scenario.body, operatorToken)
		if result.Code != http.StatusOK || current.backend.last(scenario.method) == nil {
			test.Errorf("%s: %d %s", scenario.action, result.Code, result.Body.String())
		}
	}
	if arguments := current.backend.last("ResumeCampaign"); arguments[2] != true || arguments[3] != "known flaky hardware" {
		test.Fatalf("ResumeCampaign %v", arguments)
	}
	if arguments := current.backend.last("AbortCampaign"); arguments[2] != "wrong script" {
		test.Fatalf("AbortCampaign %v", arguments)
	}
	if extra := current.call(http.MethodPost, path+"/start", map[string]any{"reason": "go"}, operatorToken); extra.Code != http.StatusBadRequest {
		test.Fatalf("start with a reason answered %d", extra.Code)
	}
	overlapping := uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e91")
	current.backend.failure = &engine.OverlapError{Count: 12, Campaigns: []uuid.UUID{overlapping}}
	overlap := current.call(http.MethodPost, path+"/start", nil, operatorToken)
	if overlap.Code != http.StatusConflict || errorCode(overlap) != "overlap" || errorDetails(overlap)["count"] != 12.0 || fmt.Sprint(errorDetails(overlap)["campaigns"]) != "["+overlapping.String()+"]" {
		test.Fatalf("overlap %d %s", overlap.Code, overlap.Body.String())
	}
	for failure, code := range map[error]string{
		campaign.ErrGateOverrideRequired:                                               "gate_override_required",
		fmt.Errorf("%w: cannot pause a draft campaign", campaign.ErrInvalidTransition): "invalid_transition",
		campaign.ErrNotArchivable:                                                      "not_archivable",
		campaign.ErrNotDraft:                                                           "not_draft",
	} {
		current.backend.failure = failure
		if result := current.call(http.MethodPost, path+"/resume", nil, operatorToken); result.Code != http.StatusConflict || errorCode(result) != code {
			test.Errorf("%v answered %d %s", failure, result.Code, result.Body.String())
		}
	}
	current.backend.failure = campaign.ErrNotFound
	if missing := current.call(http.MethodGet, path, nil, viewerToken); missing.Code != http.StatusNotFound {
		test.Fatalf("an unknown campaign answered %d", missing.Code)
	}
	if malformed := current.call(http.MethodGet, "/api/v1/campaigns/not-a-uuid", nil, viewerToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("a malformed id answered %d", malformed.Code)
	}
}

func TestCampaignNodesEventsAndGates(test *testing.T) {
	current := newHarness(test, nil)
	path := "/api/v1/campaigns/" + campaignIdentifier.String()
	rows := current.call(http.MethodGet, path+"/nodes?state=failed,unknown&phase=1&limit=10&cursor=r1", nil, viewerToken)
	if rows.Code != http.StatusOK || len(items(rows)) != 1 {
		test.Fatalf("nodes %d %s", rows.Code, rows.Body.String())
	}
	query := current.backend.last("CampaignNodes")[1].(campaign.RowQuery)
	if fmt.Sprint(query.States) != "[failed unknown]" || *query.Phase != 1 || query.Limit != 10 || query.After != "r1" {
		test.Fatalf("query %+v", query)
	}
	for _, bad := range []string{"state=lost", "phase=-1", "phase=one"} {
		if refused := current.call(http.MethodGet, path+"/nodes?"+bad, nil, viewerToken); refused.Code != http.StatusBadRequest {
			test.Errorf("%s answered %d", bad, refused.Code)
		}
	}
	retried := current.call(http.MethodPost, path+"/nodes/retry", map[string]any{"states": []string{"failed"}, "nodes": []any{map[string]string{"device_id": deviceID, "installation_id": installationID}}, "reason": "fixed the mirror"}, operatorToken)
	if retried.Code != http.StatusOK || field(retried, "count") != 3.0 {
		test.Fatalf("retry %d %s", retried.Code, retried.Body.String())
	}
	selection := current.backend.last("RetryNodes")[1].(engine.NodeSelection)
	if len(selection.Nodes) != 1 || selection.Nodes[0].DeviceID != deviceID || selection.Reason != "fixed the mirror" || fmt.Sprint(selection.States) != "[failed]" {
		test.Fatalf("selection %+v", selection)
	}
	if refused := current.call(http.MethodPost, path+"/nodes/retry", map[string]any{"nodes": []any{map[string]string{"device_id": "x", "installation_id": installationID}}, "reason": "r"}, operatorToken); refused.Code != http.StatusBadRequest {
		test.Fatalf("a malformed node answered %d", refused.Code)
	}
	if refused := current.call(http.MethodPost, path+"/nodes/retry", map[string]any{"states": []string{"failed"}}, operatorToken); refused.Code != http.StatusBadRequest {
		test.Fatalf("a retry without a reason answered %d", refused.Code)
	}
	resolved := current.call(http.MethodPost, path+"/nodes/resolve", map[string]any{"nodes": []any{map[string]string{"device_id": deviceID, "installation_id": installationID}}, "outcome": "succeeded", "reason": "checked by hand"}, operatorToken)
	if resolved.Code != http.StatusOK || field(resolved, "count") != 1.0 || current.backend.last("ResolveNodes")[2] != "succeeded" {
		test.Fatalf("resolve %d %s", resolved.Code, resolved.Body.String())
	}
	full := current.call(http.MethodGet, path+"/events?after=40&limit=2", nil, viewerToken)
	if full.Code != http.StatusOK || len(items(full)) != 2 || field(full, "next_cursor") != "42" {
		test.Fatalf("events %d %s", full.Code, full.Body.String())
	}
	if last := current.call(http.MethodGet, path+"/events?after=42&limit=5", nil, viewerToken); field(last, "next_cursor") != nil {
		test.Fatalf("a short page offered a next cursor: %s", last.Body.String())
	}
	if refused := current.call(http.MethodGet, path+"/events?after=x", nil, viewerToken); refused.Code != http.StatusBadRequest {
		test.Fatalf("a malformed after answered %d", refused.Code)
	}
	gates := current.call(http.MethodGet, path+"/gates", nil, viewerToken)
	if gates.Code != http.StatusOK || field(gates, "verdict") != "hold" || field(gates, "phase_name") != "canary" || field(gates, "min_sample") != float64(10) || field(gates, "required") != float64(10) {
		test.Fatalf("gates %d %s", gates.Code, gates.Body.String())
	}
}

func TestAlerts(test *testing.T) {
	current := newHarness(test, nil)
	if open := current.call(http.MethodGet, "/api/v1/alerts", nil, viewerToken); open.Code != http.StatusOK || current.backend.last("Alerts")[0] != true {
		test.Fatalf("open %d %s", open.Code, open.Body.String())
	}
	if all := current.call(http.MethodGet, "/api/v1/alerts?state=all&cursor=a1&limit=3", nil, viewerToken); all.Code != http.StatusOK || current.backend.last("Alerts")[0] != false || current.backend.last("Alerts")[1] != "a1" {
		test.Fatalf("all %d %v", all.Code, current.backend.last("Alerts"))
	}
	if refused := current.call(http.MethodGet, "/api/v1/alerts?state=closed", nil, viewerToken); refused.Code != http.StatusBadRequest {
		test.Fatalf("an unknown state answered %d", refused.Code)
	}
	acknowledged := current.call(http.MethodPost, "/api/v1/alerts/7/acknowledge", nil, operatorToken)
	if acknowledged.Code != http.StatusOK || field(acknowledged, "acknowledged_by") != "token:00000000-0000-7000-8000-000000000002" || current.backend.last("AcknowledgeAlert")[0] != int64(7) {
		test.Fatalf("acknowledge %d %s", acknowledged.Code, acknowledged.Body.String())
	}
	if resolved := current.call(http.MethodPost, "/api/v1/alerts/7/resolve", map[string]any{}, operatorToken); resolved.Code != http.StatusOK {
		test.Fatalf("resolve %d", resolved.Code)
	}
	if malformed := current.call(http.MethodPost, "/api/v1/alerts/0/resolve", nil, operatorToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("alert 0 answered %d", malformed.Code)
	}
	current.backend.failure = alerts.ErrNotFound
	if missing := current.call(http.MethodPost, "/api/v1/alerts/99/resolve", nil, operatorToken); missing.Code != http.StatusNotFound {
		test.Fatalf("an unknown alert answered %d", missing.Code)
	}
}

func TestRequestBodies(test *testing.T) {
	current := newHarness(test, nil)
	target := "/api/v1/nodes/" + deviceID + "/" + installationID + "/files"
	plain := newRequest(http.MethodPost, target, `{"path":"/etc/hosts"}`, operatorToken)
	plain.Header.Set("Content-Type", "text/plain")
	if result := current.do(plain); result.Code != http.StatusUnsupportedMediaType {
		test.Fatalf("text/plain answered %d", result.Code)
	}
	charset := newRequest(http.MethodPost, target, `{"path":"/etc/hosts"}`, operatorToken)
	charset.Header.Set("Content-Type", "application/json; charset=utf-8")
	if result := current.do(charset); result.Code != http.StatusAccepted {
		test.Fatalf("a JSON body with a charset answered %d", result.Code)
	}
	huge := `{"path":"` + strings.Repeat("a", maximumBodyBytes) + `"}`
	if result := current.call(http.MethodPost, target, huge, operatorToken); result.Code != http.StatusRequestEntityTooLarge {
		test.Fatalf("a body over 1 MiB answered %d", result.Code)
	}
	for name, body := range map[string]string{
		"empty":         "",
		"not JSON":      "{",
		"two values":    `{"path":"/a"} {"path":"/b"}`,
		"an array":      `["/a"]`,
		"unknown field": `{"path":"/a","recursive":true}`,
		"a wrong type":  `{"path":7}`,
	} {
		request := newRequest(http.MethodPost, target, body, operatorToken)
		request.Header.Set("Content-Type", "application/json")
		if result := current.do(request); result.Code != http.StatusBadRequest || errorCode(result) != "invalid_argument" {
			test.Errorf("%s answered %d %s", name, result.Code, result.Body.String())
		}
	}
}

func TestUnknownRoutesAndMethods(test *testing.T) {
	current := newHarness(test, func(options *Options) {
		options.UI = http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) { _, _ = writer.Write([]byte("the app")) })
	})
	missing := current.call(http.MethodGet, "/api/v1/nowhere", nil, "")
	if missing.Code != http.StatusNotFound || errorCode(missing) != "not_found" {
		test.Fatalf("%d %s", missing.Code, missing.Body.String())
	}
	wrongMethod := current.call(http.MethodDelete, "/api/v1/campaigns/"+campaignIdentifier.String(), nil, "")
	if wrongMethod.Code != http.StatusMethodNotAllowed || wrongMethod.Header().Get("Allow") != "GET, PUT" {
		test.Fatalf("%d %s", wrongMethod.Code, wrongMethod.Header())
	}
	if app := current.call(http.MethodGet, "/campaigns/x", nil, ""); app.Body.String() != "the app" {
		test.Fatalf("the UI was not served: %q", app.Body.String())
	}
}

func TestServerFailuresAreLoggedWithARequestID(test *testing.T) {
	current := newHarness(test, nil)
	current.backend.failure = errors.New("connection reset by peer")
	result := current.call(http.MethodGet, "/api/v1/overview", nil, viewerToken)
	identifier, _ := errorDetails(result)["request_id"].(string)
	if result.Code != http.StatusInternalServerError || errorCode(result) != "internal" || identifier == "" || identifier != result.Header().Get("X-Request-Id") {
		test.Fatalf("%d %s", result.Code, result.Body.String())
	}
	if strings.Contains(result.Body.String(), "connection reset") {
		test.Fatal("an internal error leaked to the caller")
	}
	if !strings.Contains(current.logs.String(), identifier) || !strings.Contains(current.logs.String(), "connection reset by peer") {
		test.Fatalf("the failure was not logged with its request id:\n%s", current.logs.String())
	}
	abandoned := newHarness(test, nil)
	abandoned.backend.failure = context.Canceled
	operation, cancel := context.WithCancel(context.Background())
	cancel()
	abandoned.server.ServeHTTP(httptest.NewRecorder(), newRequest(http.MethodGet, "/api/v1/overview", nil, viewerToken).WithContext(operation))
	if strings.Contains(abandoned.logs.String(), `"level":"ERROR"`) || !strings.Contains(abandoned.logs.String(), "ended by the client") {
		test.Fatalf("a request the client abandoned was logged as a failure:\n%s", abandoned.logs.String())
	}
}

func TestACampaignsOverlapIsAnswered(test *testing.T) {
	current := newHarness(test, nil)
	path := "/api/v1/campaigns/" + campaignIdentifier.String() + "/overlap"
	overlap := current.call(http.MethodGet, path, nil, viewerToken)
	if overlap.Code != http.StatusOK || field(overlap, "count") != 12.0 || fmt.Sprint(field(overlap, "campaigns")) != "[0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e91]" {
		test.Fatalf("overlap %d %s", overlap.Code, overlap.Body.String())
	}
	if arguments := current.backend.last("Overlap"); arguments[0] != campaignIdentifier {
		test.Fatalf("Overlap %v", arguments)
	}
	current.backend.failure = campaign.ErrNotFound
	if missing := current.call(http.MethodGet, path, nil, viewerToken); missing.Code != http.StatusNotFound {
		test.Fatalf("the overlap of an unknown campaign answered %d", missing.Code)
	}
}

func TestRequestsWaitForASlotUntilTheirDeadline(test *testing.T) {
	current := newHarness(test, func(options *Options) {
		options.MaxRequests = 1
		options.RequestTimeout = 300 * time.Millisecond
	})
	current.backend.entered, current.backend.hold = make(chan struct{}, 1), make(chan struct{})
	first := make(chan response, 1)
	started := time.Now()
	go func() { first <- current.call(http.MethodGet, "/api/v1/overview", nil, viewerToken) }()
	<-current.backend.entered
	waited := current.call(http.MethodGet, "/api/v1/me", nil, viewerToken)
	if waited.Code != http.StatusServiceUnavailable || errorCode(waited) != "busy" || waited.Header().Get("Retry-After") != "1" {
		test.Fatalf("a request with no free slot answered %d %s", waited.Code, waited.Body.String())
	}
	close(current.backend.hold)
	if held := <-first; held.Code != http.StatusOK {
		test.Fatalf("the request holding the slot answered %d %s", held.Code, held.Body.String())
	}
	current.backend.mutex.Lock()
	deadline := current.backend.deadline
	current.backend.mutex.Unlock()
	if deadline.IsZero() || deadline.After(started.Add(time.Second)) {
		test.Fatalf("the backend saw the deadline %v for a request bounded to 300 ms", deadline)
	}
	if me := current.call(http.MethodGet, "/api/v1/me", nil, viewerToken); me.Code != http.StatusOK {
		test.Fatalf("the slot was not given back: %d", me.Code)
	}
	current.backend.failure = context.DeadlineExceeded
	if late := current.call(http.MethodGet, "/api/v1/overview", nil, viewerToken); late.Code != http.StatusServiceUnavailable || errorCode(late) != "timeout" {
		test.Fatalf("a request that ran past its deadline answered %d %s", late.Code, late.Body.String())
	}
	if !strings.Contains(current.logs.String(), `"msg":"an api request waited out its deadline for a slot"`) {
		test.Fatal("a refused request was not logged")
	}
}
