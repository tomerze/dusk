package engine

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"net"
	"net/netip"
	"sort"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

const (
	signalIssued     = "issued"
	signalRefused    = "refused"
	signalNewNetwork = "new_network"
	signalQuota      = "quota"

	scopeFleet      = "fleet"
	scopeCredential = "credential"
	scopeNetwork    = "network"

	maximumWatchedNetworks = 100_000
	maximumNetworkAlerts   = 10
	maximumKnownNetworks   = 1_000_000
	maximumPendingCounts   = 1_000_000
	contributorsPerMinute  = 64
	contributorsReported   = 10
	watchSweepInterval     = 15 * time.Second
	enrollmentResolver     = "twilight"
)

var observedEnrollments = promauto.NewCounterVec(prometheus.CounterOpts{
	Name: "twilight_enrollment_observations_total",
	Help: "New enrollments the enrollment alerts counted, by signal.",
}, []string{"signal"})

type watchKey struct {
	signal string
	scope  string
	name   string
}

func (key watchKey) kind() string {
	switch key.signal {
	case signalRefused:
		return alerts.KindDeniedEnrollmentsSpike
	case signalNewNetwork:
		return alerts.KindNewNetworkBurst
	case signalQuota:
		return alerts.KindCredentialQuotaReached
	default:
		return alerts.KindEnrollmentRate
	}
}

var watchSignals = map[string]string{
	alerts.KindEnrollmentRate:         signalIssued,
	alerts.KindDeniedEnrollmentsSpike: signalRefused,
	alerts.KindNewNetworkBurst:        signalNewNetwork,
	alerts.KindCredentialQuotaReached: signalQuota,
}

func keyOf(fingerprint string) (watchKey, bool) {
	parts := strings.SplitN(fingerprint, ":", 3)
	signal, known := watchSignals[parts[0]]
	if !known || len(parts) < 2 {
		return watchKey{}, false
	}
	key := watchKey{signal: signal, scope: parts[1]}
	if len(parts) == 3 {
		key.name = parts[2]
	}
	switch key.scope {
	case scopeCredential:
		return key, key.name != ""
	case scopeNetwork:
		return key, true
	case scopeFleet:
		return key, key.name == ""
	}
	return key, false
}

func (key watchKey) overflow() bool {
	return key.scope == scopeNetwork && key.name == ""
}

func (key watchKey) fingerprint() string {
	if key.name == "" {
		return key.kind() + ":" + key.scope
	}
	return key.kind() + ":" + key.scope + ":" + key.name
}

type hourKey struct {
	watchKey
	hour time.Time
}

type observation struct {
	at         time.Time
	signal     string
	quota      bool
	credential string
	tenant     string
	network    string
}

func networkOf(remote string) string {
	host, _, failure := net.SplitHostPort(remote)
	if failure != nil {
		host = remote
	}
	address, failure := netip.ParseAddr(host)
	if failure != nil {
		return ""
	}
	address = address.Unmap()
	bits := 48
	if address.Is4() {
		bits = 24
	}
	return netip.PrefixFrom(address, bits).Masked().String()
}

func observe(enrollment kafka.Enrollment, at time.Time) (observation, bool) {
	if enrollment.Operation != "assign" && enrollment.Operation != "enroll" {
		return observation{}, false
	}
	observed := observation{at: at, network: networkOf(enrollment.RemoteAddress)}
	switch {
	case enrollment.Operation == "enroll" && enrollment.Outcome == "issued":
		observed.signal = signalIssued
	case enrollment.Outcome == "denied" || enrollment.Outcome == "rate_limited":
		observed.signal = signalRefused
		observed.quota = enrollment.Reason != nil && *enrollment.Reason == "credential_quota_reached"
	default:
		return observation{}, false
	}
	switch {
	case enrollment.CredentialKind == "fleet_token" && enrollment.CredentialRef != nil:
		observed.credential = "fleet_token:" + *enrollment.CredentialRef
	case enrollment.CredentialKind == "install_token" && enrollment.CredentialIssuer != nil:
		observed.credential = "install_token:" + *enrollment.CredentialIssuer
	}
	if enrollment.Tenant != nil {
		observed.tenant = *enrollment.Tenant
	}
	return observed, true
}

