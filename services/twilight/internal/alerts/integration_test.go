//go:build integration

package alerts_test

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	if failure := writeMailCertificate(); failure != nil {
		fmt.Fprintln(os.Stderr, "write the mail server certificate:", failure)
		os.Exit(1)
	}
	if failure := os.Setenv("SSL_CERT_FILE", mailCertificate); failure != nil {
		fmt.Fprintln(os.Stderr, failure)
		os.Exit(1)
	}
	code := testsupport.Run(suite)
	if failure := os.RemoveAll(filepath.Dir(mailCertificate)); failure != nil {
		fmt.Fprintln(os.Stderr, "remove the mail server certificate:", failure)
	}
	os.Exit(code)
}

func TestAlertsDeduplicateAndRecordTheirTransitions(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := testsupport.AlertStore(test, pool)
	operation := context.Background()
	raised := alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindProcessWithoutIntent, Fingerprint: "process_without_intent:12808937078074471924", Detail: map[string]any{"pid": "12808937078074471924"}}
	first, failure := store.Raise(operation, raised)
	if failure != nil {
		test.Fatal(failure)
	}
	second, failure := store.Raise(operation, raised)
	if failure != nil || second.ID != first.ID || second.Occurrences != 2 || second.Severity != alerts.Critical {
		test.Fatalf("repeat %+v %v", second, failure)
	}
	lower := raised
	lower.Severity = alerts.High
	if third, failure := store.Raise(operation, lower); failure != nil || third.Severity != alerts.Critical || third.Occurrences != 3 {
		test.Fatalf("a lower repeat changed the alert: %+v %v", third, failure)
	}
	if _, failure := store.Raise(operation, alerts.Raised{Severity: alerts.Medium, Kind: alerts.KindQuarantineOverride, Fingerprint: "quarantine_override:x"}); failure != nil {
		test.Fatal(failure)
	}
	acknowledged, failure := store.Acknowledge(operation, first.ID, "operator@example.org", time.Now())
	if failure != nil || acknowledged.AcknowledgedBy == nil {
		test.Fatalf("acknowledge %+v %v", acknowledged, failure)
	}
	if again, failure := store.Acknowledge(operation, first.ID, "someone@example.org", time.Now()); failure != nil || *again.AcknowledgedBy != "operator@example.org" {
		test.Fatalf("a second acknowledgement replaced the first: %+v %v", again, failure)
	}
	if _, failure := store.Resolve(operation, first.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Resolve(operation, first.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	rows, failure := pool.Query(operation, `select transition, actor from alert_transitions where alert_id = $1 order by id`, first.ID)
	if failure != nil {
		test.Fatal(failure)
	}
	var recorded []string
	for rows.Next() {
		var transition string
		var actor *string
		if failure := rows.Scan(&transition, &actor); failure != nil {
			test.Fatal(failure)
		}
		if actor != nil {
			transition += " by " + *actor
		}
		recorded = append(recorded, transition)
	}
	if strings.Join(recorded, ", ") != "opened, acknowledged by operator@example.org, resolved by operator@example.org" {
		test.Fatalf("transitions %v", recorded)
	}
	reopened, failure := store.Raise(operation, raised)
	if failure != nil || reopened.ID == first.ID || reopened.Occurrences != 1 {
		test.Fatalf("a resolved alert must open a new one: %+v %v", reopened, failure)
	}
	if _, failure := pool.Exec(operation, `insert into campaigns (id, name, tenant, status, kind, selector, action, policy, salt, created_by)
		values ('0192f0a4-5b6c-7d8e-9f01-23456789abcd', 'Patch', 'acme', 'running', 'run_script', 'true', '{}', '{}', decode(repeat('00', 32), 'hex'), 'test')`); failure != nil {
		test.Fatal(failure)
	}
	gated, failure := store.Raise(operation, alerts.Raised{Severity: alerts.High, Kind: alerts.KindCampaignPausedByGate, Fingerprint: "campaign_paused_by_gate:0192f0a4-5b6c-7d8e-9f01-23456789abcd",
		Detail: map[string]any{"campaign_id": "0192f0a4-5b6c-7d8e-9f01-23456789abcd"}})
	if failure != nil || gated.Tenant == nil || *gated.Tenant != "acme" {
		test.Fatalf("the alert did not take its campaign's tenant: %+v %v", gated, failure)
	}
	if failure := alerts.ResolveFingerprintIn(operation, pool, gated.Fingerprint, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	if resolved, failure := store.Get(operation, gated.ID); failure != nil || resolved.ResolvedAt == nil || *resolved.ResolvedBy != "operator@example.org" {
		test.Fatalf("resolve by fingerprint %+v %v", resolved, failure)
	}
	if _, failure := pool.Exec(operation, `delete from alerts where id = $1`, gated.ID); failure != nil {
		test.Fatal(failure)
	}
	counts, failure := store.Counts(operation)
	if failure != nil || counts.Open[alerts.Critical] != 1 || counts.Open[alerts.Medium] != 1 || counts.Unacknowledged[alerts.Critical] != 1 {
		test.Fatalf("open counts %+v %v", counts, failure)
	}
	if _, failure := store.Acknowledge(operation, reopened.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	counts, failure = store.Counts(operation)
	if failure != nil || counts.Open[alerts.Critical] != 1 || counts.Unacknowledged[alerts.Critical] != 0 || counts.Unacknowledged[alerts.Medium] != 1 {
		test.Fatalf("an acknowledged critical alert still counts as unacknowledged: %+v %v", counts, failure)
	}
	open, failure := store.List(operation, true, "", 10)
	if failure != nil || len(open.Alerts) != 2 || open.Next != "" {
		test.Fatalf("open %+v %v", open, failure)
	}
	firstPage, failure := store.List(operation, false, "", 2)
	if failure != nil || len(firstPage.Alerts) != 2 || firstPage.Next == "" {
		test.Fatalf("first page %+v %v", firstPage, failure)
	}
	rest, failure := store.List(operation, false, firstPage.Next, 2)
	if failure != nil || len(rest.Alerts) != 1 || rest.Next != "" || rest.Alerts[0].ID == firstPage.Alerts[1].ID {
		test.Fatalf("second page %+v %v", rest, failure)
	}
	if rest.Alerts[0].Time.After(firstPage.Alerts[1].Time) {
		test.Fatalf("pages are not newest first: %+v then %+v", firstPage.Alerts, rest.Alerts)
	}
	if _, failure := store.List(operation, false, "not a cursor", 2); !errors.Is(failure, alerts.ErrMalformedCursor) {
		test.Fatalf("a malformed cursor: %v", failure)
	}
	if _, failure := store.Acknowledge(operation, 999999, "x", time.Now()); failure != alerts.ErrNotFound {
		test.Fatalf("unknown alert: %v", failure)
	}
}

func gaugeSeries(test *testing.T) map[string]float64 {
	test.Helper()
	families, failure := prometheus.DefaultGatherer.Gather()
	if failure != nil {
		test.Fatal(failure)
	}
	series := map[string]float64{}
	for _, family := range families {
		if family.GetName() != "twilight_alerts_open" {
			continue
		}
		for _, metric := range family.GetMetric() {
			labels := map[string]string{}
			for _, pair := range metric.GetLabel() {
				labels[pair.GetName()] = pair.GetValue()
			}
			series[labels["severity"]+"/"+labels["kind"]] = metric.GetGauge().GetValue()
		}
	}
	return series
}

func TestTheOpenAlertsGaugeDropsResolvedKindsWithoutGaps(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := testsupport.AlertStore(test, pool)
	operation, cancel := context.WithCancel(context.Background())
	defer cancel()
	critical, failure := store.Raise(operation, alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindPidReused, Fingerprint: "pid_reused:65536"})
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.Raise(operation, alerts.Raised{Severity: alerts.High, Kind: alerts.KindProcessShape, Fingerprint: "process_shape:65537"}); failure != nil {
		test.Fatal(failure)
	}
	go store.RunMetrics(operation, 50*time.Millisecond)
	deadline := time.Now().Add(10 * time.Second)
	for gaugeSeries(test)["critical/pid_reused"] != 1 || gaugeSeries(test)["high/process_shape"] != 1 {
		if time.Now().After(deadline) {
			test.Fatalf("the gauge never showed both alerts: %v", gaugeSeries(test))
		}
		time.Sleep(20 * time.Millisecond)
	}
	if _, failure := store.Resolve(operation, critical.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	for {
		series := gaugeSeries(test)
		if series["high/process_shape"] != 1 {
			test.Fatalf("a kind that stayed open left the gauge: %v", series)
		}
		if _, present := series["critical/pid_reused"]; !present {
			break
		}
		if time.Now().After(deadline) {
			test.Fatalf("a resolved kind stayed in the gauge: %v", series)
		}
		time.Sleep(5 * time.Millisecond)
	}
}
