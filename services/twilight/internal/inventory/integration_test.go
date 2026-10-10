//go:build integration

package inventory_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/selector"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

var key = inventory.Key{DeviceID: "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", InstallationID: "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"}

func TestInventoryUpkeep(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := inventory.NewStore(pool)
	operation := context.Background()
	at := time.Date(2026, 10, 7, 12, 0, 0, 0, time.UTC)
	fingerprint, renewed, tenant, version, reference := strings.Repeat("a", 64), strings.Repeat("b", 64), "retail-eu", "0.1.0", "unit-7"
	issuer := "factory-2026"
	if failure := store.RecordEnrollment(operation, inventory.Enrollment{Key: key, Operation: "enroll", CertFingerprint: &fingerprint, Tenant: &tenant, DuskVersion: &version,
		CredentialKind: "install_token", CredentialRef: &reference, CredentialIssuer: &issuer, At: at}); failure != nil {
		test.Fatal(failure)
	}
	node, failure := store.Get(operation, key)
	if failure != nil || node.Lifecycle != "enrolled" || *node.CertFingerprint != fingerprint || node.EnrolledAt == nil || *node.DuskVersion != "0.1.0" {
		test.Fatalf("enrolled node %+v %v", node, failure)
	}
	if node.CredentialKind == nil || *node.CredentialKind != "install_token" || *node.CredentialRef != reference || *node.CredentialIssuer != issuer {
		test.Fatalf("the node's credential %v %v %v", node.CredentialKind, node.CredentialRef, node.CredentialIssuer)
	}

	accept := func(string) error { return nil }
	refused := errors.New("the node-state record was not acknowledged")
	if previous, failure := store.SetLifecycle(operation, key, "revoked", "test", at, func(string) error { return refused }); !errors.Is(failure, refused) || previous != "enrolled" {
		test.Fatalf("a refused change: %q %v", previous, failure)
	}
	if node, _ := store.Get(operation, key); node.Lifecycle != "enrolled" {
		test.Fatalf("a change its check refused was stored: %s", node.Lifecycle)
	}
	if previous, failure := store.SetLifecycle(operation, key, "quarantined", "test", at, accept); failure != nil || previous != "enrolled" {
		test.Fatalf("quarantine: %q %v", previous, failure)
	}
	stranger := inventory.Key{DeviceID: strings.Repeat("c", 32), InstallationID: strings.Repeat("d", 32)}
	if _, failure := store.SetLifecycle(operation, stranger, "quarantined", "test", at, accept); !errors.Is(failure, inventory.ErrNotFound) {
		test.Fatalf("a node inventory does not hold: %v", failure)
	}
	if failure := store.RecordEnrollment(operation, inventory.Enrollment{Key: key, Operation: "renew", CertFingerprint: &renewed, At: at.Add(time.Hour)}); failure != nil {
		test.Fatal(failure)
	}
	node, _ = store.Get(operation, key)
	if *node.CertFingerprint != renewed || node.Lifecycle != "quarantined" {
		test.Fatalf("a renewal must change only the fingerprint: %+v", node)
	}
	if previous, failure := store.SetLifecycle(operation, key, "enrolled", "test", at, accept); failure != nil || previous != "quarantined" {
		test.Fatalf("back to enrolled: %q %v", previous, failure)
	}
	activated, failure := store.RecordConnections(operation, []inventory.Sighting{{Key: key, At: at}, {Key: stranger, At: at}})
	if failure != nil || len(activated) != 2 {
		test.Fatalf("activated %v %v", activated, failure)
	}
	unknown, failure := store.Get(operation, stranger)
	if failure != nil || unknown.EnrolledAt != nil || unknown.Lifecycle != "active" || !unknown.FirstSeenAt.Equal(at) {
		test.Fatalf("a node first seen connected: %+v %v", unknown, failure)
	}
	again, _ := store.RecordConnections(operation, []inventory.Sighting{{Key: key, At: at}})
	if len(again) != 0 {
		test.Fatal("activated twice")
	}
	factsJSON := map[string]json.RawMessage{
		"dusk.device.id": json.RawMessage(`"x"`), "dusk.version": json.RawMessage(`"0.2.0"`), "dusk.target.os": json.RawMessage(`"linux"`),
		"dusk.os.linux.os_release.id": json.RawMessage(`"debian"`), "dusk.os.time_zone": json.RawMessage(`"Europe/Berlin"`),
		"dusk.device.memory_bytes": json.RawMessage(`8589934592`),
	}
	if failure := store.ApplyReport(operation, key, "5d2e9a1c7b3f8e04", at.Add(time.Minute), inventory.Reported{Facts: factsJSON, Services: []string{"nightfall"}}); failure != nil {
		test.Fatal(failure)
	}
	node, _ = store.Get(operation, key)
	if *node.OSName != "debian" || *node.Country != "DE" || *node.ReportedVersion != "0.2.0" || *node.FactsNamespaceID != "5d2e9a1c7b3f8e04" || strings.Contains(string(node.FactsJSON), "dusk.device.id") {
		test.Fatalf("facts applied: %+v %s", node, node.FactsJSON)
	}
	appVersion, appKey, configHash := "3.1.0", "app.version", "c0ffee"
	if failure := store.ApplyReport(operation, key, "ffffffffffffffff", at.Add(2*time.Minute), inventory.Reported{VersionKey: &appKey, Version: &appVersion, ConfigHash: &configHash}); failure != nil {
		test.Fatal(failure)
	}
	node, _ = store.Get(operation, key)
	if value, _ := node.Fact("app.version"); value != "3.1.0" || *node.ReportedConfigHash != "c0ffee" || *node.FactsNamespaceID != "5d2e9a1c7b3f8e04" || *node.OSName != "debian" {
		test.Fatalf("a partial report: %+v %s", node, node.FactsJSON)
	}
	if failure := store.ApplyReport(operation, key, "0000000000000000", at, inventory.Reported{Facts: map[string]json.RawMessage{}}); failure != nil {
		test.Fatal(failure)
	}
	node, _ = store.Get(operation, key)
	if *node.FactsNamespaceID != "5d2e9a1c7b3f8e04" {
		test.Fatal("older facts replaced newer ones")
	}
	parsed, _ := selector.Parse(`country == "DE" and facts["app.version"] == "3.1.0"`)
	byKey := inventory.Order{Column: "device_id"}
	page, failure := store.List(operation, parsed, byKey, "", 10)
	if failure != nil || len(page.Nodes) != 1 || page.Next != "" {
		test.Fatalf("list %+v %v", page, failure)
	}
	all, _ := selector.Parse("")
	first, _ := store.List(operation, all, byKey, "", 1)
	if len(first.Nodes) != 1 || first.Next == "" {
		test.Fatalf("first page %+v", first)
	}
	second, _ := store.List(operation, all, byKey, first.Next, 1)
	if len(second.Nodes) != 1 || second.Next != "" || second.Nodes[0].Key() == first.Nodes[0].Key() {
		test.Fatalf("second page %+v", second)
	}
	connected := at
	presence := []inventory.Presence{{Key: key, Online: true, ConnectedAt: &connected, LastSeenAt: &connected,
		Sessions: []inventory.PresenceSession{{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 7, Instance: "nightfall-0", InnerAddress: "nightfall-0:8444", ConnectedAt: at}}},
		{Key: inventory.Key{DeviceID: strings.Repeat("e", 32), InstallationID: strings.Repeat("f", 32)}, Online: false}}
	if failure := store.WritePresence(operation, presence); failure != nil {
		test.Fatal(failure)
	}
	if failure := store.UpdateLastSeen(operation, []inventory.Key{key}, []time.Time{at.Add(time.Hour)}); failure != nil {
		test.Fatal(failure)
	}
	stored, failure := store.GetPresence(operation, key)
	if failure != nil || !stored.Online || !stored.LastSeenAt.Equal(at.Add(time.Hour)) || len(stored.Sessions) != 1 || stored.Sessions[0].Epoch != 7 {
		test.Fatalf("presence %+v %v", stored, failure)
	}
	scanned := 0
	if failure := store.ScanPresence(operation, func(inventory.Presence) error { scanned++; return nil }); failure != nil || scanned != 2 {
		test.Fatalf("scanned %d %v", scanned, failure)
	}
	counts, failure := store.LifecycleCounts(operation)
	if failure != nil || counts["active"] != 2 || counts["enrolled"] != 1 {
		test.Fatalf("lifecycle counts %v %v", counts, failure)
	}
	stolen := inventory.DeviceLifecycle{DeviceID: key.DeviceID, Lifecycle: "revoked", Reason: "stolen", ChangedAt: at.Add(time.Hour), Actor: "token:admin"}
	if previous, failure := store.SetDeviceLifecycle(operation, stolen, accept); failure != nil || previous != "active" {
		test.Fatalf("revoke the device: %q %v", previous, failure)
	}
	withdrawn, failure := store.Withdrawn(operation)
	if failure != nil || len(withdrawn) != 1 || withdrawn[0].Key != key || withdrawn[0].Lifecycle != "revoked" || !withdrawn[0].ChangedAt.Equal(stolen.ChangedAt) {
		test.Fatalf("an online installation of a revoked device: %+v %v", withdrawn, failure)
	}
	if _, failure := store.SetDeviceLifecycle(operation, inventory.DeviceLifecycle{DeviceID: key.DeviceID, Lifecycle: "active", Reason: "x", ChangedAt: at, Actor: "x"}, func(string) error { return refused }); !errors.Is(failure, refused) {
		test.Fatalf("a device change its check refused: %v", failure)
	}
	if device, failure := store.GetDeviceLifecycle(operation, key.DeviceID); failure != nil || device == nil || device.Lifecycle != "revoked" || device.Actor != "token:admin" {
		test.Fatalf("the device after a refused change: %+v %v", device, failure)
	}
	if previous, failure := store.SetDeviceLifecycle(operation, inventory.DeviceLifecycle{DeviceID: key.DeviceID, Lifecycle: "active", Reason: "recovered", ChangedAt: at.Add(2 * time.Hour), Actor: "token:admin"}, accept); failure != nil || previous != "revoked" {
		test.Fatalf("lift the device's block: %q %v", previous, failure)
	}
	if withdrawn, failure := store.Withdrawn(operation); failure != nil || len(withdrawn) != 0 {
		test.Fatalf("withdrawn after the block was lifted: %+v %v", withdrawn, failure)
	}
	if unknown, failure := store.GetDeviceLifecycle(operation, strings.Repeat("9", 32)); failure != nil || unknown != nil {
		test.Fatalf("a device never changed: %+v %v", unknown, failure)
	}
	refreshedAt := at.Add(10 * time.Minute)
	refreshed := map[string]json.RawMessage{"dusk.version": json.RawMessage(`"0.3.0"`), "dusk.target.os": json.RawMessage(`"linux"`)}
	if failure := store.ApplyReport(operation, key, "1111111111111111", refreshedAt, inventory.Reported{Facts: refreshed}); failure != nil {
		test.Fatal(failure)
	}
	duskKey, staleVersion, staleHash := "dusk.version", "0.2.0", "5ca1ab1e"
	if failure := store.ApplyReport(operation, key, "1111111111111111", at.Add(5*time.Minute), inventory.Reported{VersionKey: &duskKey, Version: &staleVersion, ConfigHash: &staleHash, Services: []string{"stale"}}); failure != nil {
		test.Fatal(failure)
	}
	node, _ = store.Get(operation, key)
	if *node.DuskVersion != "0.3.0" || *node.ReportedVersion != "0.3.0" || !node.ReportedAt.Equal(refreshedAt) || *node.ReportedConfigHash != "c0ffee" || node.ReportedServices[0] == "stale" {
		test.Fatalf("a result older than the stored facts rolled them back: %+v %s", node, node.FactsJSON)
	}
	var poisoned map[string]json.RawMessage
	if failure := json.Unmarshal(database.Storable([]byte(`{"dusk.hostname":"edge\u0000","dusk\u0000key":["\ud800",{"a":"b\u0000"}]}`)), &poisoned); failure != nil {
		test.Fatal(failure)
	}
	if failure := store.ApplyReport(operation, key, "2222222222222222", at.Add(20*time.Minute), inventory.Reported{Facts: poisoned}); failure != nil {
		test.Fatalf("facts that held U+0000 were refused after cleaning: %v", failure)
	}
	node, _ = store.Get(operation, key)
	if *node.Hostname != "edge\uFFFD" || !strings.Contains(string(node.FactsJSON), "dusk\uFFFDkey") {
		test.Fatalf("cleaned facts stored: %+v %s", node, node.FactsJSON)
	}
}