type minuteCounts struct {
	count       int64
	networks    map[string]int64
	credentials map[string]int64
}

type watched struct {
	minutes     map[int64]*minuteCounts
	tenant      string
	open        bool
	notified    int64
	normalSince time.Time
}

type enrollmentHistory interface {
	Baseline(operation context.Context, key watchKey, hours []time.Time) (int64, error)
	Record(operation context.Context, counts map[hourKey]int64) error
	Networks(operation context.Context, since time.Time) (map[string]time.Time, error)
	Forget(operation context.Context, before time.Time) error
}

type enrollmentAlerting interface {
	OpenOfKinds(operation context.Context, kinds []string) ([]alerts.Alert, error)
	Open(operation context.Context, raised alerts.Raised) (alerts.Alert, bool, error)
	Escalate(operation context.Context, fingerprint string, severity alerts.Severity, detail map[string]any, at time.Time) (alerts.Alert, error)
	ResolveOpen(operation context.Context, fingerprint, actor string, at time.Time) (alerts.Alert, error)
}

type enrollmentWatch struct {
	settings      config.Enrollments
	window        time.Duration
	history       enrollmentHistory
	alerting      enrollmentAlerting
	logger        *slog.Logger
	kinds         []string
	keys          map[watchKey]*watched
	folded        map[string]map[string]int64
	networkAlerts map[string]int
	networkKeys   int
	pending       map[hourKey]int64
	networks      map[string]time.Time
	baselines     map[hourKey]float64
	latest        time.Time
	latestAt      time.Time
	forgotten     time.Time
	dropped       int64
}

func newEnrollmentWatch(settings config.Enrollments, history enrollmentHistory, alerting enrollmentAlerting, logger *slog.Logger) *enrollmentWatch {
	kinds := make([]string, 0, len(watchSignals))
	for kind := range watchSignals {
		kinds = append(kinds, kind)
	}
	sort.Strings(kinds)
	return &enrollmentWatch{settings: settings, window: time.Duration(settings.WindowSeconds) * time.Second, history: history, alerting: alerting, logger: logger, kinds: kinds,
		keys: map[watchKey]*watched{}, folded: map[string]map[string]int64{}, networkAlerts: map[string]int{}, pending: map[hourKey]int64{}, networks: map[string]time.Time{},
		baselines: map[hourKey]float64{}}
}

func (watch *enrollmentWatch) baselinePeriod() time.Duration {
	return time.Duration(watch.settings.BaselineDays) * 24 * time.Hour
}

func (watch *enrollmentWatch) load(operation context.Context, now time.Time) error {
	networks, failure := watch.history.Networks(operation, now.Add(-watch.baselinePeriod()))
	if failure != nil {
		return failure
	}
	open, failure := watch.alerting.OpenOfKinds(operation, watch.kinds)
	if failure != nil {
		return failure
	}
	watch.networks = networks
	watch.keys, watch.folded, watch.networkAlerts, watch.networkKeys = map[watchKey]*watched{}, map[string]map[string]int64{}, map[string]int{}, 0
	for _, alert := range open {
		key, parsed := keyOf(alert.Fingerprint)
		if !parsed {
			continue
		}
		entry := &watched{minutes: map[int64]*minuteCounts{}, notified: notifiedCount(alert, 0), normalSince: now}
		watch.keys[key] = entry
		watch.setOpen(key, entry, true)
		if key.scope == scopeNetwork && !key.overflow() {
			watch.networkKeys++
		}
	}
	watch.logger.Info("enrollment alerts loaded the networks enrollments came from and the alerts left open", "networks", len(networks), "days", watch.settings.BaselineDays,
		"open_alerts", len(open))
	return nil
}

func (watch *enrollmentWatch) clock(now time.Time) time.Time {
	if watch.latest.IsZero() {
		return now
	}
	return watch.latest.Add(now.Sub(watch.latestAt))
}

func (watch *enrollmentWatch) floor(key watchKey) int64 {
	var floors config.Floors
	switch {
	case key.signal == signalQuota || key.overflow():
		return 0
	case key.signal == signalRefused:
		floors = watch.settings.RefusedFloors
	case key.signal == signalNewNetwork:
		floors = config.Floors{Fleet: watch.settings.NewNetworkFloors.Fleet, Credential: watch.settings.NewNetworkFloors.Credential}
	default:
		floors = watch.settings.IssuedFloors
	}
	switch key.scope {
	case scopeCredential:
		return int64(floors.Credential)
	case scopeNetwork:
		return int64(floors.Network)
	default:
		return int64(floors.Fleet)
	}
}

