//go:build integration

package alerts_test

import (
	"context"
	"testing"
	"time"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/testsupport"
)

func TestPurgeResolvedRemovesOnlyAlertsResolvedBeforeTheCutoff(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := testsupport.AlertStore(test, pool)
	operation := context.Background()
	now := time.Now().UTC()
	var identifiers []int64
	for _, fingerprint := range []string{"old", "recent", "open"} {
		raised, failure := store.Raise(operation, alerts.Raised{Severity: alerts.High, Kind: alerts.KindEnrollmentRate, Fingerprint: fingerprint, Detail: map[string]any{}})
		if failure != nil {
			test.Fatal(failure)
		}
		identifiers = append(identifiers, raised.ID)
	}
	if _, failure := pool.Exec(operation, `update alerts set resolved_at = $2, resolved_by = 'token:test' where id = $1`, identifiers[0], now.Add(-500*24*time.Hour)); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `update alerts set resolved_at = $2, resolved_by = 'token:test' where id = $1`, identifiers[1], now.Add(-24*time.Hour)); failure != nil {
		test.Fatal(failure)
	}
	removed, failure := store.PurgeResolved(operation, now.Add(-400*24*time.Hour), 1000)
	if failure != nil || removed != 1 {
		test.Fatalf("removed %d %v", removed, failure)
	}
	var left []int64
	rows, failure := pool.Query(operation, `select id from alerts order by id`)
	if failure != nil {
		test.Fatal(failure)
	}
	for rows.Next() {
		var identifier int64
		_ = rows.Scan(&identifier)
		left = append(left, identifier)
	}
	rows.Close()
	if len(left) != 2 || left[0] != identifiers[1] || left[1] != identifiers[2] {
		test.Fatalf("left %v of %v", left, identifiers)
	}
}