func TestInventorySorting(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := inventory.NewStore(pool)
	operation := context.Background()
	hostnames := []any{"b", nil, "a", "c", nil, "a"}
	versions := []any{"0.10.0", "0.9.0", "garbage", "1.0.0", nil, "0.10.0-rc.1"}
	keys := make([]inventory.Key, len(hostnames))
	sightings := make([]inventory.Sighting, len(hostnames))
	for index := range keys {
		keys[index] = inventory.Key{DeviceID: fmt.Sprintf("%032x", index+1), InstallationID: strings.Repeat("e", 32)}
		sightings[index] = inventory.Sighting{Key: keys[index], At: time.Now()}
	}
	if _, failure := store.RecordConnections(operation, sightings); failure != nil {
		test.Fatal(failure)
	}
	for index, key := range keys {
		if _, failure := pool.Exec(operation, `update nodes set hostname = $3, dusk_version = $4 where device_id = $1 and installation_id = $2`, key.DeviceID, key.InstallationID, hostnames[index], versions[index]); failure != nil {
			test.Fatal(failure)
		}
	}
	all, _ := selector.Parse("")
	expected := map[string][]int{
		"device_id":     {0, 1, 2, 3, 4, 5},
		"-device_id":    {5, 4, 3, 2, 1, 0},
		"hostname":      {2, 5, 0, 3, 1, 4},
		"-hostname":     {4, 1, 3, 0, 5, 2},
		"dusk_version":  {1, 5, 0, 3, 2, 4},
		"-dusk_version": {4, 2, 3, 0, 5, 1},
	}
	for text, want := range expected {
		order, failure := inventory.ParseOrder(text)
		if failure != nil {
			test.Fatal(failure)
		}
		for _, size := range []int{1, 2, 4, 6} {
			var got []int
			cursor := ""
			for pages := 0; pages < 10; pages++ {
				page, failure := store.List(operation, all, order, cursor, size)
				if failure != nil {
					test.Fatalf("%s by %d: %v", text, size, failure)
				}
				for _, node := range page.Nodes {
					for index, key := range keys {
						if node.Key() == key {
							got = append(got, index)
						}
					}
				}
				if cursor = page.Next; cursor == "" {
					break
				}
			}
			if fmt.Sprint(got) != fmt.Sprint(want) {
				test.Errorf("%s in pages of %d: %v, want %v", text, size, got, want)
			}
		}
	}
}

