package selector

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

type record struct {
	columns map[string]string
	facts   map[string]any
}

func (row record) Column(name string) (string, bool) {
	value, found := row.columns[name]
	return value, found
}

func (row record) Fact(key string) (any, bool) {
	value, found := row.facts[key]
	return value, found
}

func factsFrom(test *testing.T, text string) map[string]any {
	test.Helper()
	decoder := json.NewDecoder(strings.NewReader(text))
	decoder.UseNumber()
	facts := map[string]any{}
	if failure := decoder.Decode(&facts); failure != nil {
		test.Fatalf("decode facts: %v", failure)
	}
	return facts
}

func mustParse(test *testing.T, source string) *Selector {
	test.Helper()
	parsed, failure := Parse(source)
	if failure != nil {
		test.Fatalf("parse %q: %v", source, failure)
	}
	return parsed
}

func TestParseErrorsCarryPositions(test *testing.T) {
	cases := []struct {
		source   string
		position int
		message  string
	}{
		{`country = "US"`, 8, `use "==" to compare`},
		{`country == 'US'`, 11, "strings use double quotes"},
		{`country == "US`, 11, "this string is not closed"},
		{`colour == "red"`, 0, `unknown field "colour"; use facts["colour"] for a fact`},
		{`country == 5`, 11, "country compares with strings, not numbers"},
		{`country == "US" and`, 19, "expected a field name or facts[\"key\"], found the end of the selector"},
		{`country in ["US", 5]`, 18, "a list holds values of one type; this list starts with strings"},
		{`dusk_version < "latest"`, 15, `"latest" is not a semantic version like "1.2.3"`},
		{`facts["flag"] < true`, 0, "booleans can only be compared with ==, !=, in and not in"},
		{`facts[""] == 1`, 6, "a fact name cannot be empty"},
		{`country == "US" country`, 16, `expected "and", "or" or the end of the selector, found "country"`},
		{`facts["x"] == 01`, 14, `"01" is not a number`},
		{`facts["k"] > 1e1000001`, 13, "a number's exponent is between -1000 and 1000"},
		{`facts["k"] > 1E-1001`, 13, "a number's exponent is between -1000 and 1000"},
		{`facts["k"] > 1e99999999999999999999`, 13, "a number's exponent is between -1000 and 1000"},
		{`facts["k"] > ` + strings.Repeat("9", 101), 13, "a number has at most 100 characters"},
		{`(country == "US"`, 16, `expected ")", found the end of the selector`},
		{`hostname == "é" and colour == "x"`, 20, `unknown field "colour"; use facts["colour"] for a fact`},
		{`semver(os_version) == 10`, 22, `semver(os_version) compares with version strings like "1.2.3"`},
		{`country == "\u0000"`, 11, "strings cannot contain U+0000"},
		{`not`, 3, "expected a field name or facts[\"key\"], found the end of the selector"},
		{`country ! "US"`, 8, `use "not" to negate, or "!=" to compare`},
	}
	for _, testCase := range cases {
		_, failure := Parse(testCase.source)
		var selectorError *Error
		if !errors.As(failure, &selectorError) {
			test.Errorf("%q: expected a selector error, got %v", testCase.source, failure)
			continue
		}
		if selectorError.Position != testCase.position || selectorError.Message != testCase.message {
			test.Errorf("%q: got (%d, %q), want (%d, %q)", testCase.source, selectorError.Position, selectorError.Message, testCase.position, testCase.message)
		}
		if selectorError.End < selectorError.Position {
			test.Errorf("%q: end %d before position %d", testCase.source, selectorError.End, selectorError.Position)
		}
	}
}

func TestParseLimits(test *testing.T) {
	deep := strings.Repeat("not ", maximumDepth+1) + `country == "US"`
	if _, failure := Parse(deep); failure == nil || !strings.Contains(failure.Error(), "nests deeper") {
		test.Fatalf("deep selector: %v", failure)
	}
	long := strings.Repeat(" ", MaximumLength+1)
	if _, failure := Parse(long); failure == nil {
		test.Fatal("over-long selector parsed")
	}
	values := make([]string, maximumListLength+1)
	for index := range values {
		values[index] = `"x"`
	}
	if _, failure := Parse(`country in [` + strings.Join(values, ",") + `]`); failure == nil {
		test.Fatal("over-long list parsed")
	}
	if _, failure := Parse("country == \"\xff\""); failure == nil {
		test.Fatal("invalid UTF-8 parsed")
	}
}

func TestEmptySelectorMatchesAll(test *testing.T) {
	parsed := mustParse(test, "  \n")
	if !parsed.MatchesAll() || !parsed.Matches(record{}) {
		test.Fatal("empty selector does not match everything")
	}
	if compiled := parsed.Compile("n", 1); compiled.SQL != "true" || len(compiled.Parameters) != 0 {
		test.Fatalf("empty selector compiled to %+v", compiled)
	}
}

