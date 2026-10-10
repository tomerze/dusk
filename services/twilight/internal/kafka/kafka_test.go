package kafka

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5/pgconn"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
)

const repositoryContracts = "../../../contracts/kafka"

func validator(test *testing.T) *Validator {
	test.Helper()
	loaded, failure := NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	return loaded
}

func TestEmbeddedContractsMatchTheRepository(test *testing.T) {
	entries, failure := contracts.ReadDir("contracts")
	if failure != nil {
		test.Fatal(failure)
	}
	repository, failure := filepath.Glob(filepath.Join(repositoryContracts, "*.schema.json"))
	if failure != nil || len(repository) == 0 {
		test.Fatalf("no contracts under %s (%v)", repositoryContracts, failure)
	}
	if len(repository) != len(entries) {
		test.Fatalf("%d embedded contracts, %d in the repository; copy services/contracts/kafka/*.schema.json into internal/kafka/contracts", len(entries), len(repository))
	}
	for _, entry := range entries {
		embedded, _ := contracts.ReadFile("contracts/" + entry.Name())
		original, failure := os.ReadFile(filepath.Join(repositoryContracts, entry.Name()))
		if failure != nil {
			test.Fatalf("%s: %v", entry.Name(), failure)
		}
		if !bytes.Equal(embedded, original) {
			test.Errorf("internal/kafka/contracts/%s differs from services/contracts/kafka/%s", entry.Name(), entry.Name())
		}
	}
}

func TestContractExamples(test *testing.T) {
	if checked := checkExamples(test, filepath.Join(repositoryContracts, "examples")); checked < 50 {
		test.Fatalf("only %d examples checked", checked)
	}
}

func checkExamples(test *testing.T, root string) int {
	test.Helper()
	loaded := validator(test)
	directories, failure := filepath.Glob(filepath.Join(root, "*"))
	if failure != nil || len(directories) == 0 {
		test.Fatalf("no examples under %s (%v)", root, failure)
	}
	checked := 0
	for _, directory := range directories {
		contract := filepath.Base(directory)
		files, _ := filepath.Glob(filepath.Join(directory, "*.json"))
		for _, file := range files {
			content, failure := os.ReadFile(file)
			if failure != nil {
				test.Fatal(failure)
			}
			failure = loaded.Validate(contract, content)
			invalid := strings.HasPrefix(filepath.Base(file), "invalid-")
			if invalid && failure == nil {
				test.Errorf("%s/%s passed its contract", contract, filepath.Base(file))
			}
			if !invalid && failure != nil {
				test.Errorf("%s/%s: %v", contract, filepath.Base(file), failure)
			}
			checked++
		}
	}
	return checked
}

func TestDecodeDropsInvalidRecords(test *testing.T) {
	loaded := validator(test)
	logger := slog.New(slog.NewJSONHandler(io.Discard, nil))
	valid, _ := os.ReadFile(filepath.Join(repositoryContracts, "examples", "dusk.connections", "connected.json"))
	record := &kgo.Record{Topic: "dusk.connections", Value: valid}
	connection, accepted := Decode[Connection](loaded, ContractConnections, record, logger)
	if !accepted || connection.Event != "connected" || connection.Epoch == 0 || len(connection.NamespaceID) != 16 {
		test.Fatalf("valid connection: %+v %v", connection, accepted)
	}
	broken := bytes.Replace(valid, []byte(`"connected"`), []byte(`"appeared"`), 1)
	if _, accepted := Decode[Connection](loaded, ContractConnections, &kgo.Record{Topic: "dusk.connections", Value: broken}, logger); accepted {
		test.Fatal("invalid connection accepted")
	}
	if _, accepted := Decode[Connection](loaded, ContractConnections, &kgo.Record{Topic: "dusk.connections", Value: []byte("{")}, logger); accepted {
		test.Fatal("malformed JSON accepted")
	}
	var fields map[string]any
	_ = json.Unmarshal(valid, &fields)
	fields["time"] = "2026-02-30T00:00:00.000000000Z"
	impossible, _ := json.Marshal(fields)
	if _, accepted := Decode[Connection](loaded, ContractConnections, &kgo.Record{Topic: "dusk.connections", Value: impossible}, logger); accepted {
		test.Fatal("a time that matches the pattern but names no real day was accepted")
	}
	if failure := loaded.Validate("dusk.unknown", valid); failure == nil {
		test.Fatal("validated against an unknown contract")
	}
}

