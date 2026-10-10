package alerts

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/config"
)

func secretFile(test *testing.T, content string) string {
	test.Helper()
	path := filepath.Join(test.TempDir(), "secret")
	if failure := os.WriteFile(path, []byte(content), 0o600); failure != nil {
		test.Fatal(failure)
	}
	return path
}

func TestRoutesPickReceiversBySeverityKindAndTenant(test *testing.T) {
	key := secretFile(test, routingKey+"\n")
	settings := config.Alerts{
		DeliveryHorizonSeconds: 60,
		Receivers: []config.Receiver{
			{Name: "noc", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: key}},
			{Name: "tenant-a", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: key}},
			{Name: "chat", Slack: &config.SlackReceiver{WebhookURLFile: key}},
			{Name: "siem", Webhook: &config.WebhookReceiver{URL: "https://siem.example.org", SecretFile: key}},
		},
		Routes: []config.Route{
			{Receiver: "noc", Severities: []string{"critical"}},
			{Receiver: "tenant-a", Severities: []string{"critical", "high"}, Tenants: []string{"a"}},
			{Receiver: "chat", Severities: []string{"medium", "low"}},
			{Receiver: "siem"},
			{Receiver: "noc", Kinds: []string{KindLedgerChainBroken}},
		},
	}
	built, failure := newNotifier(settings, "twilight-0", slog.New(slog.DiscardHandler))
	if failure != nil {
		test.Fatal(failure)
	}
	tenant := "a"
	other := "b"
	cases := []struct {
		severity Severity
		kind     string
		tenant   *string
		want     []string
	}{
		{Critical, KindProcessWithoutIntent, &tenant, []string{"noc", "tenant-a", "siem"}},
		{High, KindPidReused, &tenant, []string{"tenant-a", "siem"}},
		{High, KindPidReused, &other, []string{"siem"}},
		{Medium, KindCampaignConflict, nil, []string{"chat", "siem"}},
		{High, KindLedgerChainBroken, nil, []string{"siem", "noc"}},
		{Critical, KindLedgerChainBroken, nil, []string{"noc", "siem"}},
	}
	for _, entry := range cases {
		if got := built.match(entry.severity, entry.kind, entry.tenant); !slices.Equal(got, entry.want) {
			test.Errorf("%s %s %v: %v, want %v", entry.severity, entry.kind, entry.tenant, got, entry.want)
		}
	}
	if !slices.Equal(built.names, []string{"noc", "tenant-a", "chat", "siem"}) {
		test.Errorf("names %v", built.names)
	}
}

func TestAReceiverWithoutItsSecretStopsTwilight(test *testing.T) {
	for _, path := range []string{filepath.Join(test.TempDir(), "absent"), secretFile(test, " \n")} {
		_, failure := newNotifier(config.Alerts{Receivers: []config.Receiver{{Name: "chat", Slack: &config.SlackReceiver{WebhookURLFile: path}}}}, "twilight-0", slog.New(slog.DiscardHandler))
		if failure == nil || !strings.Contains(failure.Error(), "alerts receiver chat") {
			test.Errorf("%s: %v", path, failure)
		}
	}
}

func TestReceiverAnswersDecideRetry(test *testing.T) {
	status := 0
	retryAfter := ""
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if retryAfter != "" {
			writer.Header().Set("Retry-After", retryAfter)
		}
		if status == http.StatusFound {
			writer.Header().Set("Location", "https://elsewhere.example.org")
		}
		writer.WriteHeader(status)
		_, _ = writer.Write([]byte("invalid_payload\n"))
	}))
	defer server.Close()
	client := &http.Client{Timeout: time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	cases := []struct {
		status    int
		after     string
		success   bool
		permanent bool
		wait      time.Duration
	}{
		{http.StatusAccepted, "", true, false, 0},
		{http.StatusOK, "", true, false, 0},
		{http.StatusBadRequest, "", false, true, 0},
		{http.StatusNotFound, "", false, true, 0},
		{http.StatusFound, "", false, true, 0},
		{http.StatusRequestTimeout, "", false, false, 0},
		{http.StatusTooManyRequests, "120", false, false, 2 * time.Minute},
		{http.StatusTooManyRequests, "86400", false, false, retryAfterCeiling},
		{http.StatusInternalServerError, "", false, false, 0},
		{http.StatusServiceUnavailable, "", false, false, 0},
	}
	for _, entry := range cases {
		status, retryAfter = entry.status, entry.after
		failure := postJSON(context.Background(), client, server.URL, []byte(`{}`), nil)
		if entry.success {
			if failure != nil {
				test.Errorf("%d: %v", entry.status, failure)
			}
			continue
		}
		var answered deliveryError
		if !errors.As(failure, &answered) || answered.permanent != entry.permanent || answered.after != entry.wait {
			test.Errorf("%d: %+v", entry.status, failure)
		}
		if !strings.Contains(answered.message, "invalid_payload") {
			test.Errorf("%d: the receiver's answer is not kept: %q", entry.status, answered.message)
		}
	}
}

func TestDeliveryErrorsNeverCarryTheSecretURL(test *testing.T) {
	listener, failure := net.Listen("tcp", "127.0.0.1:0")
	if failure != nil {
		test.Fatal(failure)
	}
	address := listener.Addr().String()
	listener.Close()
	secret := "http://" + address + "/services/T000/B000/hunter2hunter2"
	client := &http.Client{Timeout: time.Second}
	failure = postJSON(context.Background(), client, secret, []byte(`{}`), nil)
	var answered deliveryError
	if !errors.As(failure, &answered) || answered.permanent {
		test.Fatalf("a refused connection is not retried: %v", failure)
	}
	if strings.Contains(answered.message, "hunter2") || strings.Contains(answered.message, "/services/") {
		test.Errorf("the secret URL leaked into %q", answered.message)
	}
	failure = postJSON(context.Background(), client, "http://in valid/hunter2", []byte(`{}`), nil)
	if !errors.As(failure, &answered) || !answered.permanent || strings.Contains(answered.message, "hunter2") {
		test.Errorf("an invalid URL: %v", failure)
	}
}

func TestBackoffStaysWithinItsBounds(test *testing.T) {
	for attempts := range 40 {
		ceiling := retryCap
		if attempts < 20 {
			ceiling = min(retryCap, retryBase<<max(attempts-1, 0))
		}
		for range 50 {
			if delay := backoff(attempts); delay < time.Second || delay > max(ceiling, time.Second) {
				test.Fatalf("attempt %d waited %s, outside 1s..%s", attempts, delay, ceiling)
			}
		}
	}
}

func TestPrintableTrimsAnswers(test *testing.T) {
	if got := printable("  bad\r\ngateway\x00 "); got != "bad  gateway " {
		test.Errorf("%q", got)
	}
	if got := printable(strings.Repeat("a", 1000)); len(got) != errorExcerpt+3 {
		test.Errorf("%d", len(got))
	}
}
