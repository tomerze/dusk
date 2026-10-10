//go:build integration

package alerts_test

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/hmac"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/testcontainers/testcontainers-go"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/testsupport"
)

const (
	pagerDutyKey  = "0123456789abcdef0123456789abcdef"
	webhookSecret = "a webhook secret"
	smtpPassword  = "mail-secret"
)

var mailCertificate, mailKey string

func writeMailCertificate() error {
	key, failure := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if failure != nil {
		return failure
	}
	template := &x509.Certificate{
		SerialNumber: big.NewInt(time.Now().UnixNano()), Subject: pkix.Name{CommonName: "localhost"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour),
		DNSNames: []string{"localhost"}, IPAddresses: []net.IP{net.ParseIP("127.0.0.1")},
		KeyUsage: x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true, IsCA: true,
	}
	certificate, failure := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if failure != nil {
		return failure
	}
	encodedKey, failure := x509.MarshalPKCS8PrivateKey(key)
	if failure != nil {
		return failure
	}
	directory, failure := os.MkdirTemp("", "twilight-mail-")
	if failure != nil {
		return failure
	}
	mailCertificate, mailKey = filepath.Join(directory, "tls.crt"), filepath.Join(directory, "tls.key")
	if failure := os.WriteFile(mailCertificate, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: certificate}), 0o644); failure != nil {
		return failure
	}
	return os.WriteFile(mailKey, pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: encodedKey}), 0o644)
}

type recorded struct {
	path           string
	action         string
	idempotencyKey string
	body           string
	status         int
}

type sink struct {
	mutex    sync.Mutex
	requests []recorded
	failing  map[string]bool
	server   *httptest.Server
	test     *testing.T
}

func newSink(test *testing.T) *sink {
	receiving := &sink{failing: map[string]bool{}, test: test}
	receiving.server = httptest.NewServer(http.HandlerFunc(receiving.serve))
	test.Cleanup(receiving.server.Close)
	return receiving
}