func (watch *enrollmentWatch) newNetwork(network string, at time.Time) bool {
	if network == "" {
		return false
	}
	first, known := watch.networks[network]
	if !known || at.Sub(first) > watch.baselinePeriod() {
		if !known && len(watch.networks) >= maximumKnownNetworks {
			return false
		}
		watch.networks[network] = at
		first = at
	}
	return at.Sub(first) <= watch.window
}

func (watch *enrollmentWatch) add(operation context.Context, observed observation, now time.Time) {
	if observed.at.After(watch.latest) {
		watch.latest, watch.latestAt = observed.at, now
	}
	observedEnrollments.WithLabelValues(observed.signal).Inc()
	var touched []watchKey
	count := func(key watchKey) {
		if watch.count(key, observed) {
			touched = append(touched, key)
		}
	}
	signals := []string{observed.signal}
	if observed.signal == signalIssued && watch.newNetwork(observed.network, observed.at) {
		signals = append(signals, signalNewNetwork)
	}
	for _, signal := range signals {
		count(watchKey{signal: signal, scope: scopeFleet})
		if observed.credential != "" {
			count(watchKey{signal: signal, scope: scopeCredential, name: observed.credential})
		}
		if observed.network != "" && signal != signalNewNetwork {
			count(watchKey{signal: signal, scope: scopeNetwork, name: observed.network})
		}
	}
	if observed.quota {
		observedEnrollments.WithLabelValues(signalQuota).Inc()
		if observed.credential != "" {
			count(watchKey{signal: signalQuota, scope: scopeCredential, name: observed.credential})
		}
	}
	end := watch.clock(now)
	for _, key := range touched {
		watch.evaluate(operation, key, end)
	}
}

func bump(counts map[string]int64, name string) map[string]int64 {
	if name == "" {
		return counts
	}
	if counts == nil {
		counts = map[string]int64{}
	}
	if _, present := counts[name]; present || len(counts) < contributorsPerMinute {
		counts[name]++
	}
	return counts
}

func (watch *enrollmentWatch) count(key watchKey, observed observation) bool {
	entry := watch.keys[key]
	if entry == nil {
		if key.scope == scopeNetwork {
			if watch.networkKeys >= maximumWatchedNetworks {
				watch.dropped++
				if watch.dropped == 1 || watch.dropped%10000 == 0 {
					watch.logger.Warn("the enrollment alerts watch as many networks as they can; new networks are not counted until some go quiet",
						"networks", watch.networkKeys, "dropped", watch.dropped)
				}
				return false
			}
			watch.networkKeys++
		}
		entry = &watched{minutes: map[int64]*minuteCounts{}}
		watch.keys[key] = entry
	}
	minute := observed.at.Unix() / 60
	counts := entry.minutes[minute]
	if counts == nil {
		counts = &minuteCounts{}
		entry.minutes[minute] = counts
	}
	counts.count++
	if key.scope != scopeNetwork {
		counts.networks = bump(counts.networks, observed.network)
	}
	if key.scope != scopeCredential {
		counts.credentials = bump(counts.credentials, observed.credential)
	}
	if observed.tenant != "" {
		entry.tenant = observed.tenant
	}
	_, folded := watch.folded[key.signal][key.name]
	if entry.open || (key.scope == scopeNetwork && folded) {
		return true
	}
	hour := hourKey{watchKey: key, hour: observed.at.UTC().Truncate(time.Hour)}
	if _, present := watch.pending[hour]; present || len(watch.pending) < maximumPendingCounts {
		watch.pending[hour]++
	}
	return true
}

type contributor struct {
	Name  string `json:"name"`
	Count int64  `json:"count"`
}

func top(counts map[string]int64) []contributor {
	listed := make([]contributor, 0, len(counts))
	for name, count := range counts {
		listed = append(listed, contributor{Name: name, Count: count})
	}
	sort.Slice(listed, func(left, right int) bool {
		if listed[left].Count != listed[right].Count {
			return listed[left].Count > listed[right].Count
		}
		return listed[left].Name < listed[right].Name
	})
	return listed[:min(len(listed), contributorsReported)]
}

