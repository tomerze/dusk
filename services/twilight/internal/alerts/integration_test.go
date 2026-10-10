//go:build integration

package alerts_test

import (
	"context"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
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
