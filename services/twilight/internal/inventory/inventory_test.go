package inventory

import (
	"encoding/json"
	"strings"
	"testing"

	"dusk/services/twilight/internal/selector"
)

func facts(test *testing.T, text string) map[string]json.RawMessage {
	test.Helper()
	decoded := map[string]json.RawMessage{}
	if failure := json.Unmarshal([]byte(text), &decoded); failure != nil {
		test.Fatal(failure)
	}
	return decoded
}

func value(pointer *string) string {
	if pointer == nil {
		return "<null>"
	}
	return *pointer
}

func TestNormalizeLinux(test *testing.T) {
	columns, cleaned := Normalize(facts(test, `{
		"dusk.device.id": "secret", "dusk.version": "0.2.0", "dusk.target.os": "linux", "dusk.target.arch": "x86_64",
		"dusk.os.linux.os_release.id": "debian", "dusk.os.linux.os_release.version_id": "13", "dusk.os.nix.uname.release": "6.12.48-amd64",
		"dusk.device.cores": 6, "dusk.device.memory_bytes": 16624349184, "dusk.os.locale": "en_US.UTF-8", "dusk.os.time_zone": "America/New_York",
		"dusk.hostname": "kiosk-0042", "dusk.impl": "nix"}`))
	if _, leaked := cleaned[DeviceIDFact]; leaked {
		test.Fatal("dusk.device.id kept")
	}
	want := map[string]string{
		"os_name": "debian", "os_version": "13", "os_build": "6.12.48-amd64", "dusk_version": "0.2.0", "hardware_class": "x86_64-4c-16g",
		"locale": "en_US.UTF-8", "country": "US", "hostname": "kiosk-0042", "impl": "nix", "target_arch": "x86_64",
	}
	got := map[string]string{
		"os_name": value(columns.OSName), "os_version": value(columns.OSVersion), "os_build": value(columns.OSBuild), "dusk_version": value(columns.DuskVersion),
		"hardware_class": value(columns.HardwareClass), "locale": value(columns.Locale), "country": value(columns.Country), "hostname": value(columns.Hostname),
		"impl": value(columns.Impl), "target_arch": value(columns.TargetArch),
	}
	for column, expected := range want {
		if got[column] != expected {
			test.Errorf("%s = %s, want %s", column, got[column], expected)
		}
	}
}

func TestNormalizeOtherPlatforms(test *testing.T) {
	cases := []struct {
		facts                string
		name, version, build string
		hardware, country    string
	}{
		{`{"dusk.target.os": "windows", "dusk.target.arch": "x86_64", "dusk.os.windows.edition": "Professional", "dusk.os.windows.display_version": "23H2",
			"dusk.os.windows.build_number": 22631, "dusk.os.windows.revision": 4317, "dusk.device.cores": 16, "dusk.device.memory_bytes": 34359738368,
			"dusk.os.time_zone": "Pacific Standard Time"}`, "Professional", "23H2", "22631.4317", "x86_64-16c-32g", "<null>"},
		{`{"dusk.target.os": "android", "dusk.target.arch": "aarch64", "dusk.os.android.brand": "google", "dusk.os.android.release": "15",
			"dusk.os.android.incremental": "12345678", "dusk.os.time_zone": "Asia/Calcutta"}`, "google", "15", "12345678", "aarch64", "IN"},
		{`{"dusk.target.os": "macos", "dusk.target.arch": "aarch64", "dusk.os.macos.product_version": "15.6", "dusk.os.macos.build_version": "24G84",
			"dusk.device.cores": 1, "dusk.device.memory_bytes": 536870912}`, "macos", "15.6", "24G84", "aarch64-1c-1g", "<null>"},
		{`{"dusk.target.os": "freebsd", "dusk.os.nix.uname.release": "14.3-RELEASE"}`, "freebsd", "<null>", "14.3-RELEASE", "<null>", "<null>"},
		{`{"dusk.os.time_zone": "Etc/UTC"}`, "<null>", "<null>", "<null>", "<null>", "<null>"},
	}
	for _, testCase := range cases {
		columns, _ := Normalize(facts(test, testCase.facts))
		got := []string{value(columns.OSName), value(columns.OSVersion), value(columns.OSBuild), value(columns.HardwareClass), value(columns.Country)}
		want := []string{testCase.name, testCase.version, testCase.build, testCase.hardware, testCase.country}
		for index := range got {
			if got[index] != want[index] {
				test.Errorf("%s: got %v, want %v", testCase.facts, got, want)
				break
			}
		}
	}
}