func (watch *enrollmentWatch) total(entry *watched, end time.Time) (int64, map[string]int64, map[string]int64) {
	last := end.Unix() / 60
	first := end.Add(-watch.window).Unix()/60 + 1
	var count int64
	networks, credentials := map[string]int64{}, map[string]int64{}
	for minute, counts := range entry.minutes {
		if minute < first {
			delete(entry.minutes, minute)
			continue
		}
		if minute > last {
			continue
		}
		count += counts.count
		for name, value := range counts.networks {
			networks[name] += value
		}
		for name, value := range counts.credentials {
			credentials[name] += value
		}
	}
	return count, networks, credentials
}

func (watch *enrollmentWatch) baseline(operation context.Context, key watchKey, end time.Time) (float64, error) {
	hour := end.UTC().Truncate(time.Hour)
	cached := hourKey{watchKey: key, hour: hour}
	if value, present := watch.baselines[cached]; present {
		return value, nil
	}
	hours := make([]time.Time, watch.settings.BaselineDays)
	for day := range hours {
		hours[day] = hour.Add(-time.Duration(day+1) * 24 * time.Hour)
	}
	sum, failure := watch.history.Baseline(operation, key, hours)
	if failure != nil {
		return 0, failure
	}
	value := float64(sum) / float64(watch.settings.BaselineDays) * watch.window.Hours()
	watch.baselines[cached] = value
	return value, nil
}

func splitCredential(credential string) (string, string) {
	kind, name, _ := strings.Cut(credential, ":")
	return kind, name
}

var watchMessages = map[string]string{
	signalIssued:     "More new installations enrolled than the baseline allows",
	signalRefused:    "More enrollments were refused than the baseline allows; someone may be probing provisioning",
	signalNewNetwork: "More new installations enrolled from networks not seen before than the baseline allows",
	signalQuota:      "A credential reached its installation cap and nightfall refuses its new enrollments",
}

const overflowMessage = "More networks went above their threshold than get an alert of their own; this alert names the others"

func (watch *enrollmentWatch) detail(key watchKey, entry *watched, count, threshold int64, baseline float64, networks, credentials map[string]int64) map[string]any {
	detail := map[string]any{"scope": key.scope, "count": count, "window_seconds": watch.settings.WindowSeconds, "threshold": threshold,
		"baseline": math.Round(baseline*100) / 100, "baseline_days": watch.settings.BaselineDays, "message": watchMessages[key.signal]}
	switch key.scope {
	case scopeCredential:
		kind, name := splitCredential(key.name)
		detail["credential_kind"], detail["credential"] = kind, name
		if entry.tenant != "" {
			detail["tenant"] = entry.tenant
		}
	case scopeNetwork:
		if key.overflow() {
			detail["message"], detail["network_alerts"] = overflowMessage, watch.networkAlerts[key.signal]
		} else {
			detail["network"] = key.name
		}
	}
	if key.scope != scopeNetwork || key.overflow() {
		detail["networks"], detail["distinct_networks"] = top(networks), len(networks)
	}
	if key.scope != scopeCredential && !key.overflow() {
		listed := top(credentials)
		for index := range listed {
			kind, name := splitCredential(listed[index].Name)
			listed[index].Name = kind + " " + name
		}
		detail["credentials"] = listed
	}
	return detail
}

