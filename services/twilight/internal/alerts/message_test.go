package alerts

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/santhosh-tekuri/jsonschema/v6"

	"dusk/services/twilight/internal/config"
)

var sampleTime = time.Date(2026, 10, 10, 3, 4, 5, 123456789, time.UTC)

func sampleNotification(transition string) notification {
	hostname := "camera-17"
	tenant := "acme"
	actor := "operator@example.org"
	message := notification{
		alert: Alert{
			ID: 42, Time: sampleTime, LastSeenAt: sampleTime, Occurrences: 3, Severity: Critical, Kind: KindProcessWithoutIntent,
			Fingerprint: "process_without_intent:12808937078074471924", Tenant: &tenant,
			Detail: json.RawMessage(`{"message": "a client created a process at a pid twilight never intended", "pid": "12808937078074471924",
				"device_id": "0123456789abcdef0123456789abcdef", "installation_id": "fedcba9876543210fedcba9876543210",
				"principal": "dawn-0", "action": "Dusk.process", "instance": "nightfall-1", "partition": 3, "sequence": 9001}`),
		},
		transition: transition, severity: Critical, at: sampleTime.Add(time.Minute), key: "0192f0a4-5b6c-7d8e-9f01-23456789abcd",
		receiver: "on-call", hostname: &hostname,
		settings: config.Alerts{PublicURL: "https://twilight.example.org/", RunbookURL: "https://docs.example.org/stack/runbooks/{kind}/"},
	}
	if transition == TransitionAcknowledged || transition == TransitionResolved {
		message.actor = &actor
	}
	if failure := message.parseDetail(); failure != nil {
		panic(failure)
	}
	return message
}

func withDetail(test *testing.T, message *notification, detail []byte) {
	test.Helper()
	message.alert.Detail = detail
	if failure := message.parseDetail(); failure != nil {
		test.Fatal(failure)
	}
}

var transitions = []string{TransitionOpened, TransitionReEscalated, TransitionAcknowledged, TransitionResolved}

func compileSchema(test *testing.T, name string) *jsonschema.Schema {
	test.Helper()
	content, failure := os.ReadFile(filepath.Join("testdata", name))
	if failure != nil {
		test.Fatal(failure)
	}
	document, failure := jsonschema.UnmarshalJSON(bytes.NewReader(content))
	if failure != nil {
		test.Fatal(failure)
	}
	compiler := jsonschema.NewCompiler()
	compiler.AssertFormat()
	if failure := compiler.AddResource("file:///"+name, document); failure != nil {
		test.Fatal(failure)
	}
	schema, failure := compiler.Compile("file:///" + name)
	if failure != nil {
		test.Fatal(failure)
	}
	return schema
}

func conforms(test *testing.T, schema *jsonschema.Schema, payload []byte) {
	test.Helper()
	instance, failure := jsonschema.UnmarshalJSON(bytes.NewReader(payload))
	if failure != nil {
		test.Fatalf("not JSON: %v\n%s", failure, payload)
	}
	if failure := schema.Validate(instance); failure != nil {
		test.Fatalf("%v\n%s", failure, payload)
	}
}

func TestMessageContent(test *testing.T) {
	message := sampleNotification(TransitionOpened)
	if got := message.title(); got != "[CRITICAL] Process nobody intended on camera-17" {
		test.Errorf("title %q", got)
	}
	if got := message.summary(); got != "A client created a process at a pid twilight never intended" {
		test.Errorf("summary %q", got)
	}
	if got := message.alertURL(); got != "https://twilight.example.org/alerts?alert=42" {
		test.Errorf("alert link %q", got)
	}
	if got := message.runbookURL(); got != "https://docs.example.org/stack/runbooks/process_without_intent/" {
		test.Errorf("runbook link %q", got)
	}
	facts := map[string]string{}
	for _, entry := range message.facts() {
		facts[entry.label] = entry.value
	}
	if facts["Node"] != "camera-17 (0123456789abcdef0123456789abcdef/fedcba9876543210fedcba9876543210)" || facts["Tenant"] != "acme" ||
		facts["Principal"] != "dawn-0" || facts["Occurrences"] != "3" || facts["Raised"] != "2026-10-10T03:04:05Z" {
		test.Errorf("facts %v", facts)
	}
	var evidence []string
	for _, entry := range message.evidence() {
		evidence = append(evidence, entry.label+"="+entry.value)
	}
	if strings.Join(evidence, " ") != "action=Dusk.process instance=nightfall-1 partition=3 pid=12808937078074471924 sequence=9001" {
		test.Errorf("evidence %v", evidence)
	}
	for transition, want := range map[string]string{
		TransitionReEscalated:  "[CRITICAL] Escalated: Process nobody intended on camera-17",
		TransitionAcknowledged: "[ACKNOWLEDGED] Process nobody intended on camera-17",
		TransitionResolved:     "[RESOLVED] Process nobody intended on camera-17",
	} {
		if got := sampleNotification(transition).title(); got != want {
			test.Errorf("%s title %q", transition, got)
		}
	}
	if got := sampleNotification(TransitionResolved).change(); got != "Resolved by operator@example.org at 2026-10-10T03:05:05Z." {
		test.Errorf("resolved change %q", got)
	}
}

func TestMessageSubjectPrefersTheCampaign(test *testing.T) {
	message := sampleNotification(TransitionOpened)
	message.alert.Kind = KindCampaignPausedByGate
	withDetail(test, &message, []byte(`{"campaign_id": "0192f0a4-5b6c-7d8e-9f01-23456789abcd", "campaign_name": "Patch 4.2", "reason": "failure rate 0.62"}`))
	message.hostname = nil
	if got := message.title(); got != "[CRITICAL] Campaign paused by its gate in campaign Patch 4.2" {
		test.Errorf("title %q", got)
	}
	withDetail(test, &message, []byte(`{}`))
	message.alert.Kind = "a_kind_from_elsewhere"
	if got, summary := message.title(), message.summary(); got != "[CRITICAL] a kind from elsewhere" || summary != "A kind from elsewhere." {
		test.Errorf("unknown kind %q %q", got, summary)
	}
}
