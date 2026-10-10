package engine

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"testing"
	"time"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
)

type fakeHistory struct {
	hourly    map[hourKey]int64
	networks  map[string]time.Time
	forgotten []time.Time
}

func (history *fakeHistory) Baseline(operation context.Context, key watchKey, hours []time.Time) (int64, error) {
	var sum int64
	for _, hour := range hours {
		sum += history.hourly[hourKey{watchKey: key, hour: hour}]
	}
	return sum, nil
}

func (history *fakeHistory) Record(operation context.Context, counts map[hourKey]int64) error {
	for key, value := range counts {
		history.hourly[key] += value
	}
	return nil
}

func (history *fakeHistory) Networks(operation context.Context, since time.Time) (map[string]time.Time, error) {
	copied := map[string]time.Time{}
	for network, first := range history.networks {
		copied[network] = first
	}
	return copied, nil
}

func (history *fakeHistory) Forget(operation context.Context, before time.Time) error {
	history.forgotten = append(history.forgotten, before)
	return nil
}

type escalation struct {
	fingerprint string
	detail      map[string]any
}

type fakeAlerting struct {
	opened    []alerts.Raised
	escalated []escalation
	resolved  []string
	open      map[string]alerts.Raised
}

func (alerting *fakeAlerting) OpenOfKinds(operation context.Context, kinds []string) ([]alerts.Alert, error) {
	var listed []alerts.Alert
	for fingerprint, raised := range alerting.open {
		detail, _ := json.Marshal(raised.Detail)
		listed = append(listed, alerts.Alert{Kind: raised.Kind, Fingerprint: fingerprint, Detail: detail})
	}
	return listed, nil
}

func (alerting *fakeAlerting) Open(operation context.Context, raised alerts.Raised) (alerts.Alert, bool, error) {
	if existing, present := alerting.open[raised.Fingerprint]; present {
		detail, _ := json.Marshal(existing.Detail)
		return alerts.Alert{Fingerprint: raised.Fingerprint, Detail: detail}, false, nil
	}
	alerting.open[raised.Fingerprint] = raised
	alerting.opened = append(alerting.opened, raised)
	return alerts.Alert{Fingerprint: raised.Fingerprint}, true, nil
}

func (alerting *fakeAlerting) Escalate(operation context.Context, fingerprint string, detail map[string]any, at time.Time) (alerts.Alert, error) {
	alerting.escalated = append(alerting.escalated, escalation{fingerprint: fingerprint, detail: detail})
	return alerts.Alert{Fingerprint: fingerprint}, nil
}

func (alerting *fakeAlerting) ResolveOpen(operation context.Context, fingerprint, actor string, at time.Time) (alerts.Alert, error) {
	if actor != enrollmentResolver {
		return alerts.Alert{}, fmt.Errorf("resolved by %s", actor)
	}
	delete(alerting.open, fingerprint)
	alerting.resolved = append(alerting.resolved, fingerprint)
	return alerts.Alert{Fingerprint: fingerprint}, nil
}

func (alerting *fakeAlerting) fingerprints() []string {
	listed := make([]string, len(alerting.opened))
	for index, raised := range alerting.opened {
		listed[index] = raised.Fingerprint
	}
	return listed
}

var watchStart = time.Date(2026, 10, 9, 14, 0, 0, 0, time.UTC)

func newTestWatch(test *testing.T) (*enrollmentWatch, *fakeHistory, *fakeAlerting) {
	settings := config.Default().Alerts.Enrollments
	history := &fakeHistory{hourly: map[hourKey]int64{}, networks: map[string]time.Time{}}
	alerting := &fakeAlerting{open: map[string]alerts.Raised{}}
	watch := newEnrollmentWatch(settings, history, alerting, slog.New(slog.NewTextHandler(io.Discard, nil)))
	if failure := watch.load(context.Background(), watchStart); failure != nil {
		test.Fatal(failure)
	}
	return watch, history, alerting
}

func (history *fakeHistory) seed(key watchKey, perHour int64, days int) {
	for day := 1; day <= days; day++ {
		history.hourly[hourKey{watchKey: key, hour: watchStart.Add(-time.Duration(day) * 24 * time.Hour)}] = perHour
	}
}

func enrollment(operation, outcome string, reason *string, credentialKind string, reference, issuer *string, remote string) kafka.Enrollment {
	return kafka.Enrollment{Operation: operation, Outcome: outcome, Reason: reason, CredentialKind: credentialKind, CredentialRef: reference, CredentialIssuer: issuer, RemoteAddress: remote}
}

func text(value string) *string {
	return &value
}

