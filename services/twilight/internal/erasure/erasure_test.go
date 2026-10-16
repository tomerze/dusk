package erasure

import (
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

var sampleErasure = uuid.MustParse("0192f0c4-9e1a-7b8e-9d2f-3a4b5c6d7e8f")

func TestPredicatesSelectTheScope(test *testing.T) {
	devices := "device_id IN (SELECT value FROM dusk.erasure_scope WHERE erasure_id = '" + sampleErasure.String() + "' AND kind = 'device')"
	if got := predicate(table{"connections", false}, sampleErasure); got != devices {
		test.Fatalf("connections: %s", got)
	}
	telemetry := predicate(table{"otel_logs", true}, sampleErasure)
	if !strings.HasPrefix(telemetry, "(attributes['device_id'] IN (SELECT value") || !strings.Contains(telemetry, "resource_attributes['host.id'] IN (SELECT value") ||
		!strings.Contains(telemetry, "kind = 'host'") {
		test.Fatalf("telemetry: %s", telemetry)
	}
}

func TestHostsAreNamespacesWithoutLeadingZeros(test *testing.T) {
	for namespace, host := range map[string]string{"05d2e9a1c7b3f8e0": "5d2e9a1c7b3f8e0", "5D2E9A1C7B3F8E04": "5d2e9a1c7b3f8e04", "0000000000000000": "0", "000000000000000a": "a"} {
		if got := hostOf(namespace); got != host {
			test.Errorf("%s: %s, want %s", namespace, got, host)
		}
	}
}

func TestARewrittenLakeFileStaysInItsPartitionUnderAStableName(test *testing.T) {
	key := "connections/dt=2026-10-01/1759300000-0192f0c4.parquet"
	first, again, other := rewrittenKey(key, "a"), rewrittenKey(key, "a"), rewrittenKey(key, "b")
	if first != again || first == other || !strings.HasPrefix(first, "connections/dt=2026-10-01/") || !strings.HasSuffix(first, ".parquet") || first == key {
		test.Fatalf("rewritten %s %s %s", first, again, other)
	}
}

func TestStepsOfAnErasure(test *testing.T) {
	names := func(steps []Step) string {
		var found []string
		for _, step := range steps {
			if step.State != StepPending || step.Counts == nil {
				test.Fatalf("a new step %+v", step)
			}
			found = append(found, step.Name)
		}
		return strings.Join(found, ",")
	}
	if got := names(newSteps()); got != "scope,clickhouse,lake,files,inventory" {
		test.Fatalf("erasure steps %s", got)
	}
}

func TestBackoffGrowsAndStaysUnderAnHour(test *testing.T) {
	previousCeiling := time.Duration(0)
	for attempts := 1; attempts <= 20; attempts++ {
		ceiling := min(30*time.Second<<min(attempts, 10), maximumBackoff)
		for range 50 {
			delay := backoff(attempts)
			if delay < ceiling/2 || delay > ceiling {
				test.Fatalf("attempt %d waited %s outside [%s, %s]", attempts, delay, ceiling/2, ceiling)
			}
		}
		if ceiling < previousCeiling {
			test.Fatalf("the ceiling shrank at attempt %d", attempts)
		}
		previousCeiling = ceiling
	}
}

func TestReasonsAreTrimmedAndBounded(test *testing.T) {
	if reason, failure := ValidReason("  ticket 9 \n"); failure != nil || reason != "ticket 9" {
		test.Fatalf("%q %v", reason, failure)
	}
	for _, bad := range []string{"", "   ", strings.Repeat("x", 1001)} {
		if _, failure := ValidReason(bad); failure != ErrInvalidReason {
			test.Errorf("%d characters: %v", len(bad), failure)
		}
	}
}

func TestANotReadyErrorSaysWhatToDo(test *testing.T) {
	message := (&NotReadyError{Subject: "tenant acme", NotWithdrawn: 2, Online: 1, RunningCampaigns: 3}).Error()
	for _, part := range []string{"tenant acme cannot be erased yet", "2 of its installations are neither retired nor revoked", "1 of its installations are online",
		"3 of its campaigns are running or paused", "retire or revoke it"} {
		if !strings.Contains(message, part) {
			test.Errorf("%q lacks %q", message, part)
		}
	}
}
