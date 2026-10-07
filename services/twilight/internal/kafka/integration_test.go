//go:build integration

package kafka_test

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func client(test *testing.T, extra ...kgo.Opt) *kgo.Client {
	test.Helper()
	settings := config.Default().Kafka
	settings.Brokers = []string{testsupport.Brokers(test)}
	settings.AllowPlaintext = true
	options, failure := kafka.Options(settings, "test")
	if failure != nil {
		test.Fatal(failure)
	}
	created, failure := kgo.NewClient(append(options, extra...)...)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(created.Close)
	return created
}

func TestNodeStateRoundTripAndTopicCheck(test *testing.T) {
	operation, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	producerClient := client(test)
	admin := kadm.NewClient(producerClient)
	suffix := strings.ReplaceAll(test.Name(), "/", "-")
	nodeStateTopic := "dusk.node-state." + suffix
	ledgerTopic := "dusk.ledger." + suffix
	if _, failure := admin.CreateTopic(operation, 1, 1, map[string]*string{"cleanup.policy": new("compact")}, nodeStateTopic); failure != nil {
		test.Fatal(failure)
	}
	requirements := []kafka.TopicRequirement{{Topic: nodeStateTopic, CleanupPolicy: "compact"}, {Topic: ledgerTopic, CleanupPolicy: "delete"}}
	if failure := kafka.CheckTopics(operation, producerClient, requirements); failure == nil || !strings.Contains(failure.Error(), ledgerTopic+" is missing") {
		test.Fatalf("missing topic not reported: %v", failure)
	}
	if _, failure := admin.CreateTopic(operation, 3, 1, map[string]*string{"cleanup.policy": new("compact")}, ledgerTopic); failure != nil {
		test.Fatal(failure)
	}
	if failure := eventually(func() error { return kafka.CheckTopics(operation, producerClient, requirements) }, `cleanup.policy "compact", want "delete"`); failure != nil {
		test.Fatalf("wrong cleanup policy not reported: %v", failure)
	}
	requirements[1].CleanupPolicy = "compact"
	if failure := eventually(func() error { return kafka.CheckTopics(operation, producerClient, requirements) }, ""); failure != nil {
		test.Fatalf("topics as required: %v", failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	reason := "quarantine campaign"
	state := kafka.NewInstallationState("3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", "quarantined", &reason, "campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd", time.Now())
	producer := kafka.NewNodeStateProducer(producerClient, validator, nodeStateTopic)
	if failure := producer.ProduceNodeState(operation, state); failure != nil {
		test.Fatal(failure)
	}
	consumer := client(test, kgo.ConsumeTopics(nodeStateTopic), kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))
	fetches := consumer.PollRecords(operation, 1)
	if fetchErrors := fetches.Errors(); len(fetchErrors) > 0 {
		test.Fatalf("poll: %v", fetchErrors)
	}
	records := fetches.Records()
	if len(records) != 1 {
		test.Fatalf("%d records", len(records))
	}
	if string(records[0].Key) != state.Key() {
		test.Fatalf("key %s", records[0].Key)
	}
	if failure := validator.Validate(kafka.ContractNodeState, records[0].Value); failure != nil {
		test.Fatalf("consumed node-state fails its contract: %v", failure)
	}
}

func eventually(check func() error, want string) error {
	deadline := time.Now().Add(15 * time.Second)
	for {
		failure := check()
		matched := (want == "" && failure == nil) || (want != "" && failure != nil && strings.Contains(failure.Error(), want))
		if matched {
			return nil
		}
		if time.Now().After(deadline) {
			if failure == nil {
				return errors.New("no error reported")
			}
			return failure
		}
		time.Sleep(200 * time.Millisecond)
	}
}