func burst(watch *enrollmentWatch, start time.Time, count int, spread time.Duration, make func(index int) kafka.Enrollment) time.Time {
	at := start
	for index := range count {
		at = start.Add(time.Duration(index) * spread / time.Duration(max(count, 1)))
		if observed, counted := observe(make(index), at); counted {
			watch.add(context.Background(), observed, at)
		}
	}
	return at
}

func TestNetworksAreSlash24AndSlash48(test *testing.T) {
	for remote, expected := range map[string]string{
		"203.0.113.24:51730":         "203.0.113.0/24",
		"[2001:db8:4::17]:40110":     "2001:db8:4::/48",
		"[::ffff:198.51.100.7]:4000": "198.51.100.0/24",
		"198.51.100.200":             "198.51.100.0/24",
		"not an address":             "",
	} {
		if network := networkOf(remote); network != expected {
			test.Errorf("%s is in %q, not %q", remote, network, expected)
		}
	}
}

func TestOnlyNewEnrollmentsAreCountedAndRenewalsNever(test *testing.T) {
	quota := text("credential_quota_reached")
	cases := []struct {
		enrollment kafka.Enrollment
		counted    bool
		signal     string
		quota      bool
		credential string
	}{
		{enrollment("renew", "issued", nil, "certificate", nil, nil, "203.0.113.1:1"), false, "", false, ""},
		{enrollment("renew", "denied", text("renew_limit"), "certificate", nil, nil, "203.0.113.1:1"), false, "", false, ""},
		{enrollment("assign", "assigned", nil, "fleet_token", text("lab"), nil, "203.0.113.1:1"), false, "", false, ""},
		{enrollment("enroll", "error", text("step_ca_error"), "fleet_token", text("lab"), nil, "203.0.113.1:1"), false, "", false, ""},
		{enrollment("enroll", "issued", nil, "fleet_token", text("lab"), nil, "203.0.113.1:1"), true, signalIssued, false, "fleet_token:lab"},
		{enrollment("assign", "denied", text("invalid_credential"), "fleet_token", nil, nil, "203.0.113.1:1"), true, signalRefused, false, ""},
		{enrollment("enroll", "rate_limited", text("credential_rate"), "fleet_token", text("lab"), nil, "203.0.113.1:1"), true, signalRefused, false, "fleet_token:lab"},
		{enrollment("assign", "denied", quota, "install_token", text("unit-7"), text("factory-2026"), "203.0.113.1:1"), true, signalRefused, true, "install_token:factory-2026"},
		{enrollment("enroll", "issued", nil, "install_token", text("unit-8"), nil, "203.0.113.1:1"), true, signalIssued, false, ""},
	}
	for index, current := range cases {
		observed, counted := observe(current.enrollment, watchStart)
		if counted != current.counted || observed.signal != current.signal || observed.quota != current.quota || observed.credential != current.credential {
			test.Errorf("case %d: counted %v signal %q quota %v credential %q", index, counted, observed.signal, observed.quota, observed.credential)
		}
	}
	watch, _, alerting := newTestWatch(test)
	burst(watch, watchStart, 6100, 10*time.Minute, func(index int) kafka.Enrollment {
		return enrollment("renew", "issued", nil, "certificate", nil, nil, fmt.Sprintf("198.51.%d.%d:443", index%250, index%200))
	})
	watch.sweep(context.Background(), watchStart.Add(10*time.Minute))
	if len(alerting.opened) != 0 || len(watch.keys) != 0 {
		test.Fatalf("renewals raised %v or were counted under %d keys", alerting.fingerprints(), len(watch.keys))
	}
}