func TestZoneTable(test *testing.T) {
	for zone, country := range map[string]string{"Europe/Berlin": "DE", "US/Pacific": "US", "Asia/Tokyo": "JP", "America/Sao_Paulo": "BR"} {
		if got, found := CountryOfZone(zone); !found || got != country {
			test.Errorf("%s -> %s %v, want %s", zone, got, found, country)
		}
	}
	if _, found := CountryOfZone("UTC"); found {
		test.Error("UTC has no country")
	}
	if len(zoneCountries) < 400 {
		test.Fatalf("only %d zones", len(zoneCountries))
	}
}

func TestNodeIsASelectorRecord(test *testing.T) {
	country, version := "US", "0.1.9"
	node := &Node{DeviceID: "d", InstallationID: "i", Lifecycle: "active", Country: &country, DuskVersion: &version,
		FactsJSON: json.RawMessage(`{"dusk.device.memory_bytes": 8589934592, "flag": true}`)}
	parsed, failure := selector.Parse(`country == "US" and dusk_version < "0.2.0" and facts["dusk.device.memory_bytes"] >= 4294967296 and facts["flag"] == true and not lifecycle == "quarantined" and not has(os_name)`)
	if failure != nil {
		test.Fatal(failure)
	}
	if !parsed.Matches(node) {
		test.Fatal("node does not match")
	}
	breakdown := node.Breakdown([]string{"country", "os_build"})
	if breakdown["country"] != "US" || breakdown["os_build"] != "" {
		test.Fatalf("breakdown %v", breakdown)
	}
	for _, column := range selector.Columns() {
		node.Column(column)
	}
}

func TestOrderAndCursors(test *testing.T) {
	for _, text := range []string{"", "device_id", "-device_id", "hostname", "-dusk_version", "reported_version"} {
		if _, failure := ParseOrder(text); failure != nil {
			test.Errorf("%q: %v", text, failure)
		}
	}
	for _, text := range []string{"facts", "-", "--hostname", "last_seen_at", "hostname desc"} {
		if _, failure := ParseOrder(text); failure == nil {
			test.Errorf("%q was accepted", text)
		}
	}
	descending, _ := ParseOrder("-hostname")
	if descending.String() != "-hostname" || !descending.Descending {
		test.Fatalf("%+v", descending)
	}
	hostname := "web-1"
	node := Node{DeviceID: strings.Repeat("a", 32), InstallationID: strings.Repeat("b", 32), sortValue: &hostname}
	decoded, failure := descending.decode(descending.CursorAfter(node))
	if failure != nil || *decoded.Value != "web-1" || decoded.DeviceID != node.DeviceID || decoded.InstallationID != node.InstallationID {
		test.Fatalf("%+v %v", decoded, failure)
	}
	ascending, _ := ParseOrder("hostname")
	for _, text := range []string{"!!", "", ascending.CursorAfter(node)} {
		if _, failure := descending.decode(text); failure != ErrMalformedCursor {
			test.Errorf("cursor %q: %v", text, failure)
		}
	}
	_, condition, ordering, values := ascending.sql(nil, &cursor{Value: &hostname, DeviceID: "d", InstallationID: "i"})
	if condition != ` and (n.hostname > $3::text or (n.hostname = $3::text and (n.device_id, n.installation_id) > ($1, $2)) or n.hostname is null)` ||
		ordering != ` order by n.hostname nulls last, n.device_id, n.installation_id` || len(values) != 3 {
		test.Fatalf("%s | %s | %v", condition, ordering, values)
	}
}