func TestPrecedence(test *testing.T) {
	parsed := mustParse(test, `country == "US" or country == "CA" and os_name == "debian"`)
	root, isOr := parsed.Root.(*Or)
	if !isOr {
		test.Fatalf("root is %T, want *Or", parsed.Root)
	}
	if _, isAnd := root.Right.(*And); !isAnd {
		test.Fatalf("right of or is %T, want *And", root.Right)
	}
	parsed = mustParse(test, `not country == "US" and os_name == "debian"`)
	if _, isAnd := parsed.Root.(*And); !isAnd {
		test.Fatalf("root is %T, want *And", parsed.Root)
	}
}

func TestEvaluateNullSemantics(test *testing.T) {
	empty := record{columns: map[string]string{}, facts: map[string]any{}}
	quarantined := record{columns: map[string]string{"lifecycle": "quarantined"}, facts: map[string]any{}}
	cases := []struct {
		source string
		row    record
		want   bool
	}{
		{`country == "US"`, empty, false},
		{`country != "US"`, empty, false},
		{`not country == "US"`, empty, true},
		{`country in ["US"]`, empty, false},
		{`country not in ["US"]`, empty, false},
		{`not lifecycle == "quarantined"`, empty, true},
		{`not lifecycle == "quarantined"`, quarantined, false},
		{`facts["k"] == 1`, empty, false},
		{`facts["k"] != 1`, empty, false},
		{`not facts["k"] == 1`, empty, true},
		{`has(facts["k"])`, empty, false},
		{`has(country)`, empty, false},
		{`not has(country)`, empty, true},
		{`dusk_version < "1.0.0"`, empty, false},
		{`not dusk_version < "1.0.0"`, empty, true},
	}
	for _, testCase := range cases {
		if got := mustParse(test, testCase.source).Matches(testCase.row); got != testCase.want {
			test.Errorf("%q on %+v: got %v, want %v", testCase.source, testCase.row, got, testCase.want)
		}
	}
}

func TestEvaluateFacts(test *testing.T) {
	row := record{
		columns: map[string]string{"country": "US", "os_name": "debian", "dusk_version": "0.1.9", "reported_version": "1.0.0-rc.1"},
		facts:   factsFrom(test, `{"memory": 8589934592, "ratio": 0.5, "name": "edge", "flag": true, "nothing": null, "list": ["a"], "huge": 1e400, "version": "2.0.0"}`),
	}
	cases := []struct {
		source string
		want   bool
	}{
		{`facts["memory"] >= 4294967296`, true},
		{`facts["memory"] == 8589934592.0`, true},
		{`facts["memory"] == 8.589934592e9`, true},
		{`facts["memory"] < 4294967296`, false},
		{`facts["memory"] == "8589934592"`, false},
		{`facts["memory"] != "8589934592"`, false},
		{`facts["memory"] != 1`, true},
		{`facts["ratio"] > 0.25`, true},
		{`facts["name"] == "edge"`, true},
		{`facts["name"] > "Edge"`, true},
		{`facts["name"] < 5`, false},
		{`facts["flag"] == true`, true},
		{`facts["flag"] != false`, true},
		{`facts["flag"] == 1`, false},
		{`facts["flag"] in [true]`, true},
		{`facts["flag"] not in [false]`, true},
		{`facts["flag"] not in ["true"]`, false},
		{`has(facts["nothing"])`, true},
		{`facts["nothing"] == "x"`, false},
		{`facts["nothing"] != "x"`, false},
		{`facts["list"] == "a"`, false},
		{`facts["list"] != "a"`, false},
		{`facts["huge"] > 1e399`, true},
		{`facts["huge"] < 1e1000`, true},
		{`facts["memory"] > -1e1000`, true},
		{`facts["ratio"] > 1e-1000`, true},
		{`facts["name"] in ["core", "edge"]`, true},
		{`facts["name"] not in ["core", "edge"]`, false},
		{`facts["name"] not in ["core"]`, true},
		{`facts["memory"] in [1, 8589934592]`, true},
		{`facts["memory"] not in [1, 2]`, true},
		{`semver(facts["version"]) >= "2.0.0"`, true},
		{`semver(facts["version"]) == "2.0.0+build.7"`, true},
		{`semver(facts["version"]) in ["1.0.0", "2.0.0"]`, true},
		{`semver(facts["version"]) != "2.0.0"`, false},
		{`semver(facts["version"]) > "2.0.0-rc.1"`, true},
		{`semver(facts["name"]) > "0.0.0"`, false},
		{`semver(facts["memory"]) > "0.0.0"`, false},
		{`not semver(facts["name"]) > "0.0.0"`, true},
		{`dusk_version < "0.2.0"`, true},
		{`dusk_version < "0.10.0"`, true},
		{`dusk_version > "0.1.9-alpha"`, true},
		{`dusk_version == "0.1.9"`, true},
		{`dusk_version in ["0.1.9", "0.2.0"]`, true},
		{`reported_version < "1.0.0"`, true},
		{`reported_version >= "1.0.0-rc.2"`, true},
		{`semver(reported_version) == "1.0.0-beta"`, true},
		{`semver(os_name) == "1.0.0"`, false},
		{`country == "US" and os_name in ["debian", "ubuntu"] and dusk_version < "0.2.0" and facts["memory"] >= 4294967296 and not lifecycle == "quarantined"`, true},
		{`country > "UA"`, true},
		{`country < "us"`, true},
	}
	for _, testCase := range cases {
		if got := mustParse(test, testCase.source).Matches(row); got != testCase.want {
			test.Errorf("%q: got %v, want %v", testCase.source, got, testCase.want)
		}
	}
}

