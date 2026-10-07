//go:build integration

package main

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
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

func get(address, path string) (int, string) {
	client := &http.Client{Timeout: 5 * time.Second}
	response, failure := client.Get("http://" + address + path)
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
	healthAddress := freeAddress(test)
	configPath := filepath.Join(test.TempDir(), "twilight.yaml")
	if failure := os.WriteFile(configPath, []byte("instance: twilight-command-test\n"), 0o600); failure != nil {
		test.Fatal(failure)
	}
	topics := map[string]string{
		"CONNECTIONS": "delete", "CENSUS": "compact", "LEDGER": "delete", "ENROLLMENTS": "delete", "NODE_STATE": "compact", "PROCESS_RESULTS": "delete",
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
	if code, _, diagnostics := command("token", "revoke", identifier); code != 0 || !strings.Contains(diagnostics, "is revoked since") {
		test.Fatalf("token revoke: exit %d, %q", code, diagnostics)
	}
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
		status, _ := get(healthAddress, "/readyz")
		ready = status == http.StatusOK
		if !ready {
			time.Sleep(250 * time.Millisecond)
		}
	}
	if !ready {
		status, body := get(healthAddress, "/readyz")
		test.Fatalf("serve never became ready: %d %q", status, body)
	}
	if status, _ := get(healthAddress, "/healthz"); status != http.StatusOK {
		test.Fatalf("healthz %d", status)
	}
	status, metrics := get(healthAddress, "/metrics")
	for _, metric := range []string{"twilight_online_nodes", "twilight_view_degraded", "twilight_reconcile_lag_seconds", "twilight_leader "} {
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
	if !strings.Contains(logs.String(), `"msg":"twilight started"`) || !strings.Contains(logs.String(), `"msg":"the engine and reconcile stopped"`) {
		test.Errorf("serve's start and stop were not logged:\n%s", logs.String())
	}
}