func TestAFloodOnOneCredentialPagesWithItsOwnFingerprintAndNamesIt(test *testing.T) {
	watch, history, alerting := newTestWatch(test)
	history.seed(watchKey{signal: signalIssued, scope: scopeCredential, name: "fleet_token:retail-eu-2026"}, 300, 7)
	history.seed(watchKey{signal: signalIssued, scope: scopeFleet}, 100_000, 7)
	for index := range 40 {
		history.networks[fmt.Sprintf("100.64.%d.0/24", index)] = watchStart.Add(-48 * time.Hour)
	}
	if failure := watch.load(context.Background(), watchStart); failure != nil {
		test.Fatal(failure)
	}
	tenant := text("retail-eu")
	quiet := func(index int) kafka.Enrollment {
		event := enrollment("enroll", "issued", nil, "fleet_token", text("lab"), nil, fmt.Sprintf("100.64.%d.9:4000", index%40))
		return event
	}
	burst(watch, watchStart, 10, time.Minute, quiet)
	flood := func(index int) kafka.Enrollment {
		event := enrollment("enroll", "issued", nil, "fleet_token", text("retail-eu-2026"), nil, fmt.Sprintf("100.64.%d.%d:4000", index%40, index%250))
		event.Tenant = tenant
		return event
	}
	end := burst(watch, watchStart.Add(time.Minute), 300, time.Minute, flood)
	expected := "enrollment_rate:credential:fleet_token:retail-eu-2026"
	if fingerprints := alerting.fingerprints(); len(fingerprints) != 1 || fingerprints[0] != expected {
		test.Fatalf("raised %v, expected only %s", fingerprints, expected)
	}
	raised := alerting.opened[0]
	if raised.Kind != alerts.KindEnrollmentRate || raised.Severity != alerts.High {
		test.Fatalf("raised %s %s", raised.Severity, raised.Kind)
	}
	detail := raised.Detail
	if detail["credential"] != "retail-eu-2026" || detail["credential_kind"] != "fleet_token" || detail["tenant"] != "retail-eu" || detail["baseline"] != 25.0 || detail["threshold"] != int64(100) {
		test.Fatalf("detail %v", detail)
	}
	if count := detail["count"].(int64); count != 101 {
		test.Fatalf("raised at count %d, expected the 101st enrollment over a threshold of 100", count)
	}
	if networks := detail["networks"].([]contributor); len(networks) != contributorsReported || detail["distinct_networks"] != 40 {
		test.Fatalf("networks %v, %v distinct", networks, detail["distinct_networks"])
	}
	burst(watch, end, 40, time.Minute, func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "install_token", text(fmt.Sprintf("unit-%d", index)), text("factory-2026"), fmt.Sprintf("100.64.%d.1:4000", index))
	})
	if fingerprints := alerting.fingerprints(); len(fingerprints) != 2 || fingerprints[1] != "enrollment_rate:credential:install_token:factory-2026" {
		test.Fatalf("a flood on a second credential raised %v", fingerprints)
	}
}

func TestAnOpenAlertNotifiesAgainWhenTheRateDoublesAndResolvesOnceNormal(test *testing.T) {
	watch, history, alerting := newTestWatch(test)
	history.seed(watchKey{signal: signalIssued, scope: scopeFleet}, 1_000_000, 7)
	watch.settings.NewNetworkFloors = config.NewNetworkFloors{Fleet: 1_000_000, Credential: 1_000_000}
	key := watchKey{signal: signalIssued, scope: scopeCredential, name: "fleet_token:lab"}
	flood := func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "fleet_token", text("lab"), nil, fmt.Sprintf("192.0.%d.1:1", index%200))
	}
	end := burst(watch, watchStart, 31, time.Minute, flood)
	if len(alerting.opened) != 1 || alerting.opened[0].Fingerprint != key.fingerprint() {
		test.Fatalf("raised %v", alerting.fingerprints())
	}
	end = burst(watch, end, 29, 30*time.Second, flood)
	if len(alerting.escalated) != 0 {
		test.Fatalf("escalated below twice the notified count: %v", alerting.escalated)
	}
	end = burst(watch, end, 5, 10*time.Second, flood)
	if len(alerting.escalated) != 1 || alerting.escalated[0].fingerprint != key.fingerprint() || alerting.escalated[0].detail["count"].(int64) != 62 {
		test.Fatalf("escalations %v", alerting.escalated)
	}
	watch.sweep(context.Background(), end.Add(6*time.Minute))
	watch.sweep(context.Background(), end.Add(6*time.Minute+14*time.Minute))
	if len(alerting.resolved) != 0 {
		test.Fatalf("resolved before resolve_after_seconds of normal rate: %v", alerting.resolved)
	}
	watch.sweep(context.Background(), end.Add(6*time.Minute+15*time.Minute))
	if len(alerting.resolved) != 1 || alerting.resolved[0] != key.fingerprint() {
		test.Fatalf("resolved %v", alerting.resolved)
	}
	burst(watch, end.Add(30*time.Minute), 31, time.Minute, flood)
	if len(alerting.opened) != 2 {
		test.Fatalf("a flood after the alert resolved raised %v", alerting.fingerprints())
	}
	if len(history.forgotten) == 0 {
		test.Fatal("old counts were never deleted")
	}
	if history.hourly[hourKey{watchKey: key, hour: watchStart}] == 0 {
		test.Fatal("the counts were never written for the baseline")
	}
}

