//go:build integration

package database_test

import (
	"context"
	"fmt"
	"math/rand/v2"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/testsupport"
	"dusk/services/twilight/migrations"
)

func partitionSettings(partitions int) map[string]func(context.Context) (string, error) {
	return map[string]func(context.Context) (string, error){
		database.NodePartitionsSetting: func(context.Context) (string, error) { return fmt.Sprint(partitions), nil },
	}
}

func migrationsBefore(test *testing.T, version int64) ([]database.Migration, []database.Migration) {
	loaded, failure := database.LoadMigrations(migrations.Files)
	if failure != nil {
		test.Fatal(failure)
	}
	var before []database.Migration
	for _, migration := range loaded {
		if migration.Version < version {
			before = append(before, migration)
		}
	}
	return before, loaded
}

func randomNode(random *rand.Rand) (string, string) {
	return fmt.Sprintf("%016x%016x", random.Uint64(), random.Uint64()), fmt.Sprintf("%016x%016x", random.Uint64(), random.Uint64())
}

func TestNodePartitionInSQLMatchesKafka(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	for _, key := range []string{"", "a", "ab", "abc", "21", "foobar", "a-little-bit-long-string", "a-little-bit-longer-string",
		"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8"} {
		var hash int32
		if failure := pool.QueryRow(operation, `select murmur2(convert_to($1, 'UTF8'))`, key).Scan(&hash); failure != nil {
			test.Fatal(failure)
		}
		if want := kafka.Murmur2([]byte(key)); hash != want {
			test.Errorf("murmur2(%q) is %d in Postgres and %d in Kafka", key, hash, want)
		}
	}
	random := rand.New(rand.NewPCG(3, 4))
	var devices, installations []string
	for range 2000 {
		device, installation := randomNode(random)
		devices, installations = append(devices, device), append(installations, installation)
	}
	for _, partitions := range []int32{1, 3, 48, 128, 32767} {
		rows, failure := pool.Query(operation, `select device_id, installation_id, node_partition(device_id, installation_id, $3)
			from unnest($1::text[], $2::text[]) as node (device_id, installation_id)`, devices, installations, partitions)
		if failure != nil {
			test.Fatal(failure)
		}
		checked := 0
		for rows.Next() {
			var device, installation string
			var partition int16
			if failure := rows.Scan(&device, &installation, &partition); failure != nil {
				test.Fatal(failure)
			}
			if want := kafka.KeyPartition(device+"/"+installation, partitions); int32(partition) != want {
				test.Fatalf("node %s/%s is in partition %d of %d in Postgres and %d in Kafka", device, installation, partition, partitions, want)
			}
			checked++
		}
		if failure := rows.Err(); failure != nil || checked != len(devices) {
			test.Fatalf("checked %d of %d nodes (%v)", checked, len(devices), failure)
		}
	}
}

