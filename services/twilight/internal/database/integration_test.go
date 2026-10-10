//go:build integration

package database_test

import (
	"context"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/testsupport"
	"dusk/services/twilight/migrations"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func TestMigrationsApplyOnceAndRefuseEditedHistory(test *testing.T) {
	pool, _ := testsupport.EmptyDatabase(test)
	operation := context.Background()
	loaded, failure := database.LoadMigrations(migrations.Files)
	if failure != nil {
		test.Fatal(failure)
	}
	applied, failure := database.Migrate(operation, pool, loaded, testsupport.Logger())
	if failure != nil {
		test.Fatal(failure)
	}
	if len(applied) != len(loaded) {
		test.Fatalf("applied %d of %d migrations", len(applied), len(loaded))
	}
	again, failure := database.Migrate(operation, pool, loaded, testsupport.Logger())
	if failure != nil || len(again) != 0 {
		test.Fatalf("second run applied %d, error %v", len(again), failure)
	}
	status, failure := database.Status(operation, pool, loaded)
	if failure != nil || len(status.Pending) != 0 || len(status.Applied) != len(loaded) {
		test.Fatalf("status %+v, error %v", status, failure)
	}
	edited := append([]database.Migration(nil), loaded...)
	edited[0].Checksum = strings.Repeat("0", 64)
	if _, failure := database.Migrate(operation, pool, edited, testsupport.Logger()); failure == nil || !strings.Contains(failure.Error(), "never change once applied") {
		test.Fatalf("edited migration accepted: %v", failure)
	}
	for _, table := range []string{"nodes", "node_presence", "campaigns", "campaign_counters", "campaign_events", "campaign_nodes", "intended_processes", "alerts", "reconcile_offsets", "ledger_chain_heads", "reconcile_processes", "leadership", "api_tokens", "api_sessions", "api_login_key"} {
		var exists bool
		if failure := pool.QueryRow(operation, `select to_regclass($1) is not null`, table).Scan(&exists); failure != nil || !exists {
			test.Errorf("table %s missing (%v)", table, failure)
		}
	}
	var fillfactor string
	if failure := pool.QueryRow(operation, `select array_to_string(reloptions, ',') from pg_class where relname = 'node_presence'`).Scan(&fillfactor); failure != nil || fillfactor != "fillfactor=50" {
		test.Errorf("node_presence options %q (%v)", fillfactor, failure)
	}
	var presenceIndexes int
	if failure := pool.QueryRow(operation, `select count(*) from pg_indexes where tablename = 'node_presence'`).Scan(&presenceIndexes); failure != nil || presenceIndexes != 1 {
		test.Errorf("node_presence has %d indexes (%v), want only its primary key", presenceIndexes, failure)
	}
}

func TestMigrationRunnerRollsBackAFailedMigration(test *testing.T) {
	pool, _ := testsupport.EmptyDatabase(test)
	operation := context.Background()
	broken := fstest.MapFS{
		"0001_good.sql":   {Data: []byte("create table good (id int);")},
		"0002_broken.sql": {Data: []byte("create table half (id int); select no_such_function();")},
	}
	loaded, failure := database.LoadMigrations(broken)
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := database.Migrate(operation, pool, loaded, testsupport.Logger()); failure == nil || !strings.Contains(failure.Error(), "0002_broken") {
		test.Fatalf("broken migration: %v", failure)
	}
	var half bool
	if failure := pool.QueryRow(operation, `select to_regclass('half') is not null`).Scan(&half); failure != nil || half {
		test.Fatalf("failed migration left its table behind (%v)", failure)
	}
	status, failure := database.Status(operation, pool, loaded)
	if failure != nil || len(status.Applied) != 1 || len(status.Pending) != 1 {
		test.Fatalf("status %+v (%v)", status, failure)
	}
}

func TestLoadMigrationsRejectsBadNames(test *testing.T) {
	if _, failure := database.LoadMigrations(fstest.MapFS{"1_x.sql": {Data: []byte("")}}); failure == nil {
		test.Fatal("short version accepted")
	}
	if _, failure := database.LoadMigrations(fstest.MapFS{"0001_a.sql": {}, "0001_b.sql": {}}); failure == nil {
		test.Fatal("duplicate version accepted")
	}
}

func TestCampaignPartitionsCreateAndDrop(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	campaign := uuid.New()
	failure := pgx.BeginFunc(operation, pool, func(transaction pgx.Tx) error {
		return database.CreateCampaignPartitions(operation, transaction, campaign)
	})
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state) values ($1, $2, $3, 0, 'pending')`,
		campaign, strings.Repeat("a", 32), strings.Repeat("b", 32)); failure != nil {
		test.Fatalf("insert into partition: %v", failure)
	}
	if _, failure := pool.Exec(operation, `insert into campaign_events (campaign_id, kind, actor) values ($1, 'created', 'test')`, campaign); failure != nil {
		test.Fatalf("insert event: %v", failure)
	}
	if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state) values ($1, $2, $3, 0, 'pending')`,
		uuid.New(), strings.Repeat("a", 32), strings.Repeat("b", 32)); failure == nil {
		test.Fatal("insert for a campaign without a partition succeeded")
	}
	countPartitions := func() int {
		var count int
		if failure := pool.QueryRow(operation, `select count(*) from pg_class where relname like '%' || replace($1::text, '-', '')`, campaign.String()).Scan(&count); failure != nil {
			test.Fatal(failure)
		}
		return count
	}
	if count := countPartitions(); count != 2 {
		test.Fatalf("%d partitions, want 2", count)
	}
	if failure := database.DropCampaignPartitions(operation, pool, campaign); failure != nil {
		test.Fatal(failure)
	}
	if count := countPartitions(); count != 0 {
		test.Fatalf("%d partitions left after drop", count)
	}
	var rows int
	if failure := pool.QueryRow(operation, `select count(*) from campaign_nodes`).Scan(&rows); failure != nil || rows != 0 {
		test.Fatalf("%d rows left (%v)", rows, failure)
	}
}