func TestAnOpenAlertTakenOverAfterAFailoverKeepsItsNotifiedCount(test *testing.T) {
	watch, _, alerting := newTestWatch(test)
	key := watchKey{signal: signalIssued, scope: scopeNetwork, name: "192.0.2.0/24"}
	alerting.open[key.fingerprint()] = alerts.Raised{Fingerprint: key.fingerprint(), Detail: map[string]any{"count": 40}}
	burst(watch, watchStart, 79, time.Minute, func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "fleet_token", nil, nil, "192.0.2.7:1")
	})
	if len(alerting.escalated) != 0 {
		test.Fatalf("escalated below twice the count the earlier leader notified: %v", alerting.escalated)
	}
	burst(watch, watchStart.Add(time.Minute), 1, time.Second, func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "fleet_token", nil, nil, "192.0.2.7:1")
	})
	if len(alerting.escalated) != 1 {
		test.Fatalf("escalations %v", alerting.escalated)
	}
}

func TestRefusalsQuotaAndNewNetworksRaiseTheirOwnAlerts(test *testing.T) {
	watch, history, alerting := newTestWatch(test)
	for index := range 30 {
		history.networks[fmt.Sprintf("198.51.%d.0/24", index)] = watchStart.Add(-72 * time.Hour)
	}
	if failure := watch.load(context.Background(), watchStart); failure != nil {
		test.Fatal(failure)
	}
	end := burst(watch, watchStart, 61, time.Minute, func(index int) kafka.Enrollment {
		return enrollment("assign", "denied", text("invalid_credential"), "fleet_token", nil, nil, "203.0.113.9:1")
	})
	if fingerprints := alerting.fingerprints(); len(fingerprints) != 1 || fingerprints[0] != "denied_enrollments_spike:network:203.0.113.0/24" {
		test.Fatalf("refusals raised %v", fingerprints)
	}
	end = burst(watch, end, 1, time.Second, func(index int) kafka.Enrollment {
		return enrollment("assign", "denied", text("credential_quota_reached"), "fleet_token", text("batch-7"), nil, "198.51.29.4:1")
	})
	quota := alerting.opened[len(alerting.opened)-1]
	if quota.Fingerprint != "credential_quota_reached:credential:fleet_token:batch-7" || quota.Kind != alerts.KindCredentialQuotaReached || quota.Detail["credential"] != "batch-7" {
		test.Fatalf("the first quota refusal raised %v", quota)
	}
	before := len(alerting.opened)
	burst(watch, end, 30, time.Minute, func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "fleet_token", text("batch-8"), nil, fmt.Sprintf("198.51.%d.4:1", index))
	})
	burst(watch, end.Add(time.Minute), 21, time.Minute, func(index int) kafka.Enrollment {
		return enrollment("enroll", "issued", nil, "fleet_token", text("batch-8"), nil, fmt.Sprintf("100.%d.%d.1:1", 64+index/250, index%250))
	})
	raised := alerting.fingerprints()[before:]
	if len(raised) != 2 || raised[0] != "enrollment_rate:credential:fleet_token:batch-8" || raised[1] != "enrollment_from_new_network_burst:credential:fleet_token:batch-8" {
		test.Fatalf("raised %v", raised)
	}
	newNetworks := alerting.opened[len(alerting.opened)-1]
	if newNetworks.Detail["distinct_networks"] != 21 || newNetworks.Detail["count"].(int64) != 21 {
		test.Fatalf("the new-network alert detail %v", newNetworks.Detail)
	}
}

func TestAnAlertAnEarlierLeaderLeftOpenResolvesOnceNothingMoreComes(test *testing.T) {
	watch, _, alerting := newTestWatch(test)
	key := watchKey{signal: signalRefused, scope: scopeCredential, name: "fleet_token:retail-eu:2026"}
	alerting.open[key.fingerprint()] = alerts.Raised{Kind: key.kind(), Fingerprint: key.fingerprint(), Detail: map[string]any{"count": 75}}
	alerting.open["revocation_not_enforced:0011/2233"] = alerts.Raised{Kind: alerts.KindRevocationNotEnforced, Fingerprint: "revocation_not_enforced:0011/2233"}
	if failure := watch.load(context.Background(), watchStart); failure != nil {
		test.Fatal(failure)
	}
	if entry := watch.keys[key]; entry == nil || !entry.open || entry.notified != 75 {
		test.Fatalf("the open alert was not taken over: %+v", entry)
	}
	watch.sweep(context.Background(), watchStart.Add(14*time.Minute))
	if len(alerting.resolved) != 0 {
		test.Fatalf("resolved too early: %v", alerting.resolved)
	}
	watch.sweep(context.Background(), watchStart.Add(15*time.Minute))
	if len(alerting.resolved) != 1 || alerting.resolved[0] != key.fingerprint() {
		test.Fatalf("resolved %v", alerting.resolved)
	}
	if _, parsed := keyOf("enrollment_rate:fleet"); !parsed {
		test.Fatal("a fleet fingerprint did not parse")
	}
	if _, parsed := keyOf("enrollment_rate:galaxy:x"); parsed {
		test.Fatal("an unknown scope parsed")
	}
}