func (receiving *sink) serve(writer http.ResponseWriter, request *http.Request) {
	body, _ := io.ReadAll(request.Body)
	entry := recorded{path: request.URL.Path, body: string(body), idempotencyKey: request.Header.Get(alerts.IdempotencyKeyHeader)}
	switch request.URL.Path {
	case "/pagerduty":
		var event struct {
			EventAction string `json:"event_action"`
			RoutingKey  string `json:"routing_key"`
		}
		_ = json.Unmarshal(body, &event)
		entry.action = event.EventAction
		if event.RoutingKey != pagerDutyKey {
			entry.status = http.StatusBadRequest
		}
	case "/webhook":
		var notification struct {
			Transition string `json:"transition"`
		}
		_ = json.Unmarshal(body, &notification)
		entry.action = notification.Transition
		timestamp := request.Header.Get(alerts.TimestampHeader)
		seconds, failure := strconv.ParseInt(timestamp, 10, 64)
		expected := alerts.Sign([]byte(webhookSecret), timestamp, body)
		given, _ := hex.DecodeString(request.Header.Get(alerts.SignatureHeader))
		wanted, _ := hex.DecodeString(expected)
		if failure != nil || time.Since(time.Unix(seconds, 0)).Abs() > 5*time.Minute || !hmac.Equal(given, wanted) {
			entry.status = http.StatusUnauthorized
		}
	case "/slack":
		var message struct {
			Text string `json:"text"`
		}
		_ = json.Unmarshal(body, &message)
		entry.action = strings.SplitN(message.Text, "]", 2)[0]
	case "/teams":
		var message struct {
			Attachments []struct {
				Content struct {
					Body []struct {
						Text string `json:"text"`
					} `json:"body"`
				} `json:"content"`
			} `json:"attachments"`
		}
		_ = json.Unmarshal(body, &message)
		if len(message.Attachments) == 1 && len(message.Attachments[0].Content.Body) > 0 {
			entry.action = strings.SplitN(strings.ReplaceAll(message.Attachments[0].Content.Body[0].Text, `\`, ""), "]", 2)[0]
		}
	}
	receiving.mutex.Lock()
	if entry.status == 0 && receiving.failing[request.URL.Path] {
		entry.status = http.StatusServiceUnavailable
	}
	if entry.status == 0 {
		entry.status = http.StatusAccepted
	}
	receiving.requests = append(receiving.requests, entry)
	receiving.mutex.Unlock()
	writer.WriteHeader(entry.status)
	if request.URL.Path == "/pagerduty" && entry.status == http.StatusAccepted {
		_, _ = writer.Write([]byte(`{"status":"success","message":"Event processed"}`))
	}
}

func (receiving *sink) fail(path string, failing bool) {
	receiving.mutex.Lock()
	defer receiving.mutex.Unlock()
	receiving.failing[path] = failing
}

func (receiving *sink) accepted(path string) []recorded {
	receiving.mutex.Lock()
	defer receiving.mutex.Unlock()
	var found []recorded
	for _, entry := range receiving.requests {
		if entry.path == path && entry.status == http.StatusAccepted {
			found = append(found, entry)
		}
	}
	return found
}

func (receiving *sink) all(path string) []recorded {
	receiving.mutex.Lock()
	defer receiving.mutex.Unlock()
	var found []recorded
	for _, entry := range receiving.requests {
		if entry.path == path {
			found = append(found, entry)
		}
	}
	return found
}

func writeSecret(test *testing.T, content string) string {
	test.Helper()
	path := filepath.Join(test.TempDir(), "secret")
	if failure := os.WriteFile(path, []byte(content+"\n"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	return path
}

func eventually(test *testing.T, what string, timeout time.Duration, check func() (bool, string)) {
	test.Helper()
	deadline := time.Now().Add(timeout)
	for {
		done, state := check()
		if done {
			return
		}
		if time.Now().After(deadline) {
			test.Fatalf("%s: still %s after %s", what, state, timeout)
		}
		time.Sleep(100 * time.Millisecond)
	}
}

type mailbox struct {
	api string
}

type mailMessage struct {
	ID      string `json:"ID"`
	Subject string `json:"Subject"`
}

func (box mailbox) messages(test *testing.T) []mailMessage {
	test.Helper()
	response, failure := http.Get(box.api + "/api/v1/messages")
	if failure != nil {
		test.Fatal(failure)
	}
	defer response.Body.Close()
	var listed struct {
		Messages []mailMessage `json:"messages"`
	}
	if failure := json.NewDecoder(response.Body).Decode(&listed); failure != nil {
		test.Fatal(failure)
	}
	return listed.Messages
}

func (box mailbox) text(test *testing.T, identifier string) string {
	test.Helper()
	response, failure := http.Get(box.api + "/api/v1/message/" + identifier)
	if failure != nil {
		test.Fatal(failure)
	}
	defer response.Body.Close()
	var message struct {
		Text string `json:"Text"`
		HTML string `json:"HTML"`
	}
	if failure := json.NewDecoder(response.Body).Decode(&message); failure != nil {
		test.Fatal(failure)
	}
	return message.Text + message.HTML
}

func startMailpit(test *testing.T, environment map[string]string) (string, int, mailbox) {
	test.Helper()
	files := []testcontainers.ContainerFile{
		{HostFilePath: mailCertificate, ContainerFilePath: "/certificates/tls.crt", FileMode: 0o644},
		{HostFilePath: mailKey, ContainerFilePath: "/certificates/tls.key", FileMode: 0o644},
	}
	environment["MP_SMTP_TLS_CERT"] = "/certificates/tls.crt"
	environment["MP_SMTP_TLS_KEY"] = "/certificates/tls.key"
	environment["MP_SMTP_AUTH"] = "dusk:" + smtpPassword
	smtpEndpoint, apiEndpoint := testsupport.Mailpit(test, environment, files...)
	host, port, failure := net.SplitHostPort(smtpEndpoint)
	if failure != nil {
		test.Fatal(failure)
	}
	number, _ := strconv.Atoi(port)
	return host, number, mailbox{api: apiEndpoint}
}

func notifications(test *testing.T, receiving *sink, mailHost string, mailPort int, security string) config.Alerts {
	password := writeSecret(test, smtpPassword)
	return config.Alerts{
		PublicURL: "https://twilight.example.org", RunbookURL: "https://docs.example.org/stack/runbooks/{kind}/", DeliveryHorizonSeconds: 86400,
		Receivers: []config.Receiver{
			{Name: "on-call", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: writeSecret(test, pagerDutyKey), URL: receiving.server.URL + "/pagerduty"}},
			{Name: "chat", Slack: &config.SlackReceiver{WebhookURLFile: writeSecret(test, receiving.server.URL+"/slack")}},
			{Name: "noc", Teams: &config.TeamsReceiver{WebhookURLFile: writeSecret(test, receiving.server.URL+"/teams")}},
			{Name: "siem", Webhook: &config.WebhookReceiver{URL: receiving.server.URL + "/webhook", SecretFile: writeSecret(test, webhookSecret)}},
			{Name: "soc", Email: &config.EmailReceiver{Host: mailHost, Port: mailPort, Security: security, Username: "dusk", PasswordFile: password,
				From: "dusk@example.org", To: []string{"soc@example.org"}}},
		},
		Routes: []config.Route{{Receiver: "on-call"}, {Receiver: "chat"}, {Receiver: "noc"}, {Receiver: "siem"}, {Receiver: "soc"}},
	}
}

func running(test *testing.T, pool *pgxpool.Pool, settings config.Alerts, instance string) (*alerts.Store, func()) {
	test.Helper()
	store, failure := alerts.NewStore(pool, settings, instance, testsupport.Logger())
	if failure != nil {
		test.Fatal(failure)
	}
	operation, cancel := context.WithCancel(context.Background())
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		store.RunDelivery(operation)
	}()
	stop := sync.OnceFunc(func() {
		cancel()
		<-stopped
	})
	test.Cleanup(stop)
	return store, stop
}

func states(alert alerts.Alert) string {
	var parts []string
	for _, delivery := range alert.Deliveries {
		parts = append(parts, fmt.Sprintf("%s/%s=%s(%d)", delivery.Receiver, delivery.Transition, delivery.State, delivery.Attempts))
	}
	return strings.Join(parts, " ")
}

func allDelivered(test *testing.T, store *alerts.Store, identifier int64, count int) func() (bool, string) {
	return func() (bool, string) {
		alert, failure := store.Get(context.Background(), identifier)
		if failure != nil {
			test.Fatal(failure)
		}
		delivered := 0
		for _, delivery := range alert.Deliveries {
			if delivery.State == "delivered" {
				delivered++
			}
		}
		return delivered == count && len(alert.Deliveries) == count, states(alert)
	}
}

func TestEveryTransitionReachesEveryReceiverOnce(test *testing.T) {
	receiving := newSink(test)
	host, port, box := startMailpit(test, map[string]string{"MP_SMTP_REQUIRE_STARTTLS": "true"})
	pool, _ := testsupport.Database(test)
	store, _ := running(test, pool, notifications(test, receiving, host, port, config.EmailStartTLS), "twilight-0")
	operation := context.Background()
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id, hostname, tenant) values ($1, $2, 'camera-17', 'acme')`,
		"0123456789abcdef0123456789abcdef", "fedcba9876543210fedcba9876543210"); failure != nil {
		test.Fatal(failure)
	}
	raised := alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindProcessWithoutIntent, Fingerprint: "process_without_intent:12808937078074471924",
		Detail: map[string]any{"message": "a client created a process at a pid twilight never intended", "pid": "12808937078074471924",
			"device_id": "0123456789abcdef0123456789abcdef", "installation_id": "fedcba9876543210fedcba9876543210", "principal": "dawn-0"}}
	alert, failure := store.Raise(operation, raised)
	if failure != nil {
		test.Fatal(failure)
	}
	if alert.Tenant == nil || *alert.Tenant != "acme" {
		test.Fatalf("the alert did not take its node's tenant: %v", alert.Tenant)
	}
	eventually(test, "the opening reaches every receiver", 30*time.Second, allDelivered(test, store, alert.ID, 5))
	if _, failure := store.Raise(operation, raised); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Acknowledge(operation, alert.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the acknowledgement reaches every receiver", 30*time.Second, allDelivered(test, store, alert.ID, 10))
	if _, failure := store.Resolve(operation, alert.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the resolution reaches every receiver", 30*time.Second, allDelivered(test, store, alert.ID, 15))
	time.Sleep(2 * time.Second)
	pagerDuty := receiving.accepted("/pagerduty")
	if len(pagerDuty) != 3 || pagerDuty[0].action != "trigger" || pagerDuty[1].action != "acknowledge" || pagerDuty[2].action != "resolve" {
		test.Fatalf("PagerDuty got %d events: %+v", len(pagerDuty), pagerDuty)
	}
	for _, entry := range pagerDuty {
		if !strings.Contains(entry.body, `"dedup_key":"process_without_intent:12808937078074471924"`) {
			test.Errorf("a PagerDuty event without the fingerprint as its dedup key: %s", entry.body)
		}
	}
	webhook := receiving.accepted("/webhook")
	if len(webhook) != 3 || webhook[0].action != "opened" || webhook[1].action != "acknowledged" || webhook[2].action != "resolved" ||
		webhook[0].idempotencyKey == webhook[1].idempotencyKey || len(receiving.all("/webhook")) != 3 {
		test.Fatalf("the webhook got %+v", webhook)
	}
	for _, path := range []string{"/slack", "/teams"} {
		if got := receiving.accepted(path); len(got) != 3 || got[0].action != "[CRITICAL" || got[1].action != "[ACKNOWLEDGED" || got[2].action != "[RESOLVED" {
			test.Fatalf("%s got %+v", path, got)
		}
	}
	messages := box.messages(test)
	if len(messages) != 3 {
		test.Fatalf("Mailpit holds %d messages: %+v", len(messages), messages)
	}
	subjects := map[string]string{}
	for _, message := range messages {
		subjects[message.Subject] = message.ID
	}
	opened, found := subjects["[CRITICAL] Process nobody intended on camera-17"]
	if !found || subjects["[ACKNOWLEDGED] Process nobody intended on camera-17"] == "" || subjects["[RESOLVED] Process nobody intended on camera-17"] == "" {
		test.Fatalf("mail subjects %v", subjects)
	}
	if body := box.text(test, opened); !strings.Contains(body, "https://docs.example.org/stack/runbooks/process_without_intent/") ||
		!strings.Contains(body, "https://twilight.example.org/alerts?alert="+strconv.FormatInt(alert.ID, 10)) || !strings.Contains(body, "acme") {
		test.Errorf("the opening mail lacks its links or tenant: %s", body)
	}
}

func TestAnAlertRaisedAgainHigherIsReEscalatedToTheNewRoutes(test *testing.T) {
	receiving := newSink(test)
	pool, _ := testsupport.Database(test)
	settings := config.Alerts{
		PublicURL: "https://twilight.example.org", RunbookURL: "https://docs.example.org/{kind}/", DeliveryHorizonSeconds: 86400,
		Receivers: []config.Receiver{
			{Name: "on-call", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: writeSecret(test, pagerDutyKey), URL: receiving.server.URL + "/pagerduty"}},
			{Name: "chat", Slack: &config.SlackReceiver{WebhookURLFile: writeSecret(test, receiving.server.URL+"/slack")}},
		},
		Routes: []config.Route{{Receiver: "on-call", Severities: []string{"critical"}}, {Receiver: "chat", Severities: []string{"medium", "low"}}},
	}
	store, _ := running(test, pool, settings, "twilight-0")
	operation := context.Background()
	medium := alerts.Raised{Severity: alerts.Medium, Kind: alerts.KindEnrollmentRate, Fingerprint: "enrollment_rate", Detail: map[string]any{"threshold": 600}}
	alert, failure := store.Raise(operation, medium)
	if failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the medium alert reaches the chat only", 30*time.Second, allDelivered(test, store, alert.ID, 1))
	if _, failure := store.Acknowledge(operation, alert.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the acknowledgement reaches the chat", 30*time.Second, allDelivered(test, store, alert.ID, 2))
	critical := medium
	critical.Severity = alerts.Critical
	critical.Detail = map[string]any{"threshold": 600, "count": 9000}
	escalated, failure := store.Raise(operation, critical)
	if failure != nil {
		test.Fatal(failure)
	}
	if escalated.ID != alert.ID || escalated.Severity != alerts.Critical || escalated.AcknowledgedAt != nil || !strings.Contains(string(escalated.Detail), "9000") {
		test.Fatalf("escalation %+v", escalated)
	}
	eventually(test, "the escalation pages", 30*time.Second, allDelivered(test, store, alert.ID, 3))
	if _, failure := store.Resolve(operation, alert.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the resolution reaches both", 30*time.Second, allDelivered(test, store, alert.ID, 5))
	pagerDuty := receiving.accepted("/pagerduty")
	if len(pagerDuty) != 2 || pagerDuty[0].action != "trigger" || pagerDuty[1].action != "resolve" || !strings.Contains(pagerDuty[0].body, `"severity":"critical"`) {
		test.Fatalf("PagerDuty got %+v", pagerDuty)
	}
	if chat := receiving.accepted("/slack"); len(chat) != 3 {
		test.Fatalf("the chat got %d messages", len(chat))
	}
}

func TestDeliverySurvivesARestartMidRetry(test *testing.T) {
	receiving := newSink(test)
	receiving.fail("/pagerduty", true)
	receiving.fail("/webhook", true)
	pool, _ := testsupport.Database(test)
	settings := config.Alerts{
		PublicURL: "https://twilight.example.org", RunbookURL: "https://docs.example.org/{kind}/", DeliveryHorizonSeconds: 86400,
		Receivers: []config.Receiver{
			{Name: "on-call", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: writeSecret(test, pagerDutyKey), URL: receiving.server.URL + "/pagerduty"}},
			{Name: "siem", Webhook: &config.WebhookReceiver{URL: receiving.server.URL + "/webhook", SecretFile: writeSecret(test, webhookSecret)}},
		},
		Routes: []config.Route{{Receiver: "on-call"}, {Receiver: "siem"}},
	}
	first, stopFirst := running(test, pool, settings, "twilight-0")
	operation := context.Background()
	alert, failure := first.Raise(operation, alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindLedgerChainBroken, Fingerprint: "ledger_chain_broken:nightfall-0/0/gap",
		Detail: map[string]any{"instance": "nightfall-0", "partition": 0}})
	if failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "both receivers are retrying", 30*time.Second, func() (bool, string) {
		current, failure := first.Get(operation, alert.ID)
		if failure != nil {
			test.Fatal(failure)
		}
		retrying := 0
		for _, delivery := range current.Deliveries {
			if delivery.State == "pending" && delivery.Attempts >= 1 && delivery.LastError != nil && strings.Contains(*delivery.LastError, "HTTP 503") {
				retrying++
			}
		}
		return retrying == 2, states(current)
	})
	stopFirst()
	if _, failure := pool.Exec(operation, `update alert_deliveries set leased_by = 'twilight-crashed', leased_until = now() + interval '3 seconds',
		next_attempt_at = now() where receiver = 'siem'`); failure != nil {
		test.Fatal(failure)
	}
	receiving.fail("/pagerduty", false)
	receiving.fail("/webhook", false)
	second, _ := running(test, pool, settings, "twilight-1")
	eventually(test, "the restarted instance delivers both", 30*time.Second, allDelivered(test, second, alert.ID, 2))
	if got := receiving.accepted("/pagerduty"); len(got) != 1 || got[0].action != "trigger" {
		test.Fatalf("PagerDuty accepted %+v", got)
	}
	tries := receiving.all("/webhook")
	accepted := receiving.accepted("/webhook")
	if len(accepted) != 1 || len(tries) < 2 {
		test.Fatalf("the webhook saw %d attempts, %d accepted", len(tries), len(accepted))
	}
	for _, entry := range tries {
		if entry.idempotencyKey != accepted[0].idempotencyKey || entry.body != accepted[0].body {
			test.Errorf("a retry changed the idempotency key or the body: %+v", entry)
		}
	}
	var leasedBy *string
	if failure := pool.QueryRow(operation, `select leased_by from alert_deliveries where receiver = 'siem'`).Scan(&leasedBy); failure != nil || leasedBy != nil {
		test.Fatalf("lease %v %v", leasedBy, failure)
	}
}

func TestNotificationsGiveUpPermanentlyOrAfterTheHorizon(test *testing.T) {
	receiving := newSink(test)
	receiving.fail("/webhook", true)
	pool, _ := testsupport.Database(test)
	settings := config.Alerts{
		PublicURL: "https://twilight.example.org", RunbookURL: "https://docs.example.org/{kind}/", DeliveryHorizonSeconds: 3,
		Receivers: []config.Receiver{
			{Name: "on-call", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: writeSecret(test, strings.Repeat("f", 32)), URL: receiving.server.URL + "/pagerduty"}},
			{Name: "siem", Webhook: &config.WebhookReceiver{URL: receiving.server.URL + "/webhook", SecretFile: writeSecret(test, webhookSecret)}},
		},
		Routes: []config.Route{{Receiver: "on-call"}, {Receiver: "siem"}},
	}
	store, _ := running(test, pool, settings, "twilight-0")
	alert, failure := store.Raise(context.Background(), alerts.Raised{Severity: alerts.High, Kind: alerts.KindPidReused, Fingerprint: "pid_reused:70000"})
	if failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "both notifications give up", 30*time.Second, func() (bool, string) {
		current, failure := store.Get(context.Background(), alert.ID)
		if failure != nil {
			test.Fatal(failure)
		}
		failed := map[string]string{}
		for _, delivery := range current.Deliveries {
			if delivery.State == "failed" && delivery.LastError != nil {
				failed[delivery.Receiver] = *delivery.LastError
			}
		}
		return strings.HasPrefix(failed["on-call"], "HTTP 400") && failed["siem"] != "", states(current)
	})
	if tries := receiving.all("/pagerduty"); len(tries) != 1 {
		test.Fatalf("a refused PagerDuty event was retried %d times", len(tries))
	}
}

func TestImplicitTLSMail(test *testing.T) {
	receiving := newSink(test)
	host, port, box := startMailpit(test, map[string]string{"MP_SMTP_REQUIRE_TLS": "true"})
	pool, _ := testsupport.Database(test)
	settings := notifications(test, receiving, host, port, config.EmailTLS)
	settings.Routes = []config.Route{{Receiver: "soc"}}
	settings.Receivers = settings.Receivers[4:]
	store, _ := running(test, pool, settings, "twilight-0")
	alert, failure := store.Raise(context.Background(), alerts.Raised{Severity: alerts.Low, Kind: alerts.KindQuarantineOverride, Fingerprint: "quarantine_override:x"})
	if failure != nil {
		test.Fatal(failure)
	}
	eventually(test, "the mail is delivered over TLS", 30*time.Second, allDelivered(test, store, alert.ID, 1))
	if messages := box.messages(test); len(messages) != 1 || messages[0].Subject != "[LOW] Quarantine override" {
		test.Fatalf("Mailpit holds %+v", messages)
	}
}