func TestNodePartitionMigrationBackfillsEveryRow(test *testing.T) {
	pool, _ := testsupport.EmptyDatabase(test)
	operation := context.Background()
	before, loaded := migrationsBefore(test, 18)
	if _, failure := database.Migrate(operation, pool, before, nil, testsupport.Logger()); failure != nil {
		test.Fatal(failure)
	}
	random := rand.New(rand.NewPCG(5, 6))
	campaign := uuid.New()
	var devices, installations []string
	for range 600 {
		device, installation := randomNode(random)
		devices, installations = append(devices, device), append(installations, installation)
	}
	failure := pgx.BeginFunc(operation, pool, func(transaction pgx.Tx) error {
		if failure := database.CreateCampaignPartitions(operation, transaction, campaign); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `insert into nodes (device_id, installation_id) select * from unnest($1::text[], $2::text[])`, devices, installations); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `insert into node_presence (device_id, installation_id, online) select *, true from unnest($1::text[], $2::text[])`, devices, installations); failure != nil {
			return failure
		}
		_, failure := transaction.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state)
			select $1, device_id, installation_id, 0, 'pending' from unnest($2::text[], $3::text[]) as node (device_id, installation_id)`, campaign, devices, installations)
		return failure
	})
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := database.Migrate(operation, pool, loaded, nil, testsupport.Logger()); failure == nil ||
		!strings.Contains(failure.Error(), "twilight.node_partitions must be the partition count of the process results topic") {
		test.Fatalf("the partition migration ran without a partition count: %v", failure)
	}
	if status, failure := database.Status(operation, pool, loaded); failure != nil || len(status.Pending) != len(loaded)-len(before) {
		test.Fatalf("a refused partition migration left %+v (%v)", status, failure)
	}
	const partitions = 48
	if _, failure := database.Migrate(operation, pool, loaded, partitionSettings(partitions), testsupport.Logger()); failure != nil {
		test.Fatal(failure)
	}
	device, installation := randomNode(random)
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id) values ($1, $2)`, device, installation); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `insert into node_presence (device_id, installation_id) values ($1, $2)`, device, installation); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state) values ($1, $2, $3, 0, 'pending')`, campaign, device, installation); failure != nil {
		test.Fatal(failure)
	}
	devices, installations = append(devices, device), append(installations, installation)
	for _, table := range []string{"nodes", "node_presence", "campaign_nodes"} {
		rows, failure := pool.Query(operation, `select device_id, installation_id, partition from `+table)
		if failure != nil {
			test.Fatal(failure)
		}
		checked := 0
		for rows.Next() {
			var rowDevice, rowInstallation string
			var partition int16
			if failure := rows.Scan(&rowDevice, &rowInstallation, &partition); failure != nil {
				test.Fatal(failure)
			}
			if want := kafka.KeyPartition(rowDevice+"/"+rowInstallation, partitions); int32(partition) != want {
				test.Fatalf("%s row %s/%s is in partition %d, its process results go to %d", table, rowDevice, rowInstallation, partition, want)
			}
			checked++
		}
		if failure := rows.Err(); failure != nil || checked != len(devices) {
			test.Fatalf("%s: checked %d of %d rows (%v)", table, checked, len(devices), failure)
		}
	}
	var leases, epochs int
	var owned bool
	if failure := pool.QueryRow(operation, `select count(*), count(distinct partition) filter (where partition between 0 and $1 - 1 and epoch = 0), bool_or(owner is not null or renewed_at is not null)
		from node_partition_leases`, partitions).Scan(&leases, &epochs, &owned); failure != nil || leases != partitions || epochs != partitions || owned {
		test.Fatalf("node_partition_leases holds %d rows, %d of them partitions 0 to %d at epoch 0, owned %v (%v)", leases, epochs, partitions-1, owned, failure)
	}
	if count, failure := database.NodePartitionCount(operation, pool); failure != nil || count != partitions {
		test.Fatalf("the inventory's partition count reads as %d (%v)", count, failure)
	}
	for index, definition := range map[string]string{
		"nodes_partition":          "(partition, device_id, installation_id)",
		"campaign_nodes_partition": "(campaign_id, partition, state)",
		"campaign_nodes_in_flight": "(partition, deadline_at) WHERE (state = ANY (ARRAY['dispatching'::text, 'dispatched'::text, 'delivered'::text]))",
	} {
		var found string
		if failure := pool.QueryRow(operation, `select indexdef from pg_indexes where indexname = $1`, index).Scan(&found); failure != nil || !strings.HasSuffix(found, definition) {
			test.Errorf("index %s is %q (%v), want one on %s", index, found, failure, definition)
		}
	}
	var presenceIndexes int
	if failure := pool.QueryRow(operation, `select count(*) from pg_indexes where tablename = 'node_presence'`).Scan(&presenceIndexes); failure != nil || presenceIndexes != 1 {
		test.Errorf("node_presence has %d indexes (%v), want only its primary key so its updates stay HOT", presenceIndexes, failure)
	}
	if _, failure := pool.Exec(operation, `update node_presence set partition = 0`); failure == nil {
		test.Error("a row's partition was overwritten; it must follow from its node alone")
	}
	connection, failure := pool.Acquire(operation)
	if failure != nil {
		test.Fatal(failure)
	}
	defer connection.Release()
	if _, failure := connection.Exec(operation, `set search_path = ''`); failure != nil {
		test.Fatal(failure)
	}
	restored, _ := randomNode(random)
	var partition int16
	if failure := connection.QueryRow(operation, `insert into public.nodes (device_id, installation_id) values ($1, $2) returning partition`, restored, installation).Scan(&partition); failure != nil {
		test.Fatalf("a row inserted with an empty search_path, as pg_restore inserts them: %v", failure)
	}
	if want := kafka.KeyPartition(restored+"/"+installation, partitions); int32(partition) != want {
		test.Fatalf("a row inserted with an empty search_path is in partition %d, want %d", partition, want)
	}
	if _, failure := connection.Exec(operation, `reset search_path`); failure != nil {
		test.Fatal(failure)
	}
}

func TestMigrationsAskForASettingOnlyWhenTheyNameIt(test *testing.T) {
	pool, _ := testsupport.EmptyDatabase(test)
	operation := context.Background()
	asked := 0
	settings := map[string]func(context.Context) (string, error){
		database.NodePartitionsSetting: func(context.Context) (string, error) {
			asked++
			return "7", nil
		},
	}
	files := fstest.MapFS{"0001_plain.sql": {Data: []byte("create table plain (id int);")}}
	plain, failure := database.LoadMigrations(files)
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := database.Migrate(operation, pool, plain, settings, testsupport.Logger()); failure != nil || asked != 0 {
		test.Fatalf("a migration that names no setting asked for one %d times (%v)", asked, failure)
	}
	files["0002_named.sql"] = &fstest.MapFile{Data: []byte("create table named as select current_setting('twilight.node_partitions')::integer as partitions;")}
	files["0003_later.sql"] = &fstest.MapFile{Data: []byte("create table later (id int);")}
	both, failure := database.LoadMigrations(files)
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := database.Migrate(operation, pool, both, settings, testsupport.Logger()); failure != nil || asked != 1 {
		test.Fatalf("the setting was asked for %d times (%v), want once for the migration that names it", asked, failure)
	}
	var partitions int
	if failure := pool.QueryRow(operation, `select partitions from named`).Scan(&partitions); failure != nil || partitions != 7 {
		test.Fatalf("the migration saw the setting as %d (%v)", partitions, failure)
	}
}

func TestPartitionCountersMigrationRecountsFromRows(test *testing.T) {
	pool, _ := testsupport.EmptyDatabase(test)
	operation := context.Background()
	before, loaded := migrationsBefore(test, 21)
	if _, failure := database.Migrate(operation, pool, before, testsupport.MigrationSettings, testsupport.Logger()); failure != nil {
		test.Fatal(failure)
	}
	campaign := uuid.New()
	failure := pgx.BeginFunc(operation, pool, func(transaction pgx.Tx) error {
		if failure := database.CreateCampaignPartitions(operation, transaction, campaign); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `insert into campaigns (id, name, status, kind, selector, action, policy, salt, created_by)
			values ($1, 'counted', 'running', 'run_script', 'has(device_id)', '{}', '{}', $2, 'operator')`, campaign, make([]byte, 32)); failure != nil {
			return failure
		}
		if _, failure := transaction.Exec(operation, `insert into campaign_counters (campaign_id, phase, state, count) values ($1, 0, 'pending', 999)`, campaign); failure != nil {
			return failure
		}
		_, failure := transaction.Exec(operation, `insert into campaign_nodes (campaign_id, device_id, installation_id, phase, state)
			select $1, lpad(to_hex(index), 32, '0'), lpad(to_hex(index), 32, 'f'), 0, case when index % 4 = 0 then 'succeeded' else 'pending' end
			from generate_series(1, 200) as index`, campaign)
		return failure
	})
	if failure != nil {
		test.Fatal(failure)
	}
	if _, failure := database.Migrate(operation, pool, loaded, testsupport.MigrationSettings, testsupport.Logger()); failure != nil {
		test.Fatal(failure)
	}
	var mismatched, counted int
	if failure := pool.QueryRow(operation, `select count(*) filter (where counter.count is distinct from actual.count), coalesce(sum(counter.count), 0)
		from (select partition, phase, state, count(*) as count from campaign_nodes where campaign_id = $1 group by partition, phase, state) actual
		full join (select partition, phase, state, count from campaign_counters where campaign_id = $1) counter using (partition, phase, state)`, campaign).Scan(&mismatched, &counted); failure != nil {
		test.Fatal(failure)
	}
	if mismatched != 0 || counted != 200 {
		test.Fatalf("after the migration %d counters differ from the rows and they sum to %d, want 200", mismatched, counted)
	}
}