func (watch *enrollmentWatch) evaluate(operation context.Context, key watchKey, end time.Time) {
	entry := watch.keys[key]
	if entry == nil {
		return
	}
	var count int64
	var networks, credentials map[string]int64
	if key.overflow() {
		networks = watch.folded[key.signal]
		count = int64(len(networks))
	} else {
		count, networks, credentials = watch.total(entry, end)
	}
	threshold := watch.floor(key)
	above := count > threshold
	var baseline float64
	if above && key.signal != signalQuota && !key.overflow() {
		measured, failure := watch.baseline(operation, key, end)
		if failure != nil && operation.Err() == nil {
			watch.logger.Warn("the enrollment baseline was not read; the floor alone is the threshold", "kind", key.kind(), "scope", key.scope, "name", key.name, "error", failure)
		}
		baseline = measured
		threshold = max(threshold, int64(math.Ceil(float64(watch.settings.BaselineFactor)*baseline)))
		above = count > threshold
	}
	network := key.scope == scopeNetwork && !key.overflow()
	if network && !above {
		watch.unfold(operation, key, end)
	}
	switch {
	case above && !entry.open && network && watch.networkAlerts[key.signal] >= maximumNetworkAlerts:
		watch.fold(operation, key, count, end)
	case above && !entry.open:
		var tenant string
		if key.scope == scopeCredential {
			tenant = entry.tenant
		}
		alert, inserted, failure := watch.alerting.Open(operation, alerts.Raised{Severity: alerts.High, Kind: key.kind(), Fingerprint: key.fingerprint(), Tenant: tenant,
			Detail: watch.detail(key, entry, count, threshold, baseline, networks, credentials), At: end})
		if failure != nil {
			if operation.Err() == nil {
				watch.logger.Warn("an enrollment alert was not raised; it is tried again on the next count", "kind", key.kind(), "fingerprint", key.fingerprint(), "error", failure)
			}
			return
		}
		watch.setOpen(key, entry, true)
		entry.notified, entry.normalSince = count, time.Time{}
		if !inserted {
			entry.notified = notifiedCount(alert, count)
		}
		if network {
			watch.unfold(operation, key, end)
		}
	case above:
		entry.normalSince = time.Time{}
		if count < entry.notified*int64(watch.settings.EscalationFactor) {
			return
		}
		_, failure := watch.alerting.Escalate(operation, key.fingerprint(), alerts.Critical, watch.detail(key, entry, count, threshold, baseline, networks, credentials), end)
		switch {
		case errors.Is(failure, alerts.ErrNotFound):
			watch.resolvedElsewhere(key, entry)
		case failure != nil:
			if operation.Err() == nil {
				watch.logger.Warn("an enrollment alert was not escalated; it is tried again on the next count", "kind", key.kind(), "fingerprint", key.fingerprint(), "error", failure)
			}
		default:
			entry.notified = count
		}
	case entry.open && entry.normalSince.IsZero():
		entry.normalSince = end
	case entry.open && end.Sub(entry.normalSince) >= time.Duration(watch.settings.ResolveAfterSeconds)*time.Second:
		if _, failure := watch.alerting.ResolveOpen(operation, key.fingerprint(), enrollmentResolver, end); failure != nil && !errors.Is(failure, alerts.ErrNotFound) {
			if operation.Err() == nil {
				watch.logger.Warn("an enrollment alert back to normal was not resolved; it is tried again on the next sweep", "kind", key.kind(), "fingerprint", key.fingerprint(), "error", failure)
			}
			return
		}
		watch.setOpen(key, entry, false)
		entry.notified, entry.normalSince = 0, time.Time{}
	}
}

func (watch *enrollmentWatch) setOpen(key watchKey, entry *watched, open bool) {
	if entry.open == open {
		return
	}
	entry.open = open
	if key.scope != scopeNetwork || key.overflow() {
		return
	}
	if open {
		watch.networkAlerts[key.signal]++
	} else {
		watch.networkAlerts[key.signal]--
	}
}

func (watch *enrollmentWatch) fold(operation context.Context, key watchKey, count int64, end time.Time) {
	folded := watch.folded[key.signal]
	if folded == nil {
		folded = map[string]int64{}
		watch.folded[key.signal] = folded
	}
	_, present := folded[key.name]
	folded[key.name] = count
	if present {
		return
	}
	overflow := watchKey{signal: key.signal, scope: scopeNetwork}
	if watch.keys[overflow] == nil {
		watch.keys[overflow] = &watched{minutes: map[int64]*minuteCounts{}}
	}
	watch.evaluate(operation, overflow, end)
}

func (watch *enrollmentWatch) unfold(operation context.Context, key watchKey, end time.Time) {
	if _, present := watch.folded[key.signal][key.name]; !present {
		return
	}
	delete(watch.folded[key.signal], key.name)
	watch.evaluate(operation, watchKey{signal: key.signal, scope: scopeNetwork}, end)
}

