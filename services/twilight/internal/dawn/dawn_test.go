package dawn

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
)

type authority struct {
	certificate *x509.Certificate
	key         *ecdsa.PrivateKey
	pem         []byte
}

func newAuthority(test *testing.T) authority {
	test.Helper()
	key, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	template := &x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: "internal test CA"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(time.Hour),
		IsCA:                  true,
		BasicConstraintsValid: true,
		KeyUsage:              x509.KeyUsageCertSign,
	}
	der, failure := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if failure != nil {
		test.Fatal(failure)
	}
	certificate, _ := x509.ParseCertificate(der)
	return authority{certificate: certificate, key: key, pem: pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})}
}

func (issuer authority) issue(test *testing.T, name string, usage x509.ExtKeyUsage, serial int64) ([]byte, []byte) {
	test.Helper()
	key, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	template := &x509.Certificate{
		SerialNumber: big.NewInt(serial),
		Subject:      pkix.Name{CommonName: name},
		DNSNames:     []string{name},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{usage},
	}
	der, failure := x509.CreateCertificate(rand.Reader, template, issuer.certificate, &key.PublicKey, issuer.key)
	if failure != nil {
		test.Fatal(failure)
	}
	keyDER, _ := x509.MarshalPKCS8PrivateKey(key)
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER})
}

type fakeDawn struct {
	server   *httptest.Server
	settings config.Dawn
	requests chan *http.Request
	bodies   chan []byte
	handler  func(writer http.ResponseWriter, request *http.Request)
}

func startFakeDawn(test *testing.T) *fakeDawn {
	test.Helper()
	issuer := newAuthority(test)
	serverCertificate, serverKey := issuer.issue(test, "dawn", x509.ExtKeyUsageServerAuth, 2)
	clientCertificate, clientKey := issuer.issue(test, "twilight-0", x509.ExtKeyUsageClientAuth, 3)
	pair, failure := tls.X509KeyPair(serverCertificate, serverKey)
	if failure != nil {
		test.Fatal(failure)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(issuer.pem)
	fake := &fakeDawn{requests: make(chan *http.Request, 16), bodies: make(chan []byte, 16)}
	fake.server = httptest.NewUnstartedServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		fake.requests <- request
		fake.bodies <- body
		fake.handler(writer, request)
	}))
	fake.server.TLS = &tls.Config{Certificates: []tls.Certificate{pair}, ClientAuth: tls.RequireAndVerifyClientCert, ClientCAs: pool, MinVersion: tls.VersionTLS13}
	fake.server.StartTLS()
	test.Cleanup(fake.server.Close)
	directory := test.TempDir()
	write := func(name string, content []byte) string {
		path := filepath.Join(directory, name)
		if failure := os.WriteFile(path, content, 0o600); failure != nil {
			test.Fatal(failure)
		}
		return path
	}
	fake.settings = config.Default().Dawn
	fake.settings.Endpoints = []string{strings.TrimPrefix(fake.server.URL, "https://")}
	fake.settings.CA = write("ca.crt", issuer.pem)
	fake.settings.Certificate = write("client.crt", clientCertificate)
	fake.settings.Key = write("client.key", clientKey)
	fake.settings.RequestTimeoutSeconds = 2
	return fake
}

func logger() *slog.Logger {
	return slog.New(slog.NewJSONHandler(io.Discard, nil))
}

