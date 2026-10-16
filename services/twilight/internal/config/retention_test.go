package config

import (
	"strings"
	"testing"
)

func TestRetentionAndErasureDefaults(test *testing.T) {
	loaded, failure := Load(writeFile(test, minimal), true, nil)
	if failure != nil {
		test.Fatal(failure)
	}
	if loaded.Retention.InstallationDays != 120 || loaded.Retention.AlertDays != 400 || loaded.Retention.ErasureOverdueDays != 20 || len(loaded.Retention.TenantDays) != 0 {
		test.Fatalf("retention %+v", loaded.Retention)
	}
	if loaded.ClickHouse.URL != "http://clickhouse:8123" || loaded.ClickHouse.LakeCollection != "lake_writer" || loaded.ObjectStore.FilesBucket != "dusk-files" ||
		loaded.ObjectStore.EvidenceBucket != "dusk-ledger-evidence" {
		test.Fatalf("stores %+v %+v", loaded.ClickHouse, loaded.ObjectStore)
	}
}

func TestTenantRetentionComesFromTheFileOrTheEnvironment(test *testing.T) {
	loaded, failure := Load(writeFile(test, minimal+`
retention:
  tenant_days:
    acme: 90
`), true, []string{"TWILIGHT__RETENTION__INSTALLATION_DAYS=200"})
	if failure != nil || loaded.Retention.TenantDays["acme"] != 90 || loaded.Retention.InstallationDays != 200 {
		test.Fatalf("%+v %v", loaded.Retention, failure)
	}
	loaded, failure = Load(writeFile(test, minimal), true, []string{"TWILIGHT__RETENTION__TENANT_DAYS=acme=30, globex=7"})
	if failure != nil || loaded.Retention.TenantDays["acme"] != 30 || loaded.Retention.TenantDays["globex"] != 7 {
		test.Fatalf("%+v %v", loaded.Retention, failure)
	}
	if _, failure := Load(writeFile(test, minimal), true, []string{"TWILIGHT__RETENTION__TENANT_DAYS=acme=soon"}); failure == nil || !strings.Contains(failure.Error(), "acme") {
		test.Fatalf("a malformed number: %v", failure)
	}
}

func TestRetentionAndStoreMistakesAreRefused(test *testing.T) {
	for _, entry := range []struct{ environment, problem string }{
		{"TWILIGHT__RETENTION__INSTALLATION_DAYS=30", "retention.installation_days must be at least 98"},
		{"TWILIGHT__RETENTION__TENANT_DAYS=Acme=30", `retention.tenant_days names "Acme"`},
		{"TWILIGHT__RETENTION__TENANT_DAYS=acme=0", "retention.tenant_days of acme must be at least 1"},
		{"TWILIGHT__RETENTION__ALERT_DAYS=0", "retention.alert_days must be at least 1"},
		{"TWILIGHT__CLICKHOUSE__URL=clickhouse:8123", "clickhouse.url must be an http or https URL"},
		{"TWILIGHT__CLICKHOUSE__CLUSTER=dusk cluster", "clickhouse.cluster and clickhouse.lake_collection"},
		{"TWILIGHT__OBJECT_STORE__ENDPOINT=", "object_store.endpoint must be an http or https URL"},
		{"TWILIGHT__OBJECT_STORE__FILES_BUCKET=", "object_store.files_bucket is required"},
	} {
		_, failure := Load(writeFile(test, minimal), true, []string{entry.environment})
		if failure == nil || !strings.Contains(failure.Error(), entry.problem) {
			test.Errorf("%s: %v", entry.environment, failure)
		}
	}
}