func (watch *enrollmentWatch) resolvedElsewhere(key watchKey, entry *watched) {
	watch.logger.Info("an enrollment alert was resolved outside the enrollment alerts; its counts start over and its next breach opens a new alert",
		"kind", key.kind(), "fingerprint", key.fingerprint())
	watch.setOpen(key, entry, false)
	entry.minutes, entry.notified, entry.normalSince = map[int64]*minuteCounts{}, 0, time.Time{}
	if !key.overflow() {
		return
	}
	for network := range watch.folded[key.signal] {
		if folded := watch.keys[watchKey{signal: key.signal, scope: scopeNetwork, name: network}]; folded != nil {
			folded.minutes = map[int64]*minuteCounts{}
		}
	}
	delete(watch.folded, key.signal)
}

func notifiedCount(alert alerts.Alert, fallback int64) int64 {
	var detail struct {
		Count *int64 `json:"count"`
	}
	if failure := json.Unmarshal(alert.Detail, &detail); failure != nil || detail.Count == nil {
		return fallback
	}
	return *detail.Count
}

func (watch *enrollmentWatch) sweep(operation context.Context, now time.Time) {
	end := watch.clock(now)
	hour := end.UTC().Truncate(time.Hour)
	for cached := range watch.baselines {
		if !cached.hour.Equal(hour) {
			delete(watch.baselines, cached)
		}
	}
	open, failure := watch.alerting.OpenOfKinds(operation, watch.kinds)
	if failure != nil && operation.Err() == nil {
		watch.logger.Warn("the open enrollment alerts were not read; an alert resolved by hand is noticed on a later sweep", "error", failure)
	}
	if failure == nil {
		still := make(map[string]bool, len(open))
		for _, alert := range open {
			still[alert.Fingerprint] = true
		}
		for key, entry := range watch.keys {
			if entry.open && !still[key.fingerprint()] {
				watch.resolvedElsewhere(key, entry)
			}
		}
	}
	for key, entry := range watch.keys {
		watch.evaluate(operation, key, end)
		if entry.open || len(entry.minutes) > 0 || (key.overflow() && len(watch.folded[key.signal]) > 0) {
			continue
		}
		delete(watch.keys, key)
		if key.scope == scopeNetwork && !key.overflow() {
			watch.networkKeys--
		}
	}
	for network, first := range watch.networks {
		if end.Sub(first) > watch.baselinePeriod() {
			delete(watch.networks, network)
		}
	}
	watch.flush(operation)
	if end.Sub(watch.forgotten) >= time.Hour {
		if failure := watch.history.Forget(operation, end.Add(-watch.baselinePeriod()-24*time.Hour)); failure != nil {
			if operation.Err() == nil {
				watch.logger.Warn("old enrollment counts were not deleted", "error", failure)
			}
		} else {
			watch.forgotten = end
		}
	}
}

func (watch *enrollmentWatch) flush(operation context.Context) {
	if len(watch.pending) == 0 {
		return
	}
	if failure := watch.history.Record(operation, watch.pending); failure != nil {
		if operation.Err() == nil {
			watch.logger.Warn("enrollment counts were not written; they are written with the next ones", "counts", len(watch.pending), "error", failure)
		}
		return
	}
	watch.pending = map[hourKey]int64{}
}

func (watch *enrollmentWatch) run(operation context.Context, observations <-chan observation) {
	for {
		if failure := watch.load(operation, time.Now()); failure == nil {
			break
		} else if operation.Err() == nil {
			watch.logger.Warn("the networks enrollments came from were not read; every network counts as new until they are", "error", failure)
		}
		if !sleep(operation, jitter(10*time.Second)) {
			return
		}
	}
	ticker := time.NewTicker(watchSweepInterval)
	defer ticker.Stop()
	for {
		select {
		case <-operation.Done():
			flushing, cancel := context.WithTimeout(context.WithoutCancel(operation), 5*time.Second)
			watch.flush(flushing)
			cancel()
			return
		case observed := <-observations:
			watch.add(operation, observed, time.Now())
		case <-ticker.C:
			watch.sweep(operation, time.Now())
		}
	}
}

type postgresEnrollmentHistory struct {
	pool *pgxpool.Pool
}

