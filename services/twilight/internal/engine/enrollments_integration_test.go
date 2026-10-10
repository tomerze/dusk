//go:build integration

package engine_test

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/kafka"
)

func (current *harness) enrollWith(target node, credential, remote string) {
	now := kafka.FormatTime(time.Now())
	current.produceJSON(current.settings.Kafka.Topics.Enrollments, target.device+"/"+target.installation, kafka.ContractEnrollments, map[string]any{
		"schema": "dusk.enrollments/v1", "id": kafka.NewMessageID(), "time": now, "operation": "enroll", "outcome": "issued", "reason": nil,
		"device_id": target.device, "installation_id": target.installation, "tenant": "acme", "credential_kind": "fleet_token", "credential_ref": credential,
		"credential_issuer": nil, "hardware_fingerprint_hash": strings.Repeat("d", 64), "remote_address": remote, "cert_serial": "01",
		"cert_fingerprint": strings.Repeat("c", 64), "cert_not_after": now, "dusk_version": target.version, "impl": "nix", "target_os": "linux",
		"target_arch": "x86_64", "hostname": "node", "instance": "nightfall-0",
	})
}

func TestABulkRevocationRevokesEveryInstallationOneCredentialEnrolledAtItsPace(test *testing.T) {
	current := newHarness(test)
	current.settings.Engine.RevocationsPerSecond = 10
	leaked := make([]node, 25)
	for index := range leaked {
		leaked[index] = identity(100 + index)
		current.enrollWith(leaked[index], "batch-7", fmt.Sprintf("198.51.100.%d:4000", index))
	}
	others := []node{identity(200), identity(201), identity(202)}
	for _, target := range others {
		current.enrollWith(target, "batch-8", "192.0.2.1:4000")
	}
	current.start()
	current.eventually("every enrollment reaches inventory with its credential", time.Minute, func() (bool, string) {
		var recorded int
		_ = current.pool.QueryRow(context.Background(), `select count(*) from nodes where credential_kind = 'fleet_token' and credential_ref in ('batch-7', 'batch-8')`).Scan(&recorded)
		return recorded == 28, fmt.Sprintf("%d recorded", recorded)
	})
	service := current.engine.Service()
	matching, failure := service.Nodes(context.Background(), `credential_ref == "batch-7"`, nil, "", "", 100)
	if failure != nil || len(matching.Nodes) != 25 {
		test.Fatalf("a credential selector found %d nodes: %v", len(matching.Nodes), failure)
	}
	counted, failure := service.StartRevocation(context.Background(), engine.RevocationRequest{CredentialKind: "fleet_token", Credential: "batch-7", DryRun: true}, "token:admin")
	if failure != nil || counted.Matched != 25 || counted.Revocation != nil {
		test.Fatalf("dry run %+v %v", counted, failure)
	}
	started := time.Now()
	running, failure := service.StartRevocation(context.Background(), engine.RevocationRequest{CredentialKind: "fleet_token", Credential: "batch-7", Reason: "leaked token"}, "token:admin")
	if failure != nil || running.Revocation == nil || running.Matched != 25 {
		test.Fatalf("revocation %+v %v", running, failure)
	}
	current.eventually("the revocation finishes", time.Minute, func() (bool, string) {
		found, failure := service.Revocation(context.Background(), running.Revocation.ID)
		return failure == nil && found.FinishedAt != nil, fmt.Sprintf("%+v %v", found, failure)
	})
	if elapsed := time.Since(started); elapsed < 2*time.Second {
		test.Fatalf("25 installations at 10 a second took %s", elapsed)
	}
	finished, _ := service.Revocation(context.Background(), running.Revocation.ID)
	if finished.Revoked != 25 || finished.Matched != 25 {
		test.Fatalf("finished %+v", finished)
	}
	lifecycles := map[string]int{}
	rows, _ := current.pool.Query(context.Background(), `select credential_ref || ':' || lifecycle, count(*) from nodes where credential_ref in ('batch-7', 'batch-8') group by 1`)
	for rows.Next() {
		var group string
		var count int
		_ = rows.Scan(&group, &count)
		lifecycles[group] = count
	}
	rows.Close()
	if lifecycles["batch-7:revoked"] != 25 || lifecycles["batch-8:enrolled"] != 3 || len(lifecycles) != 2 {
		test.Fatalf("lifecycles after the revocation %v", lifecycles)
	}
	consumer, _ := kgo.NewClient(append(must(kafka.Options(current.settings.Kafka, "check")), kgo.ConsumeTopics(current.settings.Kafka.Topics.NodeState), kgo.ConsumeResetOffset(kgo.NewOffset().AtStart()))...)
	defer consumer.Close()
	polling, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	states := map[string]map[string]any{}
	for len(states) < 25 && polling.Err() == nil {
		consumer.PollRecords(polling, 100).EachRecord(func(record *kgo.Record) {
			if failure := current.validator.Validate(kafka.ContractNodeState, record.Value); failure != nil {
				test.Errorf("a node-state record fails its contract: %v", failure)
			}
			var state map[string]any
			_ = json.Unmarshal(record.Value, &state)
			states[string(record.Key)] = state
		})
	}
	for _, target := range leaked {
		state := states["installation/"+target.device+"/"+target.installation]
		if state["lifecycle"] != "revoked" || state["actor"] != "token:admin" || state["reason"] != "leaked token" {
			test.Fatalf("the node-state record of %s is %v", target.device, state)
		}
	}
	if len(states) != 25 {
		test.Fatalf("%d node-state records for 25 revoked installations", len(states))
	}
}

func TestAnEnrollmentFloodOpensAnAlertThatNamesItsCredential(test *testing.T) {
	current := newHarness(test)
	current.settings.Alerts.Enrollments.IssuedFloors.Credential = 5
	for index := range 12 {
		current.enrollWith(identity(300+index), "leaked", fmt.Sprintf("203.0.113.%d:4000", index))
	}
	current.start()
	var detail map[string]any
	var occurrences int64
	current.eventually("the enrollment alert opens and is escalated once the count doubles", time.Minute, func() (bool, string) {
		var encoded []byte
		failure := current.pool.QueryRow(context.Background(), `select detail, occurrences from alerts where fingerprint = 'enrollment_rate:credential:fleet_token:leaked' and resolved_at is null`).Scan(&encoded, &occurrences)
		if failure != nil {
			return false, failure.Error()
		}
		_ = json.Unmarshal(encoded, &detail)
		return occurrences == 2, fmt.Sprintf("%d occurrences, detail %v", occurrences, detail)
	})
	if detail["credential"] != "leaked" || detail["credential_kind"] != "fleet_token" || detail["tenant"] != "acme" || detail["count"] != float64(12) || detail["threshold"] != float64(5) {
		test.Fatalf("the alert detail %v", detail)
	}
	current.eventually("the counts reach the baseline table", time.Minute, func() (bool, string) {
		var counted int64
		_ = current.pool.QueryRow(context.Background(), `select coalesce(sum(count), 0) from enrollment_counts where signal = 'issued' and scope = 'credential' and name = 'fleet_token:leaked'`).Scan(&counted)
		return counted == 12, fmt.Sprintf("%d counted", counted)
	})
}
