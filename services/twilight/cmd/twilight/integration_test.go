//go:build integration

package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func freeAddress(test *testing.T) string {
	listener, failure := net.Listen("tcp", "127.0.0.1:0")
	if failure != nil {
		test.Fatal(failure)
	}
	address := listener.Addr().String()
	listener.Close()
	return address
}

func writeLedgerKeys(test *testing.T) string {
	public, _, _ := ed25519.GenerateKey(rand.Reader)
	ledgerPath := filepath.Join(test.TempDir(), "ledger-keys.json")
	document := `{"keys":[{"kty":"OKP","crv":"Ed25519","x":"` + base64.RawURLEncoding.EncodeToString(public) + `"}]}`
	if failure := os.WriteFile(ledgerPath, []byte(document), 0o600); failure != nil {
		test.Fatal(failure)
	}
	return ledgerPath
}

func get(address, path, token string) (int, string) {
	client := &http.Client{Timeout: 5 * time.Second}
	request, _ := http.NewRequest(http.MethodGet, "http://"+address+path, nil)
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	response, failure := client.Do(request)
	if failure != nil {
		return 0, failure.Error()
	}
	defer response.Body.Close()
	body, _ := io.ReadAll(response.Body)
	return response.StatusCode, string(body)
}

func TestMigrateTokenAndServe(test *testing.T) {
	_, url := testsupport.EmptyDatabase(test)
	brokers := testsupport.Brokers(test)
	suffix := strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	ledgerPath := writeLedgerKeys(test)
	healthAddress, apiAddress := freeAddress(test), freeAddress(test)
	configPath := filepath.Join(test.TempDir(), "twilight.yaml")
	if failure := os.WriteFile(configPath, []byte("instance: twilight-command-test\n"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	topics := map[string]string{
		"CONNECTIONS": "delete", "CENSUS": "compact", "LEDGER": "delete", "ENROLLMENTS": "delete", "NODE_STATE": "compact", "PROCESS_RESULTS": "delete",
		"INTENDED_PROCESSES": "compact",
	}
	environment := []string{
		"TWILIGHT__DATABASE__URL=" + url,
		"TWILIGHT__KAFKA__BROKERS=" + brokers,
		"TWILIGHT__KAFKA__ALLOW_PLAINTEXT=true",
		"TWILIGHT__KAFKA__RESULTS_GROUP=results-" + suffix,
		"TWILIGHT__KAFKA__INVENTORY_GROUP=inventory-" + suffix,
		"TWILIGHT__KAFKA__RECONCILE_GROUP=reconcile-" + suffix,
		"TWILIGHT__KAFKA__RECONCILE_RESULTS_GROUP=reconcile-results-" + suffix,
		"TWILIGHT__DAWN__ENDPOINTS=127.0.0.1:9",
		"TWILIGHT__DAWN__ALLOW_PLAINTEXT=true",
		"TWILIGHT__RECONCILE__LEDGER_KEYS=" + ledgerPath,
		"TWILIGHT__HEALTH_LISTEN=" + healthAddress,
		"TWILIGHT__LISTEN=" + apiAddress,
		"TWILIGHT__DRAIN_SECONDS=30",
	}
	client, failure := kgo.NewClient(kgo.SeedBrokers(brokers))
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	admin := kadm.NewClient(client)
	for name, policy := range topics {
		topic := "dusk." + strings.ToLower(name) + "." + suffix
		environment = append(environment, "TWILIGHT__KAFKA__TOPICS__"+name+"="+topic)
		if _, failure := admin.CreateTopic(context.Background(), 1, 1, map[string]*string{"cleanup.policy": &policy}, topic); failure != nil {
			test.Fatal(failure)
		}
	}
	command := func(arguments ...string) (int, string, string) {
		var output, diagnostics bytes.Buffer
		code := run(context.Background(), append(arguments, "--config", configPath), environment, &output, &diagnostics)
		return code, output.String(), diagnostics.String()
	}

	if code, _, diagnostics := command("serve"); code != 1 || !strings.Contains(diagnostics, "run twilight migrate") {
		test.Fatalf("serve before migrate: exit %d, %q", code, diagnostics)
	}
	if code, _, diagnostics := command("migrate"); code != 0 || !strings.Contains(diagnostics, "the database schema is current") {
		test.Fatalf("migrate: exit %d, %q", code, diagnostics)
	}
	code, secret, diagnostics := command("token", "create", "--name", "ci", "--role", "operator")
	if code != 0 || !strings.HasPrefix(secret, "twilight_") || !strings.Contains(diagnostics, "only time it is shown") {
		test.Fatalf("token create: exit %d, %q, %q", code, secret, diagnostics)
	}
	identifier := strings.Fields(diagnostics)[1]
	secret = strings.TrimSpace(secret)
	if code, _, _ := command("token", "revoke", uuid.NewString()); code != 1 {
		test.Fatalf("revoking an unknown token exits %d", code)
	}

	operation, cancel := context.WithCancel(context.Background())
	defer cancel()
	finished := make(chan int, 1)
	var logs bytes.Buffer
	go func() {
		finished <- run(operation, []string{"serve", "--config", configPath}, environment, &logs, io.Discard)
	}()
	deadline := time.Now().Add(90 * time.Second)
	ready := false
	for time.Now().Before(deadline) && !ready {
		status, _ := get(healthAddress, "/readyz", "")
		ready = status == http.StatusOK
		if !ready {
			time.Sleep(250 * time.Millisecond)
		}
	}
	if !ready {
		status, body := get(healthAddress, "/readyz", "")
		test.Fatalf("serve never became ready: %d %q", status, body)
	}
	if status, _ := get(healthAddress, "/healthz", ""); status != http.StatusOK {
		test.Fatalf("healthz %d", status)
	}
	if status, body := get(apiAddress, "/api/v1/me", secret); status != http.StatusOK || !strings.Contains(body, `"subject":"token:`+identifier+`"`) || !strings.Contains(body, `"role":"operator"`) {
		test.Fatalf("the created token on the API: %d %q", status, body)
	}
	if status, body := get(apiAddress, "/api/v1/me", ""); status != http.StatusUnauthorized || !strings.Contains(body, `"code":"unauthenticated"`) {
		test.Fatalf("the API without a token: %d %q", status, body)
	}
	if status, body := get(apiAddress, "/api/openapi.json", ""); status != http.StatusOK || !strings.Contains(body, `"openapi": "3.1`) {
		test.Fatalf("the OpenAPI document: %d %.200q", status, body)
	}
	if status, body := get(apiAddress, "/campaigns", ""); status != http.StatusServiceUnavailable || strings.TrimSpace(body) != "UI not built" {
		test.Fatalf("a UI page of a binary built without the UI: %d %q", status, body)
	}
	if code, _, diagnostics := command("token", "revoke", identifier); code != 0 || !strings.Contains(diagnostics, "is revoked since") {
		test.Fatalf("token revoke: exit %d, %q", code, diagnostics)
	}
	if status, body := get(apiAddress, "/api/v1/me", secret); status != http.StatusUnauthorized || !strings.Contains(body, "unknown or revoked") {
		test.Fatalf("the revoked token on the API: %d %q", status, body)
	}
	status, metrics := get(healthAddress, "/metrics", "")
	for _, metric := range []string{"twilight_online_nodes", "twilight_view_degraded", "twilight_reconcile_lag_seconds", "twilight_leader ", `twilight_api_requests_total{code="200",route="GET /api/v1/me"} 1`} {
		if status != http.StatusOK || !strings.Contains(metrics, metric) {
			test.Errorf("metrics lack %s", metric)
		}
	}
	cancel()
	select {
	case code := <-finished:
		if code != 0 {
			test.Fatalf("serve exited %d", code)
		}
	case <-time.After(45 * time.Second):
		test.Fatal("serve did not stop within 45 s of its context ending")
	}
	if !strings.Contains(logs.String(), `"msg":"twilight started"`) || !strings.Contains(logs.String(), `"msg":"the engine and reconcile stopped"`) ||
		!strings.Contains(logs.String(), `"msg":"the API server stopped"`) {
		test.Errorf("serve's start and stop were not logged:\n%s", logs.String())
	}

	code, secret, _ = command("token", "create", "--name", "drain", "--role", "viewer")
	if code != 0 {
		test.Fatalf("token create for the drain: exit %d", code)
	}
	draining := append(append([]string(nil), environment...), "TWILIGHT__DRAIN_SECONDS=2")
	operation, cancel = context.WithCancel(context.Background())
	defer cancel()
	var drainLogs bytes.Buffer
	go func() {
		finished <- run(operation, []string{"serve", "--config", configPath}, draining, &drainLogs, io.Discard)
	}()
	deadline = time.Now().Add(90 * time.Second)
	for status, _ := get(healthAddress, "/readyz", ""); status != http.StatusOK; status, _ = get(healthAddress, "/readyz", "") {
		if time.Now().After(deadline) {
			test.Fatal("serve never became ready for the drain")
		}
		time.Sleep(250 * time.Millisecond)
	}
	held, failure := net.Dial("tcp", apiAddress)
	if failure != nil {
		test.Fatal(failure)
	}
	defer held.Close()
	if _, failure := fmt.Fprintf(held, "POST /api/v1/selectors/validate HTTP/1.1\r\nHost: twilight\r\nAuthorization: Bearer %s\r\nContent-Type: application/json\r\nContent-Length: 64\r\n\r\n{\"selector\":", strings.TrimSpace(secret)); failure != nil {
		test.Fatal(failure)
	}
	time.Sleep(500 * time.Millisecond)
	cancel()
	select {
	case code := <-finished:
		if code != 1 || !strings.Contains(drainLogs.String(), `"msg":"API requests were still running at the end of drain_seconds; closing them"`) {
			test.Fatalf("serve with a request still running at the end of its drain exited %d:\n%s", code, drainLogs.String())
		}
	case <-time.After(45 * time.Second):
		test.Fatal("serve did not stop within 45 s of its context ending")
	}
}