func (history postgresEnrollmentHistory) Baseline(operation context.Context, key watchKey, hours []time.Time) (int64, error) {
	var sum int64
	failure := history.pool.QueryRow(operation, `select coalesce(sum(count), 0)::bigint from enrollment_counts
		where signal = $1 and scope = $2 and name = $3 and hour = any($4)`, key.signal, key.scope, key.name, hours).Scan(&sum)
	return sum, failure
}

func (history postgresEnrollmentHistory) Record(operation context.Context, counts map[hourKey]int64) error {
	signals, scopes, names := make([]string, 0, len(counts)), make([]string, 0, len(counts)), make([]string, 0, len(counts))
	hours, values := make([]time.Time, 0, len(counts)), make([]int64, 0, len(counts))
	for key, value := range counts {
		signals, scopes, names = append(signals, key.signal), append(scopes, key.scope), append(names, key.name)
		hours, values = append(hours, key.hour), append(values, value)
	}
	_, failure := history.pool.Exec(operation, `insert into enrollment_counts (signal, scope, name, hour, count)
		select * from unnest($1::text[], $2::text[], $3::text[], $4::timestamptz[], $5::bigint[])
		on conflict (signal, scope, name, hour) do update set count = enrollment_counts.count + excluded.count`, signals, scopes, names, hours, values)
	return failure
}

func (history postgresEnrollmentHistory) Networks(operation context.Context, since time.Time) (map[string]time.Time, error) {
	rows, failure := history.pool.Query(operation, `select name, min(hour) from enrollment_counts
		where signal = 'issued' and scope = 'network' and hour >= $1 group by name limit $2`, since, maximumKnownNetworks)
	if failure != nil {
		return nil, failure
	}
	networks := map[string]time.Time{}
	var name string
	var first time.Time
	_, failure = pgx.ForEachRow(rows, []any{&name, &first}, func() error {
		networks[name] = first
		return nil
	})
	if failure != nil {
		return nil, failure
	}
	return networks, nil
}

func (history postgresEnrollmentHistory) Forget(operation context.Context, before time.Time) error {
	_, failure := history.pool.Exec(operation, `delete from enrollment_counts where hour < $1`, before)
	return failure
}

func (engine *Engine) runEnrollments(operation context.Context, term int64) {
	logger := engine.Logger.With("term", term)
	watch := newEnrollmentWatch(engine.Config.Alerts.Enrollments, postgresEnrollmentHistory{pool: engine.Pool}, engine.Alerts, logger)
	observations := make(chan observation, 10000)
	go watch.run(operation, observations)
	kafka.ConsumeGroup(operation, engine.KafkaOptions, "enrollments", engine.Config.Kafka.InventoryGroup, engine.Config.Kafka.Topics.Enrollments, logger, func(operation context.Context, record *kgo.Record) error {
		if record.Value == nil {
			return nil
		}
		enrollment, valid := kafka.Decode[kafka.Enrollment](engine.Validator, kafka.ContractEnrollments, record, engine.Logger)
		if !valid {
			return nil
		}
		at, failure := kafka.ParseTime(enrollment.Time)
		if failure != nil {
			at = time.Now()
		}
		if enrollment.Outcome == "issued" && enrollment.DeviceID != nil && enrollment.InstallationID != nil && (enrollment.Operation == "enroll" || enrollment.Operation == "renew") {
			key := NodeKey{DeviceID: *enrollment.DeviceID, InstallationID: *enrollment.InstallationID}
			if failure := engine.Inventory.RecordEnrollment(operation, inventory.Enrollment{Key: key, Operation: enrollment.Operation, CertFingerprint: enrollment.CertFingerprint,
				Tenant: enrollment.Tenant, DuskVersion: enrollment.DuskVersion, Impl: enrollment.Impl, TargetArch: enrollment.TargetArch, Hostname: enrollment.Hostname,
				CredentialKind: enrollment.CredentialKind, CredentialRef: enrollment.CredentialRef, CredentialIssuer: enrollment.CredentialIssuer, At: at}); failure != nil {
				return fmt.Errorf("record the enrollment of %s in inventory: %w", key, failure)
			}
			engine.Logger.Info("enrollment recorded", "operation", enrollment.Operation, "device_id", key.DeviceID, "installation_id", key.InstallationID)
		}
		if observed, counted := observe(enrollment, at); counted {
			select {
			case observations <- observed:
			case <-operation.Done():
				return operation.Err()
			}
		}
		return nil
	})
}