func TestSortedPagesReadOnlyTheirIndex(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id, lifecycle, country, dusk_version)
		select lpad(to_hex(series), 32, '0'), lpad(to_hex(series), 32, '0'),
		       (array['enrolled', 'active', 'quarantined'])[1 + series % 3], (array['US', 'DE', 'BR', 'IN'])[1 + series % 4], '0.' || (series % 5) || '.0'
		from generate_series(1, 10000) as series`); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `vacuum analyze nodes`); failure != nil {
		test.Fatal(failure)
	}
	for _, ordering := range []string{
		`n.lifecycle nulls last, n.device_id, n.installation_id`,
		`n.lifecycle desc nulls first, n.device_id desc, n.installation_id desc`,
		`n.country nulls last, n.device_id, n.installation_id`,
		`semver_key(n.dusk_version) nulls last, n.device_id, n.installation_id`,
	} {
		rows, failure := pool.Query(operation, `explain select n.device_id from nodes n where true order by `+ordering+` limit 101`)
		if failure != nil {
			test.Fatal(failure)
		}
		lines, failure := pgx.CollectRows(rows, pgx.RowTo[string])
		if failure != nil {
			test.Fatal(failure)
		}
		plan := strings.Join(lines, "\n")
		if !strings.Contains(plan, "Index Scan") && !strings.Contains(plan, "Index Only Scan") || strings.Contains(plan, "Sort") {
			test.Errorf("a page ordered by %s sorts instead of reading an index:\n%s", ordering, plan)
		}
	}
}

func TestAnEnrollmentWithoutACredentialKindIsRecordedWithoutOne(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := inventory.NewStore(pool)
	if failure := store.RecordEnrollment(context.Background(), inventory.Enrollment{Key: key, Operation: "enroll", At: time.Now()}); failure != nil {
		test.Fatal(failure)
	}
	node, failure := store.Get(context.Background(), key)
	if failure != nil || node.CredentialKind != nil {
		test.Fatalf("node %+v %v", node, failure)
	}
}