func TestIntendedProcessPartitionsRollForwardAndExpire(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	now := time.Date(2027, 3, 15, 12, 0, 0, 0, time.UTC)
	changes, failure := database.MaintainIntendedProcessPartitions(operation, pool, now.AddDate(0, 0, -40), 2, 31*24*time.Hour)
	if failure != nil {
		test.Fatal(failure)
	}
	if len(changes.Created) != 4 {
		test.Fatalf("created %v", changes.Created)
	}
	changes, failure = database.MaintainIntendedProcessPartitions(operation, pool, now, 2, 31*24*time.Hour)
	if failure != nil {
		test.Fatal(failure)
	}
	if len(changes.Created) != 4 {
		test.Fatalf("created %v", changes.Created)
	}
	wantDropped := map[string]bool{"intended_processes_20270202": true, "intended_processes_20270203": true, "intended_processes_20270204": true, "intended_processes_20270205": true}
	dropped := 0
	for _, name := range changes.Dropped {
		if wantDropped[name] {
			dropped++
		}
	}
	if dropped != 4 {
		test.Fatalf("dropped %v", changes.Dropped)
	}
	insert := `insert into intended_processes (pid, created_at, campaign_id, attempt, device_id, installation_id, action_kind, principal, subject, expires_at, max_commands,
			default_shell_commands, last_dispatched_at)
		values ($1::text::numeric, $2, $3, 1, 'd', 'i', 'run_script', 'twilight', 'campaign:x', $2, 8, 3, $2)`
	if _, failure := pool.Exec(operation, insert, "18446744073709551615", now, uuid.New()); failure != nil {
		test.Fatalf("insert into today's partition: %v", failure)
	}
	for _, reserved := range []string{"0", "65535", "17505437192229758416"} {
		if _, failure := pool.Exec(operation, insert, reserved, now, uuid.New()); failure == nil {
			test.Errorf("reserved pid %s was accepted", reserved)
		}
	}
	if _, failure := pool.Exec(operation, `insert into intended_processes (pid, created_at, campaign_id, device_id, installation_id, action_kind, principal, subject, expires_at, max_commands,
			default_shell_commands, last_dispatched_at)
		values (65536, $1, $2, 'd', 'i', 'run_script', 'twilight', 'campaign:x', $1, 8, 3, $1)`, now, uuid.New()); failure == nil {
		test.Error("campaign work without an attempt was accepted")
	}
	again, failure := database.MaintainIntendedProcessPartitions(operation, pool, now, 2, 31*24*time.Hour)
	if failure != nil || len(again.Created) != 0 || len(again.Dropped) != 0 {
		test.Fatalf("second run %+v (%v)", again, failure)
	}
}

func TestSemverKeyFunction(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	cases := map[string]string{
		"1.2.3":            "{1,2,3,1}",
		"1.2.3-rc.1":       "{1,2,3,0}",
		"1.2.3+build":      "{1,2,3,1}",
		"10.20.30-alpha+b": "{10,20,30,0}",
		"01.2.3":           "",
		"1.2":              "",
		"latest":           "",
	}
	for version, want := range cases {
		var got *string
		if failure := pool.QueryRow(operation, `select semver_key($1)::text`, version).Scan(&got); failure != nil {
			test.Fatal(failure)
		}
		if (got == nil) != (want == "") || (got != nil && *got != want) {
			test.Errorf("semver_key(%q) = %v, want %q", version, got, want)
		}
	}
}