func TestDecodedMessagesKeepEveryField(test *testing.T) {
	loaded := validator(test)
	logger := slog.New(slog.NewJSONHandler(io.Discard, nil))
	cases := map[string]func(*kgo.Record) (any, bool){
		"dusk.census/header-full.json": func(record *kgo.Record) (any, bool) {
			return Decode[CensusRecord](loaded, ContractCensus, record, logger)
		},
		"dusk.census/chunk-0.json": func(record *kgo.Record) (any, bool) {
			return Decode[CensusRecord](loaded, ContractCensus, record, logger)
		},
		"dusk.enrollments/enroll-issued.json": func(record *kgo.Record) (any, bool) {
			return Decode[Enrollment](loaded, ContractEnrollments, record, logger)
		},
		"dusk.node-state/installation-quarantined.json": func(record *kgo.Record) (any, bool) {
			return Decode[NodeState](loaded, ContractNodeState, record, logger)
		},
		"dusk.process-results/ensure-version-succeeded.json": func(record *kgo.Record) (any, bool) {
			return Decode[ProcessResult](loaded, ContractProcessResults, record, logger)
		},
		"dusk.process-results/collect-facts-succeeded.json": func(record *kgo.Record) (any, bool) {
			return Decode[ProcessResult](loaded, ContractProcessResults, record, logger)
		},
		"dusk.ledger/call-client-to-node.json": func(record *kgo.Record) (any, bool) {
			return Decode[LedgerEntry](loaded, ContractLedger, record, logger)
		},
		"dusk.ledger/checkpoint.json": func(record *kgo.Record) (any, bool) {
			return Decode[LedgerEntry](loaded, ContractLedger, record, logger)
		},
	}
	for name, decode := range cases {
		content, failure := os.ReadFile(filepath.Join(repositoryContracts, "examples", name))
		if failure != nil {
			test.Fatal(failure)
		}
		decoded, accepted := decode(&kgo.Record{Value: content})
		if !accepted {
			test.Errorf("%s rejected", name)
			continue
		}
		reencoded, _ := json.Marshal(decoded)
		var original, roundTripped map[string]any
		_ = json.Unmarshal(content, &original)
		_ = json.Unmarshal(reencoded, &roundTripped)
		for key, value := range original {
			got, present := roundTripped[key]
			if !present {
				test.Errorf("%s: field %s lost in decoding", name, key)
				continue
			}
			want, _ := json.Marshal(value)
			have, _ := json.Marshal(got)
			if !bytes.Equal(want, have) {
				test.Errorf("%s: field %s = %s, want %s", name, key, have, want)
			}
		}
	}
}

func TestNodeStateMessagesPassTheirContract(test *testing.T) {
	loaded := validator(test)
	reason := "quarantine campaign"
	state := NewInstallationState("3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", "quarantined", &reason,
		"campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd", time.Date(2026, 10, 7, 12, 0, 0, 5, time.UTC))
	value, _ := json.Marshal(state)
	if failure := loaded.Validate(ContractNodeState, value); failure != nil {
		test.Fatalf("%s: %v", value, failure)
	}
	if state.Key() != "installation/3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70" {
		test.Fatalf("key %s", state.Key())
	}
	if state.Time != "2026-10-07T12:00:00.000000005Z" {
		test.Fatalf("time %s", state.Time)
	}
	parsed, failure := ParseTime(state.Time)
	if failure != nil || !parsed.Equal(time.Date(2026, 10, 7, 12, 0, 0, 5, time.UTC)) {
		test.Fatalf("parse %v %v", parsed, failure)
	}
}

