package api

import (
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/erasure"
)

func TestErasingADeviceNeedsTheAdminRoleAndAReason(test *testing.T) {
	current := newHarness(test, nil)
	path := "/api/v1/devices/" + deviceID + "/erasure"
	if refused := current.call(http.MethodPost, path, map[string]any{"reason": "ticket 1"}, operatorToken); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator's erasure answered %d", refused.Code)
	}
	if missing := current.call(http.MethodPost, path, map[string]any{}, adminToken); missing.Code != http.StatusBadRequest {
		test.Fatalf("an erasure without a reason answered %d", missing.Code)
	}
	if malformed := current.call(http.MethodPost, "/api/v1/devices/NOT-A-DEVICE/erasure", map[string]any{"reason": "ticket 1"}, adminToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("an erasure of a malformed device id answered %d", malformed.Code)
	}
	started := current.call(http.MethodPost, path, map[string]any{"reason": "erasure request 4711"}, adminToken)
	if started.Code != http.StatusAccepted || started.Header().Get("Location") != "/api/v1/erasures/"+erasureIdentifier.String() || field(started, "device_id") != deviceID {
		test.Fatalf("the erasure answered %d %v %s", started.Code, started.Header(), started.Body.String())
	}
	if arguments := current.backend.last("EraseDevice"); arguments[1] != "erasure request 4711" || arguments[2] != "token:00000000-0000-7000-8000-000000000003" {
		test.Fatalf("the backend was asked %v", arguments)
	}
	current.backend.existing = true
	if running := current.call(http.MethodPost, path, map[string]any{"reason": "again"}, adminToken); running.Code != http.StatusAccepted || running.Header().Get("Location") != "" {
		test.Fatalf("an erasure answered by a running one answered %d %v", running.Code, running.Header())
	}
}

func TestAnErasureOfADeviceThatCanStillConnectIsAConflict(test *testing.T) {
	current := newHarness(test, nil)
	current.backend.failure = &erasure.NotReadyError{Subject: "device " + deviceID, NotWithdrawn: 2, Online: 1}
	refused := current.call(http.MethodPost, "/api/v1/devices/"+deviceID+"/erasure", map[string]any{"reason": "ticket 1"}, adminToken)
	details := errorDetails(refused)
	if refused.Code != http.StatusConflict || errorCode(refused) != "not_ready" || details["not_withdrawn"] != float64(2) || details["online"] != float64(1) {
		test.Fatalf("answered %d %s", refused.Code, refused.Body.String())
	}
	if !strings.Contains(refused.Body.String(), "retire or revoke it") {
		test.Fatalf("the message does not say what to do: %s", refused.Body.String())
	}
	current.backend.failure = erasure.ErrInvalidReason
	if blank := current.call(http.MethodPost, "/api/v1/devices/"+deviceID+"/erasure", map[string]any{"reason": "   "}, adminToken); blank.Code != http.StatusBadRequest {
		test.Fatalf("a blank reason answered %d", blank.Code)
	}
}

func TestErasingATenant(test *testing.T) {
	current := newHarness(test, nil)
	if malformed := current.call(http.MethodPost, "/api/v1/tenants/Acme_Corp/erasure", map[string]any{"reason": "contract ended"}, adminToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("a malformed tenant answered %d", malformed.Code)
	}
	started := current.call(http.MethodPost, "/api/v1/tenants/acme/erasure", map[string]any{"reason": "contract ended"}, adminToken)
	if started.Code != http.StatusAccepted || field(started, "tenant") != "acme" || field(started, "scope") != "tenant" {
		test.Fatalf("the tenant erasure answered %d %s", started.Code, started.Body.String())
	}
	current.backend.failure = &erasure.NotReadyError{Subject: "tenant acme", RunningCampaigns: 3}
	if refused := current.call(http.MethodPost, "/api/v1/tenants/acme/erasure", map[string]any{"reason": "contract ended"}, adminToken); refused.Code != http.StatusConflict ||
		errorDetails(refused)["running_campaigns"] != float64(3) {
		test.Fatalf("answered %d %s", refused.Code, refused.Body.String())
	}
}

func TestFollowingAnErasure(test *testing.T) {
	current := newHarness(test, nil)
	if malformed := current.call(http.MethodGet, "/api/v1/erasures/seven", nil, adminToken); malformed.Code != http.StatusBadRequest {
		test.Fatalf("a malformed erasure id answered %d", malformed.Code)
	}
	if refused := current.call(http.MethodGet, "/api/v1/erasures/"+erasureIdentifier.String(), nil, operatorToken); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator reading an erasure answered %d", refused.Code)
	}
	found := current.call(http.MethodGet, "/api/v1/erasures/"+erasureIdentifier.String(), nil, adminToken)
	if found.Code != http.StatusOK || field(found, "status") != erasure.StatusRunning {
		test.Fatalf("answered %d %s", found.Code, found.Body.String())
	}
	if identifier := current.backend.last("Erasure")[0].(uuid.UUID); identifier != erasureIdentifier {
		test.Fatalf("asked for %s", identifier)
	}
	current.backend.failure = erasure.ErrNotFound
	if missing := current.call(http.MethodGet, "/api/v1/erasures/"+uuid.NewString(), nil, adminToken); missing.Code != http.StatusNotFound {
		test.Fatalf("an unknown erasure answered %d", missing.Code)
	}
}

func TestExportingADeviceStreamsJSONLinesOneAtATime(test *testing.T) {
	current := newHarness(test, func(options *Options) { options.RequestTimeout = 50 * time.Millisecond })
	path := "/api/v1/devices/" + deviceID + "/export"
	if refused := current.call(http.MethodPost, path, map[string]any{"reason": "subject access request"}, operatorToken); refused.Code != http.StatusForbidden {
		test.Fatalf("an operator's export answered %d", refused.Code)
	}
	if blank := current.call(http.MethodPost, path, map[string]any{"reason": " "}, adminToken); blank.Code != http.StatusBadRequest {
		test.Fatalf("an export without a reason answered %d", blank.Code)
	}
	current.backend.entered, current.backend.hold = make(chan struct{}, 1), make(chan struct{})
	first := make(chan response, 1)
	go func() {
		first <- current.call(http.MethodPost, path, map[string]any{"reason": "subject access request"}, adminToken)
	}()
	<-current.backend.entered
	busy := current.call(http.MethodPost, path, map[string]any{"reason": "subject access request"}, adminToken)
	if busy.Code != http.StatusTooManyRequests || errorCode(busy) != "export_running" || busy.Header().Get("Retry-After") != "60" {
		test.Fatalf("a second export answered %d %s", busy.Code, busy.Body.String())
	}
	time.Sleep(100 * time.Millisecond)
	close(current.backend.hold)
	exported := <-first
	lines := strings.Split(strings.TrimSpace(exported.Body.String()), "\n")
	if exported.Code != http.StatusOK || exported.Header().Get("Content-Type") != "application/x-ndjson" || len(lines) != 2 || !strings.HasPrefix(lines[1], `{"summary"`) {
		test.Fatalf("the export answered %d %v %q", exported.Code, exported.Header(), exported.Body.String())
	}
	if arguments := current.backend.last("ExportDevice"); arguments[0] != deviceID || arguments[1] != "subject access request" {
		test.Fatalf("the backend was asked %v", arguments)
	}
}
