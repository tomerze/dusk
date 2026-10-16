//go:build integration

package inventory_test

import (
	"context"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/testsupport"
)

func TestPurgeInactiveRemovesOnlyInstallationsLongUnseen(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := inventory.NewStore(pool)
	operation := context.Background()
	now := time.Now().UTC()
	old, recent := now.Add(-200*24*time.Hour), now.Add(-10*24*time.Hour)
	identifier := func(letter string) string { return strings.Repeat(letter, 32) }
	for _, node := range []struct {
		letter    string
		lifecycle string
		updated   time.Time
		online    *bool
		lastSeen  *time.Time
	}{
		{"a", "active", old, new(false), &old},
		{"b", "active", old, new(true), &old},
		{"c", "quarantined", old, new(false), &recent},
		{"d", "enrolled", recent, nil, nil},
		{"e", "revoked", old, nil, nil},
		{"f", "retired", old, nil, nil},
	} {
		if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id, lifecycle, first_seen_at, updated_at) values ($1, $1, $2, $3, $3)`,
			identifier(node.letter), node.lifecycle, node.updated); failure != nil {
			test.Fatal(failure)
		}
		if node.online != nil {
			if _, failure := pool.Exec(operation, `insert into node_presence (device_id, installation_id, online, last_seen_at) values ($1, $1, $2, $3)`,
				identifier(node.letter), *node.online, node.lastSeen); failure != nil {
				test.Fatal(failure)
			}
		}
	}
	before := now.Add(-120 * 24 * time.Hour)
	first, failure := store.PurgeInactive(operation, before, 2)
	if failure != nil || first["active"]+first["revoked"]+first["retired"] != 2 {
		test.Fatalf("the first batch %v %v", first, failure)
	}
	second, failure := store.PurgeInactive(operation, before, 2)
	if failure != nil {
		test.Fatal(failure)
	}
	total := map[string]int64{}
	for _, batch := range []map[string]int64{first, second} {
		for lifecycle, count := range batch {
			total[lifecycle] += count
		}
	}
	if total["active"] != 1 || total["revoked"] != 1 || total["retired"] != 1 || len(total) != 3 {
		test.Fatalf("purged %v", total)
	}
	for _, letter := range []string{"b", "c", "d"} {
		if _, failure := store.Get(operation, inventory.Key{DeviceID: identifier(letter), InstallationID: identifier(letter)}); failure != nil {
			test.Errorf("installation %s was purged: %v", letter, failure)
		}
	}
	var presence int64
	if failure := pool.QueryRow(operation, `select count(*) from node_presence where device_id = $1`, identifier("a")).Scan(&presence); failure != nil || presence != 0 {
		test.Fatalf("the purged installation's presence remains: %d %v", presence, failure)
	}
	if third, failure := store.PurgeInactive(operation, before, 2); failure != nil || len(third) != 0 {
		test.Fatalf("a third pass purged %v %v", third, failure)
	}
}
