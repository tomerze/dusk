//go:build integration

package alerts_test

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func TestAlertsDeduplicateAndNotify(test *testing.T) {
	pool, _ := testsupport.Database(test)
	delivered := make(chan alerts.Alert, 4)
	webhook := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		body, _ := io.ReadAll(request.Body)
		var alert alerts.Alert
		_ = json.Unmarshal(body, &alert)
		delivered <- alert
	}))
	defer webhook.Close()
	store := alerts.NewStore(pool, webhook.URL, testsupport.Logger())
	operation, cancel := context.WithCancel(context.Background())
	defer cancel()
	go store.RunWebhook(operation)
	raised := alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindProcessWithoutIntent, Fingerprint: "process_without_intent:12808937078074471924", Detail: map[string]any{"pid": "12808937078074471924"}}
	first, failure := store.Raise(operation, raised)
	if failure != nil {
		test.Fatal(failure)
	}
	second, failure := store.Raise(operation, raised)
	if failure != nil || second.ID != first.ID || second.Occurrences != 2 {
		test.Fatalf("repeat %+v %v", second, failure)
	}
	select {
	case alert := <-delivered:
		if alert.ID != first.ID || alert.Kind != alerts.KindProcessWithoutIntent {
			test.Fatalf("webhook got %+v", alert)
		}
	case <-time.After(10 * time.Second):
		test.Fatal("webhook not called")
	}
	select {
	case alert := <-delivered:
		test.Fatalf("a repeat was announced again: %+v", alert)
	case <-time.After(300 * time.Millisecond):
	}
	if _, failure := store.Raise(operation, alerts.Raised{Severity: alerts.Medium, Kind: alerts.KindQuarantineOverride, Fingerprint: "quarantine_override:x"}); failure != nil {
		test.Fatal(failure)
	}
	select {
	case alert := <-delivered:
		test.Fatalf("a medium alert reached the webhook: %+v", alert)
	case <-time.After(300 * time.Millisecond):
	}
	acknowledged, failure := store.Acknowledge(operation, first.ID, "operator@example.org", time.Now())
	if failure != nil || acknowledged.AcknowledgedBy == nil {
		test.Fatalf("acknowledge %+v %v", acknowledged, failure)
	}
	if _, failure := store.Resolve(operation, first.ID, "operator@example.org", time.Now()); failure != nil {
		test.Fatal(failure)
	}
	reopened, failure := store.Raise(operation, raised)
	if failure != nil || reopened.ID == first.ID || reopened.Occurrences != 1 {
		test.Fatalf("a resolved alert must open a new one: %+v %v", reopened, failure)
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
	store := alerts.NewStore(pool, "", testsupport.Logger())
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
