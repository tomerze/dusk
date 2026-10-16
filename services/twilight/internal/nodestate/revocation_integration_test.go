//go:build integration

package nodestate

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/twmb/franz-go/pkg/kadm"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
)

func TestAResolveDuringARevocationByCredentialWaitsAndPublishesTheRevocation(test *testing.T) {
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	pool, _ := testsupport.Database(test)
	settings := config.Default().Kafka
	settings.Brokers = []string{testsupport.Brokers(test)}
	settings.AllowPlaintext = true
	options, failure := kafka.Options(settings, "test")
	if failure != nil {
		test.Fatal(failure)
	}
	client, failure := kgo.NewClient(options...)
	if failure != nil {
		test.Fatal(failure)
	}
	defer client.Close()
	topic := "dusk.node-state." + strings.ReplaceAll(uuid.NewString()[:8], "-", "")
	if _, failure := kadm.NewClient(client).CreateTopic(operation, 1, 1, map[string]*string{"cleanup.policy": new("compact")}, topic); failure != nil {
		test.Fatal(failure)
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		test.Fatal(failure)
	}
	producer := kafka.NewNodeStateProducer(client, validator, topic)
	store := inventory.NewStore(pool)
	key := inventory.Key{DeviceID: "40000000000000000000000000000001", InstallationID: "a0000000000000000000000000000001"}
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id) values ($1, $2)`, key.DeviceID, key.InstallationID); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := store.SetLifecycle(operation, key, "quarantined", "campaign", "campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd", time.Now().Add(-time.Hour), func(string) error { return nil }); failure != nil {
		test.Fatal(failure)
	}
	revocation, failure := store.CreateRevocation(operation, inventory.RevocationScope{CredentialKind: "fleet_token", Credential: "leaked"}, "leaked token", "admin@example.org", time.Now())
	if failure != nil {
		test.Fatal(failure)
	}
	rebuild := Rebuild{Options: options, Producer: producer, Inventory: store, Validator: validator, Topic: topic, Actor: "rebuild-test", Workers: 1, Logger: testsupport.Logger()}
	published := make(chan struct{})
	proceed := make(chan struct{})
	revoked := make(chan error, 1)
	go func() {
		revoked <- store.RevokeBatch(operation, &revocation, []inventory.Key{key}, 0, func(revoking []inventory.Key, at time.Time) error {
			reason := "leaked token"
			if failure := producer.ProduceNodeState(operation, kafka.NewInstallationState(revoking[0].DeviceID, revoking[0].InstallationID, "revoked", &reason, "admin@example.org", at)); failure != nil {
				return failure
			}
			close(published)
			<-proceed
			return nil
		})
	}()
	<-published
	resolved := make(chan error, 1)
	counts := &tally{report: &Report{}}
	go func() {
		resolved <- rebuild.resolve(operation, Key(key.DeviceID, key.InstallationID), nil, counts)
	}()
	select {
	case failure := <-resolved:
		test.Fatalf("the rebuild resolved the node while its revocation held it: %v", failure)
	case <-time.After(500 * time.Millisecond):
	}
	close(proceed)
	if failure := <-revoked; failure != nil {
		test.Fatal(failure)
	}
	if failure := <-resolved; failure != nil {
		test.Fatal(failure)
	}
	snapshot, failure := Read(operation, options, topic, validator, testsupport.Logger(), true, time.Minute)
	if failure != nil {
		test.Fatal(failure)
	}
	if entry := snapshot.States[Key(key.DeviceID, key.InstallationID)]; entry == nil || entry.Lifecycle() != "revoked" {
		test.Fatalf("the topic ends with %+v, not the revocation", entry)
	}
	if counts.report.Published != 1 {
		test.Fatalf("the rebuild published %d records after the revocation committed, not the revoked one", counts.report.Published)
	}
}
