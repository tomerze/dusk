//go:build integration

package reconcile

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

const strangerDevice = "00000000000000000000000000000def"

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

type reconcileHarness struct {
	test      *testing.T
	pool      *pgxpool.Pool
	settings  config.Config
	options   []kgo.Opt
	validator *kafka.Validator
	keys      *VerifyingKeys
	producer  *kgo.Client
	cancel    context.CancelFunc
	stopped   chan struct{}
}

func newReconcileHarness(test *testing.T, keys *VerifyingKeys) *reconcileHarness {
	pool, url := testsupport.Database(test)
	brokers := testsupport.Brokers(test)
	suffix := strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	settings := config.Default()
	settings.Instance = "twilight-test"
	settings.Database.URL, settings.Database.LeaderURL = url, url
	settings.Kafka.Brokers = []string{brokers}
	settings.Kafka.AllowPlaintext = true
	settings.Kafka.Topics.Ledger = "dusk.ledger." + suffix
	settings.Kafka.Topics.ProcessResults = "dusk.process-results." + suffix
	settings.Kafka.ReconcileGroup += "-" + suffix
	settings.Kafka.ReconcileResultsGroup += "-" + suffix
	options, failure := kafka.Options(settings.Kafka, "test")
	if failure != nil {
		test.Fatal(failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	producer, failure := kgo.NewClient(append(append([]kgo.Opt(nil), options...),
		kgo.TransactionalID("nightfall-ledger-test-"+suffix),
		kgo.RecordPartitioner(kgo.ManualPartitioner()),
	)...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(producer.Close)
	admin := kadm.NewClient(producer)
	remove := "delete"
	for _, topic := range []string{settings.Kafka.Topics.Ledger, settings.Kafka.Topics.ProcessResults} {
		if _, failure := admin.CreateTopic(context.Background(), 3, 1, map[string]*string{"cleanup.policy": &remove}, topic); failure != nil {
			test.Fatal(failure)
		}
	}
	return &reconcileHarness{test: test, pool: pool, settings: settings, options: options, validator: validator, keys: keys, producer: producer}
}

func (harness *reconcileHarness) start() {
	operation, cancel := context.WithCancel(context.Background())
	harness.cancel = cancel
	harness.stopped = make(chan struct{})
	logger := testsupport.Logger()
	reconciler := New(Dependencies{
		Config: harness.settings, Pool: harness.pool, Alerts: testsupport.AlertStore(harness.test, harness.pool), Validator: harness.validator,
		KafkaOptions: harness.options, Keys: harness.keys, Logger: logger,
	})
	go func() {
		defer close(harness.stopped)
		reconciler.Run(operation)
	}()
	harness.test.Cleanup(harness.stop)
}

func (harness *reconcileHarness) stop() {
	if harness.cancel == nil {
		return
	}
	harness.cancel()
	harness.cancel = nil
	select {
	case <-harness.stopped:
	case <-time.After(30 * time.Second):
		harness.test.Error("reconcile did not stop within 30 s")
	}
}

func (harness *reconcileHarness) produceLedger(partition int32, entries ...kafka.LedgerEntry) {
	harness.test.Helper()
	operation := context.Background()
	if failure := harness.producer.BeginTransaction(); failure != nil {
		harness.test.Fatal(failure)
	}
	for _, entry := range entries {
		encoded, _ := json.Marshal(entry)
		if failure := harness.validator.Validate(kafka.ContractLedger, encoded); failure != nil {
			harness.test.Fatalf("a fixture ledger entry fails its contract: %v\n%s", failure, encoded)
		}
		record := &kgo.Record{Topic: harness.settings.Kafka.Topics.Ledger, Partition: partition, Key: []byte(entry.Instance), Value: encoded}
		if failure := harness.producer.ProduceSync(operation, record).FirstErr(); failure != nil {
			harness.test.Fatal(failure)
		}
	}
	if failure := harness.producer.EndTransaction(operation, kgo.TryCommit); failure != nil {
		harness.test.Fatal(failure)
	}
}

func (harness *reconcileHarness) produceResult(pid campaign.Pid, actionKind, status string, delivered bool, at time.Time) {
	harness.test.Helper()
	stamp := kafka.FormatTime(at)
	var finished any = stamp
	if status == "started" {
		finished = nil
	}
	message := map[string]any{
		"schema": "dusk.process-results/v1", "id": kafka.NewMessageID(), "time": stamp, "pid": pid.String(), "campaign_id": nil, "attempt": nil,
		"device_id": fixtureDevice, "installation_id": fixtureInstallation, "namespace_id": fixtureNamespace,
		"action_kind": actionKind, "status": status, "delivered": delivered, "error": nil, "started_at": stamp, "finished_at": finished,
		"dawn_instance": "dawn-0", "output_digest": strings.Repeat("ab", 32), "output_count": 0, "output_truncated": false, "reported": nil,
	}
	encoded, _ := json.Marshal(message)
	if failure := harness.validator.Validate(kafka.ContractProcessResults, encoded); failure != nil {
		harness.test.Fatalf("a fixture process result fails its contract: %v", failure)
	}
	record := &kgo.Record{Topic: harness.settings.Kafka.Topics.ProcessResults, Key: []byte(fixtureDevice + "/" + fixtureInstallation), Value: encoded}
	if failure := harness.producer.BeginTransaction(); failure != nil {
		harness.test.Fatal(failure)
	}
	if failure := harness.producer.ProduceSync(context.Background(), record).FirstErr(); failure != nil {
		harness.test.Fatal(failure)
	}
	if failure := harness.producer.EndTransaction(context.Background(), kgo.TryCommit); failure != nil {
		harness.test.Fatal(failure)
	}
}

func (harness *reconcileHarness) intend(pid campaign.Pid, device, kind string, maxCommands, defaultShellCommands int, expires, dispatched time.Time) {
	harness.test.Helper()
	if _, failure := harness.pool.Exec(context.Background(), `insert into intended_processes (pid, created_at, device_id, installation_id, action_kind, principal, subject,
			expires_at, max_commands, default_shell_commands, last_dispatched_at)
		values ($1::text::numeric, $7, $2, $3, $4, 'operator', 'operator', $5, $6, $8, $7)`,
		pid.String(), device, fixtureInstallation, kind, expires, maxCommands, dispatched, defaultShellCommands); failure != nil {
		harness.test.Fatal(failure)
	}
}

func (harness *reconcileHarness) openAlerts() map[string][]alerts.Alert {
	harness.test.Helper()
	listed, failure := testsupport.AlertStore(harness.test, harness.pool).List(context.Background(), true, "", 1000)
	if failure != nil {
		harness.test.Fatal(failure)
	}
	byKind := map[string][]alerts.Alert{}
	for _, alert := range listed.Alerts {
		byKind[alert.Kind] = append(byKind[alert.Kind], alert)
	}
	return byKind
}

func eventually(test *testing.T, what string, timeout time.Duration, check func() (bool, string)) {
	test.Helper()
	deadline := time.Now().Add(timeout)
	last := ""
	for time.Now().Before(deadline) {
		done, detail := check()
		if done {
			return
		}
		last = detail
		time.Sleep(250 * time.Millisecond)
	}
	test.Fatalf("%s did not happen within %s; last: %s", what, timeout, last)
}

func TestReconcileRaisesEveryRuleFromTheLedgerAndResults(test *testing.T) {
	private, _ := signingKey(7)
	harness := newReconcileHarness(test, keySet(test, private))
	now := time.Now().UTC()
	reused, unknown, elsewhere, expired, quiet, other := campaign.Pid(65601), campaign.Pid(65602), campaign.Pid(65603), campaign.Pid(65604), campaign.Pid(65605), campaign.Pid(65606)
	harness.intend(reused, fixtureDevice, "ensure_version", 8, 2, now.Add(time.Hour), now.Add(-10*time.Minute))
	harness.intend(elsewhere, "00000000000000000000000000000abc", "run_script", 8, 3, now.Add(time.Hour), now.Add(-10*time.Minute))
	harness.intend(expired, fixtureDevice, "run_script", 8, 3, now.Add(-time.Hour), now.Add(-2*time.Hour))
	harness.intend(quiet, fixtureDevice, "collect_facts", 8, 0, now.Add(time.Hour), now.Add(-time.Minute))
	harness.intend(other, fixtureDevice, "run_script", 8, 0, now.Add(time.Hour), now.Add(-time.Minute))

	first := newWriter("nightfall-0", 0, now.Add(-time.Minute), private)
	var entries []kafka.LedgerEntry
	add := func(entry kafka.LedgerEntry) {
		sealed, _ := first.seal(entry)
		entries = append(entries, sealed)
	}
	add(first.call("dawn-0", reused.String(), fixtureSession, "Dusk.process", 3))
	for range 9 {
		add(first.call("dawn-0", reused.String(), fixtureSession, "ShPortal.sh", 7))
	}
	checkpoint, _ := first.checkpoint()
	entries = append(entries, checkpoint)
	add(first.call("dawn-1", reused.String(), "0192f3a4-0000-7c6b-8a59-483726150f1e", "Dusk.process", 3))
	add(first.call("dawn-0", "0", fixtureSession, "Dusk.process", 0))
	add(first.call("oncall-7", "0", "0192f3a4-1111-7c6b-8a59-483726150f1e", "Dusk.process", 0))
	add(first.call("dawn-0", "0", fixtureSession, "Dusk.ps", 0))
	add(first.call("dawn-0", campaign.DefaultShellPid.String(), fixtureSession, "Dusk.process", 2))
	add(first.call("dawn-1", campaign.DefaultShellPid.String(), "0192f3a4-2222-7c6b-8a59-483726150f1e", "Dusk.process", 2))
	for range 3 {
		add(first.call("dawn-0", campaign.DefaultShellPid.String(), fixtureSession, "ShPortal.sh", 5))
	}
	unintended := first.call("dawn-0", campaign.DefaultShellPid.String(), fixtureSession, "ShPortal.sh", 5)
	unintended.DeviceID = text(strangerDevice)
	add(unintended)
	add(first.call("dawn-0", unknown.String(), fixtureSession, "Dusk.process", 3))
	add(first.call("dawn-0", unknown.String(), fixtureSession, "ShPortal.sh", 7))
	add(first.call("dawn-0", elsewhere.String(), fixtureSession, "Dusk.process", 3))
	add(first.call("dawn-0", expired.String(), fixtureSession, "ShPortal.sh", 3))
	add(first.call("dawn-0", expired.String(), fixtureSession, "Dusk.kill", 3))
	add(first.call("dawn-0", quiet.String(), fixtureSession, "Dusk.hostname", 3))
	override := first.event("quarantine_override", map[string]any{"role": "incident-response"})
	override.DeviceID, override.InstallationID, override.NamespaceID, override.Epoch = text(fixtureDevice), text(fixtureInstallation), text(fixtureNamespace), number(fixtureEpoch)
	override.Principal, override.Pid = "responder-0", quiet.String()
	override.SessionID, override.CallID = text(fixtureSession), text(uuid.NewString())
	override.CapID, override.ParentCapID, override.Direction, override.Action = number(4), number(2), text("client_to_node"), text("ShPortal.sh")
	override.InterfaceID, override.MethodID = text("e1c5b0f3a7d29c48"), number(0)
	add(override)
	checkpoint, _ = first.checkpoint()
	entries = append(entries, checkpoint)
	harness.produceLedger(0, entries...)

	second := newWriter("nightfall-1", 1, now.Add(-time.Minute), private)
	var tampered []kafka.LedgerEntry
	for range 3 {
		sealed, _ := second.seal(second.call("dawn-0", other.String(), fixtureSession, "Dusk.hostname", 3))
		tampered = append(tampered, sealed)
	}
	tampered[1].Principal = "mallory"
	checkpoint, _ = second.checkpoint()
	harness.produceLedger(1, append(tampered, checkpoint)...)

	harness.produceResult(reused, "ensure_version", "succeeded", true, now.Add(-5*time.Minute))
	unledgered := campaign.Pid(65607)
	harness.intend(unledgered, fixtureDevice, "ensure_version", 8, 0, now.Add(time.Hour), now.Add(-30*time.Minute))
	harness.produceResult(unledgered, "ensure_version", "started", true, now.Add(-20*time.Minute))
	harness.produceResult(reused, "reap", "reaped", false, now)

	harness.start()
	want := []string{
		alerts.KindProcessWithoutIntent, alerts.KindTargetMismatch, alerts.KindProcessAfterDeadline, alerts.KindPidReused,
		alerts.KindProcessShape, alerts.KindProcessAfterResult, alerts.KindQuarantineOverride, alerts.KindLedgerChainBroken,
		alerts.KindDefaultShellWithoutIntent,
	}
	eventually(test, "every ledger rule raised its alert", time.Minute, func() (bool, string) {
		open := harness.openAlerts()
		var missing []string
		for _, kind := range want {
			if len(open[kind]) == 0 {
				missing = append(missing, kind)
			}
		}
		return len(missing) == 0 && len(open[alerts.KindProcessShape]) == 2, "missing " + strings.Join(missing, ", ")
	})

	open := harness.openAlerts()
	if counted := unattributed(test, "dawn-0"); counted != 1 {
		test.Errorf("dawn-0 created %v processes without a pid; Dusk.ps under no process is not one", counted)
	}
	if counted := unattributed(test, "oncall-7"); counted != 1 {
		test.Errorf("oncall-7 created %v processes without a pid", counted)
	}
	for _, alert := range open[alerts.KindPidReused] {
		if strings.HasSuffix(alert.Fingerprint, campaign.DefaultShellPid.String()) {
			test.Errorf("the default shell on two sessions raised %s", alert.Fingerprint)
		}
	}
	if found := open[alerts.KindProcessWithoutIntent]; len(found) != 1 || found[0].Fingerprint != "process_without_intent:"+unknown.String() {
		test.Errorf("process_without_intent: %d alerts; the shell command under the unintended pid must not raise its own", len(found))
	}
	if found := open[alerts.KindProcessAfterDeadline]; len(found) != 1 || found[0].Fingerprint != "process_after_deadline:"+expired.String() {
		test.Errorf("process_after_deadline: %v", found)
	}
	if broken := open[alerts.KindLedgerChainBroken]; len(broken) != 1 || !strings.Contains(broken[0].Fingerprint, "nightfall-1/1/hash_mismatch") {
		var fingerprints []string
		for _, alert := range broken {
			fingerprints = append(fingerprints, alert.Fingerprint)
		}
		test.Errorf("chain breaks: %v", fingerprints)
	}
	var detail map[string]any
	_ = json.Unmarshal(open[alerts.KindTargetMismatch][0].Detail, &detail)
	if detail["pid"] != elsewhere.String() || detail["intended_device_id"] != "00000000000000000000000000000abc" || detail["device_id"] != fixtureDevice {
		test.Errorf("target mismatch detail %v", detail)
	}
	if found := open[alerts.KindDefaultShellWithoutIntent]; len(found) != 1 || found[0].Fingerprint != "default_shell_without_intent:"+strangerDevice+"/"+fixtureInstallation+"/"+fixtureSession {
		test.Errorf("default_shell_without_intent: %v; only the node with no open intended process raises it", found)
	}
	shapes := map[string]map[string]any{}
	for _, alert := range open[alerts.KindProcessShape] {
		var shape map[string]any
		_ = json.Unmarshal(alert.Detail, &shape)
		shapes[fmt.Sprint(shape["pid"])] = shape
	}
	if shape := shapes[campaign.DefaultShellPid.String()]; shape == nil || shape["commands"] != 3.0 || shape["max_commands"] != 2.0 || shape["device_id"] != fixtureDevice {
		test.Errorf("the default shell's window: %v", shape)
	}
	if shape := shapes[reused.String()]; shape == nil || shape["commands"] != 9.0 {
		test.Errorf("the reused pid's own shell: %v", shape)
	}
	for _, kind := range []string{alerts.KindProcessWithoutIntent, alerts.KindTargetMismatch, alerts.KindLedgerChainBroken, alerts.KindDefaultShellWithoutIntent} {
		if open[kind][0].Severity != alerts.Critical {
			test.Errorf("%s is %s, not critical", kind, open[kind][0].Severity)
		}
	}
	for _, kind := range []string{alerts.KindPidReused, alerts.KindProcessShape, alerts.KindProcessAfterResult, alerts.KindProcessAfterDeadline} {
		if open[kind][0].Severity != alerts.High {
			test.Errorf("%s is %s, not high", kind, open[kind][0].Severity)
		}
	}

	eventually(test, "every ledger partition caught up", 30*time.Second, func() (bool, string) {
		var behind int
		if failure := harness.pool.QueryRow(context.Background(), `select count(*) from reconcile_offsets where processed_at is null or processed_at < now() - interval '1 minute'`).Scan(&behind); failure != nil {
			return false, failure.Error()
		}
		var partitions int
		_ = harness.pool.QueryRow(context.Background(), `select count(*) from reconcile_offsets`).Scan(&partitions)
		return behind == 0 && partitions == 3, "partitions behind or missing"
	})
	reconciler := New(Dependencies{Config: harness.settings, Pool: harness.pool, Alerts: testsupport.AlertStore(test, harness.pool), Logger: testsupport.Logger()})
	eventually(test, "the delivered result with no Dusk.process in the ledger was found", 30*time.Second, func() (bool, string) {
		reconciler.checkLedger(context.Background())
		found := harness.openAlerts()[alerts.KindResultWithoutLedger]
		return len(found) == 1 && strings.HasSuffix(found[0].Fingerprint, unledgered.String()), "not yet"
	})
	if found := harness.openAlerts()[alerts.KindResultWithoutLedger]; len(found) != 1 {
		test.Errorf("result_without_ledger raised %d times; a pid the ledger shows created must not raise it", len(found))
	}

	var head struct {
		sequence   int64
		checkpoint *int64
	}
	if failure := harness.pool.QueryRow(context.Background(), `select sequence, checkpoint_sequence from ledger_chain_heads where instance = 'nightfall-0' and partition = 0`).
		Scan(&head.sequence, &head.checkpoint); failure != nil {
		test.Fatal(failure)
	}
	if head.sequence != first.sequence-1 || head.checkpoint == nil || *head.checkpoint != head.sequence {
		test.Errorf("chain head %d (checkpoint %v), want %d checkpointed", head.sequence, head.checkpoint, first.sequence-1)
	}

	harness.stop()
	var resumed []kafka.LedgerEntry
	for range 3 {
		sealed, _ := first.seal(first.call("dawn-0", quiet.String(), fixtureSession, "Dusk.hostname", 3))
		resumed = append(resumed, sealed)
	}
	checkpoint, _ = first.checkpoint()
	harness.produceLedger(0, append(resumed, checkpoint)...)
	harness.start()
	eventually(test, "a restarted reconcile continued the chain", 30*time.Second, func() (bool, string) {
		var sequence int64
		if failure := harness.pool.QueryRow(context.Background(), `select sequence from ledger_chain_heads where instance = 'nightfall-0' and partition = 0`).Scan(&sequence); failure != nil {
			return false, failure.Error()
		}
		return sequence == first.sequence-1, "head not advanced"
	})
	open = harness.openAlerts()
	var fingerprints []string
	for _, alert := range open[alerts.KindLedgerChainBroken] {
		fingerprints = append(fingerprints, alert.Fingerprint)
	}
	sort.Strings(fingerprints)
	if len(fingerprints) != 1 {
		test.Errorf("a restart must neither re-read nor break the chain: %v", fingerprints)
	}
	var occurrences int64
	_ = harness.pool.QueryRow(context.Background(), `select occurrences from alerts where kind = 'process_without_intent'`).Scan(&occurrences)
	if occurrences != 1 {
		test.Errorf("process_without_intent occurred %d times; a restart re-applied entries", occurrences)
	}
	var counted int
	_ = harness.pool.QueryRow(context.Background(), `select sum(commands) from reconcile_default_shell where device_id = $1`, fixtureDevice).Scan(&counted)
	if counted != 3 {
		test.Errorf("the default shell's windows count %d commands, want 3 counted once", counted)
	}
}

func unattributed(test *testing.T, principal string) float64 {
	test.Helper()
	families, failure := prometheus.DefaultGatherer.Gather()
	if failure != nil {
		test.Fatal(failure)
	}
	for _, family := range families {
		if family.GetName() != "twilight_unattributed_processes_total" {
			continue
		}
		for _, metric := range family.GetMetric() {
			for _, pair := range metric.GetLabel() {
				if pair.GetName() == "principal" && pair.GetValue() == principal {
					return metric.GetCounter().GetValue()
				}
			}
		}
	}
	return 0
}
