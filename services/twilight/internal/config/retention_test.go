package config

import (
	"strings"
	"testing"
)

func TestRetentionDefaults(test *testing.T) {
	loaded, failure := Load(writeFile(test, minimal), true, nil)
	if failure != nil {
		test.Fatal(failure)
	}
	if loaded.Retention.InstallationDays != 120 || loaded.Retention.AlertDays != 400 {
		test.Fatalf("retention %+v", loaded.Retention)
	}
}

func TestRetentionMistakesAreRefused(test *testing.T) {
	for _, entry := range []struct{ environment, problem string }{
		{"TWILIGHT__RETENTION__INSTALLATION_DAYS=30", "retention.installation_days must be at least 98"},
		{"TWILIGHT__RETENTION__ALERT_DAYS=0", "retention.alert_days must be at least 1"},
	} {
		_, failure := Load(writeFile(test, minimal), true, []string{entry.environment})
		if failure == nil || !strings.Contains(failure.Error(), entry.problem) {
			test.Errorf("%s: %v", entry.environment, failure)
		}
	}
}
