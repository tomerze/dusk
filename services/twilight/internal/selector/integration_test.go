//go:build integration

package selector_test

import (
	"context"
	"encoding/json"
	"fmt"
	"math/rand/v2"
	"sort"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5"

	"dusk/services/twilight/internal/selector"
	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

type generatedNode struct {
	device       string
	installation string
	columns      map[string]string
	factsJSON    string
	facts        map[string]any
}

func (node generatedNode) Column(name string) (string, bool) {
	value, found := node.columns[name]
	return value, found
}

func (node generatedNode) Fact(key string) (any, bool) {
	value, found := node.facts[key]
	return value, found
}

var (
	textPool    = []string{"US", "us", "CA", "DE", "debian", "ubuntu", "Debian", "é", "z", "A", "_", "10", "9", "", "a b", "x86_64", "aarch64"}
	versionPool = []string{"0.1.0", "0.2.0", "0.2.0-rc.1", "0.10.0", "1.0.0", "1.0.0+build.3", "2.0.0-alpha", "latest", "v1.0.0", "1.0", "01.0.0"}
	factKeys    = []string{"memory", "name", "flag", "ratio", "version", "list", "nothing", "dusk.os.locale"}
	columnNames = []string{"country", "os_name", "os_version", "os_build", "dusk_version", "hardware_class", "tenant", "locale", "hostname", "impl", "target_arch", "lifecycle", "reported_version", "reported_config_hash"}
	lifecycles  = []string{"enrolled", "active", "quarantined", "retired", "revoked"}
)

func randomFactValue(random *rand.Rand) string {
	switch random.IntN(9) {
	case 0:
		encoded, _ := json.Marshal(textPool[random.IntN(len(textPool))])
		return string(encoded)
	case 1:
		encoded, _ := json.Marshal(versionPool[random.IntN(len(versionPool))])
		return string(encoded)
	case 2:
		return []string{"0", "1", "-1", "4294967296", "8589934592", "0.5", "1.0", "1e3", "-2.25", "12345678901234567890"}[random.IntN(10)]
	case 3:
		return []string{"true", "false"}[random.IntN(2)]
	case 4:
		return "null"
	case 5:
		return `["a", 1]`
	case 6:
		return `{"nested": "US"}`
	default:
		return fmt.Sprintf("%d", random.IntN(10))
	}
}

func generateNodes(test *testing.T, random *rand.Rand, count int) []generatedNode {
	nodes := make([]generatedNode, count)
	for index := range nodes {
		node := generatedNode{
			device:       fmt.Sprintf("%032x", index+1),
			installation: fmt.Sprintf("%032x", random.Uint64()),
			columns:      map[string]string{},
		}
		for _, column := range columnNames {
			if random.IntN(4) == 0 {
				continue
			}
			switch column {
			case "dusk_version", "reported_version":
				node.columns[column] = versionPool[random.IntN(len(versionPool))]
			case "lifecycle":
				node.columns[column] = lifecycles[random.IntN(len(lifecycles))]
			default:
				node.columns[column] = textPool[random.IntN(len(textPool))]
			}
		}
		if _, found := node.columns["lifecycle"]; !found {
			node.columns["lifecycle"] = "enrolled"
		}
		var parts []string
		for _, key := range factKeys {
			if random.IntN(3) == 0 {
				continue
			}
			encodedKey, _ := json.Marshal(key)
			parts = append(parts, string(encodedKey)+":"+randomFactValue(random))
		}
		node.factsJSON = "{" + strings.Join(parts, ",") + "}"
		decoder := json.NewDecoder(strings.NewReader(node.factsJSON))
		decoder.UseNumber()
		if failure := decoder.Decode(&node.facts); failure != nil {
			test.Fatalf("decode %s: %v", node.factsJSON, failure)
		}
		nodes[index] = node
	}
	return nodes
}

func randomLiteral(random *rand.Rand, kind int) string {
	switch kind {
	case 0:
		encoded, _ := json.Marshal(textPool[random.IntN(len(textPool))])
		return string(encoded)
	case 1:
		encoded, _ := json.Marshal(versionPool[random.IntN(4)+random.IntN(3)])
		return string(encoded)
	case 2:
		return []string{"0", "1", "-1", "4294967296", "0.5", "1e3", "1000", "12345678901234567890", "-2.25"}[random.IntN(9)]
	default:
		return []string{"true", "false"}[random.IntN(2)]
	}
}

func randomComparison(random *rand.Rand) string {
	operators := []string{"==", "!=", "<", "<=", ">", ">=", "in", "not in"}
	operator := operators[random.IntN(len(operators))]
	var field string
	literalKind := 0
	switch random.IntN(4) {
	case 0:
		field = columnNames[random.IntN(len(columnNames))]
		if (field == "dusk_version" || field == "reported_version") && random.IntN(2) == 0 {
			literalKind = 1
		}
	case 1:
		field = "semver(" + []string{"dusk_version", "reported_version", "os_version", `facts["version"]`, `facts["name"]`}[random.IntN(5)] + ")"
		literalKind = 1
	default:
		encoded, _ := json.Marshal(factKeys[random.IntN(len(factKeys))])
		field = "facts[" + string(encoded) + "]"
		literalKind = random.IntN(4)
	}
	if operator == "in" || operator == "not in" {
		count := 1 + random.IntN(3)
		values := make([]string, count)
		for index := range values {
			values[index] = randomLiteral(random, literalKind)
		}
		return field + " " + operator + " [" + strings.Join(values, ", ") + "]"
	}
	return field + " " + operator + " " + randomLiteral(random, literalKind)
}

func randomSelector(random *rand.Rand, depth int) string {
	if depth <= 0 {
		if random.IntN(6) == 0 {
			encoded, _ := json.Marshal(factKeys[random.IntN(len(factKeys))])
			return `has(facts[` + string(encoded) + `])`
		}
		if random.IntN(10) == 0 {
			return "has(" + columnNames[random.IntN(len(columnNames))] + ")"
		}
		return randomComparison(random)
	}
	switch random.IntN(5) {
	case 0:
		return randomSelector(random, depth-1) + " and " + randomSelector(random, depth-1)
	case 1:
		return "(" + randomSelector(random, depth-1) + " or " + randomSelector(random, depth-1) + ")"
	case 2:
		return "not " + randomSelector(random, depth-1)
	case 3:
		return "not (" + randomSelector(random, depth-1) + ")"
	default:
		return randomSelector(random, 0)
	}
}

func TestSelectorAgreesWithPostgres(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	random := rand.New(rand.NewPCG(20261007, 1))
	nodes := generateNodes(test, random, 400)
	batch := &pgx.Batch{}
	for _, node := range nodes {
		arguments := []any{node.device, node.installation, node.factsJSON}
		columnList := "device_id, installation_id, facts"
		placeholders := "$1, $2, $3::jsonb"
		for _, column := range columnNames {
			if value, found := node.columns[column]; found {
				arguments = append(arguments, value)
				columnList += ", " + column
				placeholders += fmt.Sprintf(", $%d", len(arguments))
			}
		}
		batch.Queue("insert into nodes ("+columnList+") values ("+placeholders+")", arguments...)
	}
	if failure := pool.SendBatch(operation, batch).Close(); failure != nil {
		test.Fatalf("insert nodes: %v", failure)
	}
	checked, nonTrivial := 0, 0
	for checked < 1500 {
		source := randomSelector(random, 1+random.IntN(3))
		parsed, failure := selector.Parse(source)
		if failure != nil {
			continue
		}
		checked++
		compiled := parsed.Compile("n", 1)
		arguments := make([]any, len(compiled.Parameters))
		for index, parameter := range compiled.Parameters {
			arguments[index] = parameter
		}
		rows, failure := pool.Query(operation, "select n.device_id from nodes n where "+compiled.SQL+" order by n.device_id", arguments...)
		if failure != nil {
			test.Fatalf("selector %q compiled to %s: %v", source, compiled.SQL, failure)
		}
		fromDatabase, failure := pgx.CollectRows(rows, pgx.RowTo[string])
		if failure != nil {
			test.Fatalf("selector %q compiled to %s: %v", source, compiled.SQL, failure)
		}
		var inMemory []string
		for _, node := range nodes {
			if parsed.Matches(node) {
				inMemory = append(inMemory, node.device)
			}
		}
		sort.Strings(inMemory)
		if len(inMemory) > 0 && len(inMemory) < len(nodes) {
			nonTrivial++
		}
		if strings.Join(inMemory, ",") != strings.Join(fromDatabase, ",") {
			missing, extra := difference(inMemory, fromDatabase)
			test.Fatalf("selector %q disagrees\nsql: %s\nparameters: %q\nonly in memory: %v\nonly in postgres: %v\nexample node: %+v",
				source, compiled.SQL, compiled.Parameters, missing, extra, exampleNode(nodes, append(missing, extra...)))
		}
	}
	if nonTrivial < checked/3 {
		test.Fatalf("only %d of %d selectors matched a proper subset; the generator is too weak", nonTrivial, checked)
	}
	test.Logf("%d selectors agree, %d of them matched a proper subset", checked, nonTrivial)
}

func difference(left, right []string) ([]string, []string) {
	inRight := map[string]bool{}
	for _, value := range right {
		inRight[value] = true
	}
	inLeft := map[string]bool{}
	var onlyLeft, onlyRight []string
	for _, value := range left {
		inLeft[value] = true
		if !inRight[value] {
			onlyLeft = append(onlyLeft, value)
		}
	}
	for _, value := range right {
		if !inLeft[value] {
			onlyRight = append(onlyRight, value)
		}
	}
	return onlyLeft, onlyRight
}

func exampleNode(nodes []generatedNode, devices []string) any {
	if len(devices) == 0 {
		return nil
	}
	for _, node := range nodes {
		if node.device == devices[0] {
			return struct {
				Columns map[string]string
				Facts   string
			}{node.columns, node.factsJSON}
		}
	}
	return nil
}

func TestFactQueriesUseTheGinIndex(test *testing.T) {
	pool, _ := testsupport.Database(test)
	operation := context.Background()
	if _, failure := pool.Exec(operation, `insert into nodes (device_id, installation_id, facts)
		select lpad(to_hex(series), 32, '0'), lpad(to_hex(series), 32, '0'),
		       jsonb_build_object('dusk.os.name', case when hashint4(series) % 97 = 0 then 'debian' else 'other' end, 'index', series)
		from generate_series(1, 20000) as series`); failure != nil {
		test.Fatal(failure)
	}
	if _, failure := pool.Exec(operation, `vacuum analyze nodes`); failure != nil {
		test.Fatal(failure)
	}
	for _, source := range []string{`facts["dusk.os.name"] == "debian"`, `has(facts["rare"])`, `facts["dusk.os.name"] in ["debian", "arch"]`} {
		parsed, failure := selector.Parse(source)
		if failure != nil {
			test.Fatal(failure)
		}
		compiled := parsed.Compile("n", 1)
		arguments := make([]any, len(compiled.Parameters))
		for index, parameter := range compiled.Parameters {
			arguments[index] = parameter
		}
		connection, failure := pool.Acquire(operation)
		if failure != nil {
			test.Fatal(failure)
		}
		rows, failure := connection.Query(operation, "explain select device_id from nodes n where "+compiled.SQL, arguments...)
		if failure != nil {
			connection.Release()
			test.Fatal(failure)
		}
		lines, failure := pgx.CollectRows(rows, pgx.RowTo[string])
		connection.Release()
		if failure != nil {
			test.Fatal(failure)
		}
		plan := strings.Join(lines, "\n")
		if !strings.Contains(plan, "nodes_facts") {
			test.Errorf("%q does not use the GIN index:\n%s", source, plan)
		}
	}
}