func TestRequiredTopicsCoverEveryTopicTwilightUses(test *testing.T) {
	seen := map[string]string{}
	for _, requirement := range RequiredTopics(configTopics()) {
		seen[requirement.Topic] = requirement.CleanupPolicy
	}
	if len(seen) != 7 || seen["dusk.census"] != "compact" || seen["dusk.node-state"] != "compact" || seen["dusk.ledger"] != "delete" ||
		seen["dusk.intended-processes"] != "compact" {
		test.Fatalf("requirements %v", seen)
	}
}

func TestIntendedProcessMessagesPassTheirContract(test *testing.T) {
	loaded := validator(test)
	campaignID := "0192f3a4-5b6c-7d8e-9f01-23456789abcd"
	intended := IntendedProcess{
		Envelope: Envelope{Schema: "dusk.intended-processes/v1", ID: NewMessageID(), Time: FormatTime(time.Date(2026, 10, 7, 12, 0, 0, 5, time.UTC))},
		Pid:      "13792273858822192861", DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70",
		CampaignID: &campaignID, ActionKind: "run_script", Principal: "token:0192f3a4-1111-7d8e-9f01-23456789abcd", Subject: "campaign:" + campaignID,
		CreatedAt: FormatTime(time.Date(2026, 10, 7, 12, 0, 0, 0, time.UTC)), ExpiresAt: FormatTime(time.Date(2026, 10, 7, 12, 6, 0, 0, time.UTC)),
		MaxCommands: 1, DefaultShellCommands: 3,
	}
	value, _ := json.Marshal(intended)
	if failure := loaded.Validate(ContractIntended, value); failure != nil {
		test.Fatalf("%s: %v", value, failure)
	}
	if intended.Key() != "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70/13792273858822192861" {
		test.Fatalf("key %s", intended.Key())
	}
	reap := intended
	reap.CampaignID, reap.ActionKind, reap.Subject, reap.MaxCommands = nil, "reap", "twilight", 0
	value, _ = json.Marshal(reap)
	if failure := loaded.Validate(ContractIntended, value); failure != nil {
		test.Fatalf("a reap's intended process: %s: %v", value, failure)
	}
	for _, broken := range []func(*IntendedProcess){
		func(message *IntendedProcess) { message.Pid = "0" },
		func(message *IntendedProcess) { message.Pid = "17505437192229758416" },
		func(message *IntendedProcess) { message.Subject = "" },
		func(message *IntendedProcess) { message.MaxCommands = -1 },
	} {
		copied := intended
		broken(&copied)
		value, _ := json.Marshal(copied)
		if loaded.Validate(ContractIntended, value) == nil {
			test.Errorf("%s passed its contract", value)
		}
	}
}

func configTopics() config.Topics {
	return config.Default().Kafka.Topics
}

func TestOnlyAppliedRecordsAreCommitted(test *testing.T) {
	logger := slog.New(slog.NewJSONHandler(io.Discard, nil))
	partition := func(number int32, offsets ...int64) kgo.FetchPartition {
		fetched := kgo.FetchPartition{Partition: number, HighWatermark: 100}
		for _, offset := range offsets {
			fetched.Records = append(fetched.Records, &kgo.Record{Topic: "dusk.process-results", Partition: number, Offset: offset})
		}
		return fetched
	}
	fetches := kgo.Fetches{{Topics: []kgo.FetchTopic{{Topic: "dusk.process-results", Partitions: []kgo.FetchPartition{partition(0, 10, 11, 12), partition(1, 20, 21)}}}}}
	failures := map[int64]int{11: 1}
	var mutex sync.Mutex
	handle := func(operation context.Context, record *kgo.Record) error {
		mutex.Lock()
		defer mutex.Unlock()
		if failures[record.Offset] > 0 {
			failures[record.Offset]--
			return errors.New("the database is away")
		}
		return nil
	}
	applied := applyPartitions(context.Background(), "test", logger, fetches, handle)
	committed := map[int32]int64{}
	for _, record := range applied {
		committed[record.Partition] = record.Offset
	}
	if len(committed) != 2 || committed[0] != 12 || committed[1] != 21 {
		test.Fatalf("a record that failed once was retried in place, so every record is applied: %v", committed)
	}
	operation, cancel := context.WithCancel(context.Background())
	stuck := func(_ context.Context, record *kgo.Record) error {
		if record.Offset == 11 {
			cancel()
			return errors.New("this instance no longer leads")
		}
		return nil
	}
	applied = applyPartitions(operation, "test", logger, kgo.Fetches{{Topics: []kgo.FetchTopic{{Topic: "dusk.process-results", Partitions: []kgo.FetchPartition{partition(0, 10, 11, 12)}}}}}, stuck)
	if len(applied) != 1 || applied[0].Offset != 10 {
		test.Fatalf("a record that never applied before the consumer stopped must stay uncommitted with everything after it: %v", applied)
	}
}