func TestSemverKeys(test *testing.T) {
	cases := []struct {
		left, right string
		order       int
		valid       bool
	}{
		{"1.2.3", "1.2.3", 0, true},
		{"1.2.3", "1.2.4", -1, true},
		{"1.10.0", "1.9.9", 1, true},
		{"1.0.0-rc.1", "1.0.0", -1, true},
		{"1.0.0-rc.1", "1.0.0-rc.2", 0, true},
		{"1.0.0+build.5", "1.0.0", 0, true},
		{"99999999999999999999999.0.0", "1.0.0", 1, true},
		{"01.0.0", "1.0.0", 0, false},
		{"1.0", "1.0.0", 0, false},
		{"v1.0.0", "1.0.0", 0, false},
		{"1.0.0-", "1.0.0", 0, false},
		{"1.0.0-01", "1.0.0", 0, false},
	}
	for _, testCase := range cases {
		order, valid := CompareVersions(testCase.left, testCase.right)
		if valid != testCase.valid || (valid && order != testCase.order) {
			test.Errorf("%s vs %s: got (%d, %v), want (%d, %v)", testCase.left, testCase.right, order, valid, testCase.order, testCase.valid)
		}
	}
}

func TestCompileIsParameterized(test *testing.T) {
	parsed := mustParse(test, `country == "US'; drop table nodes; --" and facts["a\"b"] >= 4 and has(facts["c"]) and dusk_version < "0.2.0"`)
	compiled := parsed.Compile("n", 3)
	if strings.Contains(compiled.SQL, "drop table") || strings.Contains(compiled.SQL, `a"b`) {
		test.Fatalf("literal leaked into SQL: %s", compiled.SQL)
	}
	want := `(((n.country = $3::text) and (case when jsonb_typeof(n.facts -> $4::text) = 'number' then (n.facts ->> $4::text)::numeric >= $5::numeric end)) and (n.facts ? $6::text)) and (semver_key(n.dusk_version) < semver_key($7::text)))`
	if compiled.SQL != "("+want {
		test.Fatalf("compiled SQL:\n%s\nwant:\n(%s", compiled.SQL, want)
	}
	parameters := []string{"US'; drop table nodes; --", `a"b`, "4", "c", "0.2.0"}
	if strings.Join(compiled.Parameters, "|") != strings.Join(parameters, "|") {
		test.Fatalf("parameters %q, want %q", compiled.Parameters, parameters)
	}
}

func TestCompileFactEqualityUsesContainment(test *testing.T) {
	compiled := mustParse(test, `facts["os"] == "debian" or facts["cores"] in [4, 8]`).Compile("", 1)
	want := `((facts @> $1::jsonb) or ((facts @> $2::jsonb) or (facts @> $3::jsonb)))`
	if compiled.SQL != want {
		test.Fatalf("got %s, want %s", compiled.SQL, want)
	}
	parameters := []string{`{"os":"debian"}`, `{"cores":4}`, `{"cores":8}`}
	if strings.Join(compiled.Parameters, "|") != strings.Join(parameters, "|") {
		test.Fatalf("parameters %q, want %q", compiled.Parameters, parameters)
	}
}

func TestCompileSemverFactEquality(test *testing.T) {
	compiled := mustParse(test, `semver(facts["app"]) == "1.0.0"`).Compile("n", 1)
	want := `(case when jsonb_typeof(n.facts -> $1::text) = 'string' then (semver_key(n.facts ->> $1::text) = semver_key($2::text)) end)`
	if compiled.SQL != want {
		test.Fatalf("got %s, want %s", compiled.SQL, want)
	}
}

func TestCompileNegationIsTwoValued(test *testing.T) {
	compiled := mustParse(test, `not (country == "US" or not has(locale))`).Compile("n", 1)
	want := `((((n.country = $1::text) or (((n.locale is not null)) is not true))) is not true)`
	if compiled.SQL != want {
		test.Fatalf("got %s, want %s", compiled.SQL, want)
	}
}