var node = NodeRef{DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", NamespaceID: "5d2e9a1c7b3f8e04"}

const workPid campaign.Pid = 12808937078074471924

func TestDispatchOverMutualTLS(test *testing.T) {
	fake := startFakeDawn(test)
	fake.handler = func(writer http.ResponseWriter, request *http.Request) {
		if len(request.TLS.PeerCertificates) == 0 || request.TLS.PeerCertificates[0].Subject.CommonName != "twilight-0" {
			writer.WriteHeader(http.StatusForbidden)
			return
		}
		writer.WriteHeader(http.StatusAccepted)
		_, _ = writer.Write([]byte(`{"accepted":["12808937078074471924"]}`))
	}
	client, failure := New(fake.settings, nil, logger())
	if failure != nil {
		test.Fatal(failure)
	}
	script, campaignID, attempt := "upgrade", "0192f3a4-5b6c-7d8e-9f01-23456789abcd", 1
	accepted, failure := client.Dispatch(context.Background(), node, []Work{{Pid: workPid, CampaignID: &campaignID, Attempt: &attempt, Kind: "run_script", Script: &script, TimeoutSeconds: 900}})
	if failure != nil {
		test.Fatal(failure)
	}
	if len(accepted) != 1 || accepted[0] != workPid {
		test.Fatalf("accepted %v", accepted)
	}
	request := <-fake.requests
	body := <-fake.bodies
	if request.URL.Path != "/v1/dispatch" || request.Header.Get("Content-Type") != "application/json" {
		test.Fatalf("request %s %s", request.URL.Path, request.Header.Get("Content-Type"))
	}
	var sent struct {
		Node map[string]any   `json:"node"`
		Work []map[string]any `json:"work"`
	}
	if failure := json.Unmarshal(body, &sent); failure != nil {
		test.Fatal(failure)
	}
	if len(sent.Node) != 4 {
		test.Errorf("node carries %d fields, want device_id, installation_id, namespace_id and nightfall: %s", len(sent.Node), body)
	}
	for _, field := range []string{"device_id", "installation_id", "namespace_id", "nightfall"} {
		if _, present := sent.Node[field]; !present {
			test.Errorf("node lacks %s: %s", field, body)
		}
	}
	fields := []string{"pid", "campaign_id", "attempt", "kind", "script", "timeout_seconds", "version_key", "desired_version", "config_hash", "collect_facts", "collect_files", "stream_logs"}
	if len(sent.Work) != 1 || len(sent.Work[0]) != len(fields) {
		test.Fatalf("work %s", body)
	}
	for _, field := range fields {
		if _, present := sent.Work[0][field]; !present {
			test.Errorf("work lacks %s: %s", field, body)
		}
	}
	if sent.Work[0]["pid"] != "12808937078074471924" || sent.Work[0]["attempt"] != float64(1) {
		test.Errorf("the pid goes as a decimal string and the attempt as a number: %s", body)
	}
	if files, isList := sent.Work[0]["collect_files"].([]any); !isList || len(files) != 0 {
		test.Errorf("collect_files must be an empty list: %s", body)
	}
}

func TestReapSendsThePids(test *testing.T) {
	fake := startFakeDawn(test)
	fake.handler = func(writer http.ResponseWriter, request *http.Request) {
		writer.WriteHeader(http.StatusAccepted)
	}
	client, failure := New(fake.settings, nil, logger())
	if failure != nil {
		test.Fatal(failure)
	}
	if failure := client.Reap(context.Background(), node, []campaign.Pid{workPid, 65536}); failure != nil {
		test.Fatal(failure)
	}
	request := <-fake.requests
	body := <-fake.bodies
	if request.URL.Path != "/v1/reap" {
		test.Fatalf("reap went to %s", request.URL.Path)
	}
	var sent struct {
		Node map[string]any `json:"node"`
		Pids []string       `json:"pids"`
	}
	if failure := json.Unmarshal(body, &sent); failure != nil {
		test.Fatal(failure)
	}
	if len(sent.Pids) != 2 || sent.Pids[0] != "12808937078074471924" || sent.Pids[1] != "65536" || sent.Node["namespace_id"] != "5d2e9a1c7b3f8e04" {
		test.Fatalf("reap body %s", body)
	}
	fake.handler = func(writer http.ResponseWriter, request *http.Request) {
		writer.WriteHeader(http.StatusTooManyRequests)
	}
	if failure := client.Reap(context.Background(), node, nil); OutcomeOf(failure) != Busy {
		test.Fatalf("a 429 to a reap: %v", failure)
	}
	<-fake.requests
	if body := <-fake.bodies; !strings.Contains(string(body), `"pids":[]`) {
		test.Fatalf("no pids go as an empty list: %s", body)
	}
}

func TestOutcomesAreClassified(test *testing.T) {
	fake := startFakeDawn(test)
	client, failure := New(fake.settings, nil, logger())
	if failure != nil {
		test.Fatal(failure)
	}
	cases := []struct {
		status  int
		outcome Outcome
	}{
		{http.StatusTooManyRequests, Busy},
		{http.StatusServiceUnavailable, NotDelivered},
		{http.StatusInternalServerError, Ambiguous},
		{http.StatusBadRequest, Rejected},
		{http.StatusForbidden, Rejected},
	}
	for _, testCase := range cases {
		fake.handler = func(writer http.ResponseWriter, request *http.Request) {
			writer.WriteHeader(testCase.status)
			_, _ = writer.Write([]byte(`{"detail":"no"}`))
		}
		_, failure := client.Dispatch(context.Background(), node, []Work{{Pid: workPid}})
		if OutcomeOf(failure) != testCase.outcome {
			test.Errorf("status %d: outcome %v (%v), want %v", testCase.status, OutcomeOf(failure), failure, testCase.outcome)
		}
		<-fake.requests
		<-fake.bodies
	}
	fake.handler = func(writer http.ResponseWriter, request *http.Request) {
		time.Sleep(3 * time.Second)
	}
	if _, failure := client.Dispatch(context.Background(), node, []Work{{Pid: workPid}}); OutcomeOf(failure) != Ambiguous {
		test.Errorf("a timeout after sending must be ambiguous: %v", failure)
	}
}

func TestUnreachableEndpointIsNotDelivered(test *testing.T) {
	listener, _ := net.Listen("tcp", "127.0.0.1:0")
	address := listener.Addr().String()
	listener.Close()
	settings := config.Default().Dawn
	settings.AllowPlaintext = true
	settings.Endpoints = []string{address}
	settings.RequestTimeoutSeconds = 2
	client, failure := New(settings, nil, logger())
	if failure != nil {
		test.Fatal(failure)
	}
	_, failure = client.Dispatch(context.Background(), node, []Work{{Pid: workPid}})
	var dawnError *Error
	if !errors.As(failure, &dawnError) || dawnError.Outcome != NotDelivered {
		test.Fatalf("refused connection: %v", failure)
	}
}

func TestFactsLogsAndFiles(test *testing.T) {
	fake := startFakeDawn(test)
	fake.handler = func(writer http.ResponseWriter, request *http.Request) {
		switch request.URL.Path {
		case "/v1/facts":
			_, _ = writer.Write([]byte(`{"facts":{"dusk.version":"0.2.0","dusk.os.name":"lin\u0000ux"},"reported":{"version_key":"dusk.version","version":"0.2.0","config_hash":null,"services":["nightfall"],"facts":null}}`))
		case "/v1/logs":
			writer.WriteHeader(http.StatusAccepted)
			_, _ = writer.Write([]byte(`{"stream_id":"s-1"}`))
		case "/v1/files":
			writer.WriteHeader(http.StatusAccepted)
			_, _ = writer.Write([]byte(`{"upload_id":"u-1"}`))
		}
	}
	client, _ := New(fake.settings, nil, logger())
	facts, failure := client.Facts(context.Background(), node, workPid, []string{"app.version"})
	if failure != nil || string(facts.Facts["dusk.version"]) != `"0.2.0"` || string(facts.Facts["dusk.os.name"]) != "\"lin\uFFFDux\"" || facts.Reported == nil || *facts.Reported.Version != "0.2.0" {
		test.Fatalf("facts %+v %v", facts, failure)
	}
	<-fake.requests
	if body := <-fake.bodies; !strings.Contains(string(body), `"pid":"12808937078074471924"`) || !strings.Contains(string(body), `"version_keys":["app.version"]`) {
		test.Fatalf("facts body %s", body)
	}
	if _, failure := client.Facts(context.Background(), node, workPid, nil); failure != nil {
		test.Fatal(failure)
	}
	<-fake.requests
	if body := <-fake.bodies; !strings.Contains(string(body), `"version_keys":[]`) {
		test.Fatalf("facts body without version keys %s", body)
	}
	stream, failure := client.Logs(context.Background(), node, workPid, "info", 60, nil)
	if failure != nil || stream != "s-1" {
		test.Fatalf("logs %q %v", stream, failure)
	}
	<-fake.requests
	if body := <-fake.bodies; !strings.Contains(string(body), `"pid":"12808937078074471924"`) {
		test.Fatalf("logs body %s", body)
	}
	upload, failure := client.Files(context.Background(), node, workPid, "/var/log/syslog", nil)
	if failure != nil || upload != "u-1" {
		test.Fatalf("files %q %v", upload, failure)
	}
}

type staticResolver map[string][]string

func (resolver staticResolver) LookupHost(operation context.Context, host string) ([]string, error) {
	addresses, found := resolver[host]
	if !found {
		return nil, fmt.Errorf("no such host %s", host)
	}
	return addresses, nil
}

func TestRendezvousRoutingIsStable(test *testing.T) {
	settings := config.Default().Dawn
	settings.AllowPlaintext = true
	resolver := staticResolver{"dawn": {"10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"}}
	client, failure := New(settings, resolver, logger())
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := client.route("any"); OutcomeOf(failure) != NotDelivered {
		test.Fatal("routed with no endpoint")
	}
	if failure := client.Resolve(context.Background()); failure != nil {
		test.Fatal(failure)
	}
	keys := make([]string, 2000)
	before := map[string]string{}
	counts := map[string]int{}
	for index := range keys {
		keys[index] = fmt.Sprintf("%032x/%032x", index, index*7)
		endpoint, _ := client.route(keys[index])
		before[keys[index]] = endpoint
		counts[endpoint]++
	}
	for endpoint, count := range counts {
		if count < 350 || count > 650 {
			test.Errorf("%s holds %d of 2000 nodes", endpoint, count)
		}
	}
	resolver["dawn"] = []string{"10.0.0.1", "10.0.0.2", "10.0.0.4"}
	if failure := client.Resolve(context.Background()); failure != nil {
		test.Fatal(failure)
	}
	for _, key := range keys {
		after, _ := client.route(key)
		if before[key] != "10.0.0.3:8443" && after != before[key] {
			test.Fatalf("%s moved from %s to %s although its endpoint stayed", key, before[key], after)
		}
		if after == "10.0.0.3:8443" {
			test.Fatalf("%s still routes to a removed endpoint", key)
		}
	}
	first, _ := client.route(keys[0])
	client.markUnhealthy(first)
	second, _ := client.route(keys[0])
	if second == first {
		test.Fatal("an unhealthy endpoint was chosen while a healthy one exists")
	}
	client.now = func() time.Time { return time.Now().Add(time.Minute) }
	if again, _ := client.route(keys[0]); again != first {
		test.Fatal("an endpoint stayed unhealthy past its window")
	}
}