func TestDecodedStringsCarryNoNul(test *testing.T) {
	loaded := validator(test)
	logger := slog.New(slog.NewJSONHandler(io.Discard, nil))
	content, failure := os.ReadFile(filepath.Join(repositoryContracts, "examples", "dusk.process-results", "collect-facts-succeeded.json"))
	if failure != nil {
		test.Fatal(failure)
	}
	poisoned := bytes.Replace(content, []byte(`"de_DE.UTF-8"`), []byte(`"de\u0000DE", "dusk.hostname\u0000": {"nested": ["a\u0000b", "\ud800"]}`), 1)
	poisoned = bytes.Replace(poisoned, []byte(`"logs"`), []byte(`"lo\u0000gs"`), 1)
	result, accepted := Decode[ProcessResult](loaded, ContractProcessResults, &kgo.Record{Topic: "dusk.process-results", Value: poisoned}, logger)
	if !accepted || result.Reported == nil {
		test.Fatalf("a result whose strings hold U+0000 was dropped: %+v", result)
	}
	if string(result.Reported.Facts["dusk.os.locale"]) != `"de�DE"` || result.Reported.Services[1] != "lo�gs" {
		test.Fatalf("U+0000 survived decoding: %s %q", result.Reported.Facts["dusk.os.locale"], result.Reported.Services)
	}
	if nested := string(result.Reported.Facts["dusk.hostname�"]); nested != `{"nested":["a�b","�"]}` {
		test.Fatalf("a fact key or a nested value kept U+0000 or a lone surrogate: %s %v", nested, result.Reported.Facts)
	}
	if string(result.Reported.Facts["dusk.device.memory_bytes"]) != "8589934592" {
		test.Fatalf("cleaning lost a number's digits: %s", result.Reported.Facts["dusk.device.memory_bytes"])
	}
	if clean := database.Storable(content); !bytes.Equal(clean, content) {
		test.Fatal("a record without U+0000 was re-encoded")
	}
}

func TestRecordsTheDatabaseRefusesAreSkipped(test *testing.T) {
	logger := slog.New(slog.NewJSONHandler(io.Discard, nil))
	fetched := kgo.FetchPartition{Partition: 0, HighWatermark: 100}
	for _, offset := range []int64{10, 11, 12} {
		fetched.Records = append(fetched.Records, &kgo.Record{Topic: "dusk.process-results", Offset: offset})
	}
	var handled []int64
	handle := func(_ context.Context, record *kgo.Record) error {
		handled = append(handled, record.Offset)
		if record.Offset == 11 {
			return fmt.Errorf("store the reported state: %w", &pgconn.PgError{Code: "22P05", Message: "unsupported Unicode escape sequence"})
		}
		return nil
	}
	operation, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	applied := applyPartitions(operation, "test", logger, kgo.Fetches{{Topics: []kgo.FetchTopic{{Topic: "dusk.process-results", Partitions: []kgo.FetchPartition{fetched}}}}}, handle)
	if len(applied) != 1 || applied[0].Offset != 12 || len(handled) != 3 {
		test.Fatalf("a record the database refuses as data blocked its partition: applied %v, handled %v", applied, handled)
	}
}
