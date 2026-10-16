package erasure

import (
	"context"
	"fmt"
	"regexp"
	"strconv"
	"time"

	"dusk/services/twilight/internal/clickhouse"
	"dusk/services/twilight/internal/objectstore"
)

var tableTTL = regexp.MustCompile(`TTL time \+ toIntervalDay\((\d+)\)`)

func (manager *Manager) retained(operation context.Context, erasure Erasure) []Retained {
	now := manager.Now()
	at := func(duration time.Duration) *time.Time {
		until := now.Add(duration).UTC().Truncate(time.Second)
		return &until
	}
	day := 24 * time.Hour
	subject := "the device"
	if erasure.Tenant != nil {
		subject = "the tenant's devices"
	}
	var retained []Retained
	note := func(store, data string, until *time.Time, why string) {
		retained = append(retained, Retained{Store: store, Data: data, Until: until, Why: why})
	}
	const ledgerWhy = "the ledger is the security record of what was done to the fleet: it holds only pseudonymous ids, parameter hashes and, for addresses, a network and a keyed hash; deleting entries would break the hash chain that makes it evidence"
	if retention, failure := manager.ObjectStore.ObjectLockRetention(operation, manager.Config.ObjectStore.EvidenceBucket); failure == nil {
		note("object store "+manager.Config.ObjectStore.EvidenceBucket, "the ledger's evidence copy", at(time.Duration(retention.Days+2)*day),
			fmt.Sprintf("locked in %s mode for %d days; %s", retention.Mode, retention.Days, ledgerWhy))
	} else {
		manager.Logger.Warn("the evidence bucket's retention was not read for an erasure report", "erasure_id", erasure.ID, "error", failure)
		note("object store "+manager.Config.ObjectStore.EvidenceBucket, "the ledger's evidence copy", nil, "object-locked; its retention could not be read: "+failure.Error())
	}
	if rules, failure := manager.ObjectStore.ExpirationRules(operation, manager.Config.ObjectStore.LakeBucket); failure == nil || failure == objectstore.ErrNoLifecycle {
		var until *time.Time
		if days, found := objectstore.ExpirationFor(rules, "ledger/"); found {
			until = at(time.Duration(days+1) * day)
		}
		note("object store "+manager.Config.ObjectStore.LakeBucket, "the ledger's copy in the lake (ledger/)", until, ledgerWhy)
	} else {
		manager.Logger.Warn("the lake's lifecycle rules were not read for an erasure report", "erasure_id", erasure.ID, "error", failure)
	}
	if found, failure := clickhouse.Scan[struct {
		Engine string `json:"engine_full"`
	}](operation, manager.ClickHouse, clickhouse.Query{Text: `SELECT engine_full FROM system.tables WHERE database = 'dusk' AND name = 'ledger'`}); failure == nil && len(found) == 1 {
		var until *time.Time
		if match := tableTTL.FindStringSubmatch(found[0].Engine); match != nil {
			days, _ := strconv.Atoi(match[1])
			until = at(time.Duration(days+1) * day)
		}
		note("clickhouse dusk.ledger", "the ledger's queryable copy", until, ledgerWhy)
	} else if failure != nil {
		manager.Logger.Warn("the ledger table's TTL was not read for an erasure report", "erasure_id", erasure.ID, "error", failure)
	}
	if manager.Topics != nil {
		topics, failure := manager.Topics(operation)
		if failure != nil {
			manager.Logger.Warn("the Kafka topics' retention was not read for an erasure report", "erasure_id", erasure.ID, "error", failure)
		}
		for _, topic := range topics {
			switch {
			case topic.CleanupPolicy == "delete" && topic.Retention > 0:
				note("kafka "+topic.Topic, "records produced before the erasure", at(topic.Retention+topic.Segment),
					fmt.Sprintf("a Kafka topic is append-only: a record leaves when its whole log segment is older than retention.ms (%s); segments roll every segment.ms (%s)", topic.Retention, topic.Segment))
			case topic.CleanupPolicy == "delete":
				note("kafka "+topic.Topic, "records produced before the erasure", nil, "a Kafka topic is append-only and this one has no retention.ms limit")
			case topic.Topic == "dusk.intended-processes":
				note("kafka "+topic.Topic, "earlier records of the intended processes of "+subject, nil, "tombstoned by this erasure; compaction removes the earlier records")
			case topic.Topic == "dusk.census":
				note("kafka "+topic.Topic, "earlier census records", nil, "each nightfall instance's next full census leaves "+subject+" out, and compaction removes the earlier records")
			}
		}
	}
	var alertsKept int64
	for _, step := range erasure.Steps {
		alertsKept += step.Counts["alerts_kept"]
	}
	if alertsKept > 0 {
		note("postgres alerts", fmt.Sprintf("%d alerts that name %s", alertsKept, subject), nil,
			fmt.Sprintf("alerts are security records; a resolved alert is deleted %d days after it was resolved", manager.Config.Retention.AlertDays))
	}
	note("postgres devices, kafka dusk.node-state", "the retired or revoked lifecycle of "+subject+", its reason and who set it", nil,
		"nightfall refuses a withdrawn device or installation from this record; it stays while it must stay out")
	note("postgres erasures", "this erasure: who asked for it, when and why", nil, "the record that the erasure was asked for and done")
	note("step-ca", "the certificates step-ca issued to "+subject, nil, "step-ca's database keeps what it signed; the certificates name only the device, installation and tenant ids")
	note("SigNoz", "node logs and traces SigNoz read from dusk.otel-logs and dusk.otel-spans", nil, "SigNoz keeps them for its own retention period (Settings, General); an erasure does not reach SigNoz")
	note("service logs", "log lines of nightfall, dawn and twilight that name "+subject, nil, "kept by the log collector you run, for its retention")
	note("alert receivers", "notifications already sent about alerts that name "+subject, nil, "they left the stack when they were sent; delete them in PagerDuty, Slack, Teams, the mailbox or the webhook's store")
	return retained
}
