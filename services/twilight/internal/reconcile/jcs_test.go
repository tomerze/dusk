package reconcile

import (
	"crypto/sha256"
	"encoding/hex"
	"testing"
)

func TestCanonicalizeMatchesTheRFC8785Example(test *testing.T) {
	input := `{
  "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
  "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
  "literals": [null, true, false]
}`
	want := `{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}`
	canonical, failure := Canonicalize([]byte(input))
	if failure != nil {
		test.Fatal(failure)
	}
	if string(canonical) != want {
		test.Fatalf("got  %s\nwant %s", canonical, want)
	}
}

func TestCanonicalizeSortsKeysByUTF16CodeUnits(test *testing.T) {
	input := `{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh","1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis"}`
	want := "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u0080\":\"Control\",\"\u00f6\":\"Latin Small Letter O With Diaeresis\",\"\u20ac\":\"Euro Sign\",\"\U0001F600\":\"Emoji: Grinning Face\",\"\ufb33\":\"Hebrew Letter Dalet With Dagesh\"}"
	canonical, failure := Canonicalize([]byte(input))
	if failure != nil {
		test.Fatal(failure)
	}
	if string(canonical) != want {
		test.Fatalf("got  %s\nwant %s", canonical, want)
	}
}

func TestFormatNumberFollowsECMAScript(test *testing.T) {
	for input, want := range map[string]string{
		"0":                      "0",
		"-0":                     "0",
		"1":                      "1",
		"-1.5":                   "-1.5",
		"1e21":                   "1e+21",
		"1e20":                   "100000000000000000000",
		"123456789012345680000":  "123456789012345680000",
		"0.000001":               "0.000001",
		"0.0000001":              "1e-7",
		"1791278043512408":       "1791278043512408",
		"9007199254740992":       "9007199254740992",
		"4.940656458412465e-324": "5e-324",
		"1.7976931348623157e308": "1.7976931348623157e+308",
		"295147905179352830000":  "295147905179352830000",
	} {
		got, failure := FormatNumber(input)
		if failure != nil || got != want {
			test.Errorf("%s: got %q (%v), want %q", input, got, failure, want)
		}
	}
	for _, input := range []string{"1e400", "-1e400", "NaN"} {
		if _, failure := FormatNumber(input); failure == nil {
			test.Errorf("%s accepted", input)
		}
	}
}

func TestCanonicalizeRefusesWhatJCSCannotRepresent(test *testing.T) {
	for _, input := range []string{`{"a":1} {"b":2}`, `{"a":1e400}`, "\"\xff\"", `{"a":`} {
		if _, failure := Canonicalize([]byte(input)); failure == nil {
			test.Errorf("%q accepted", input)
		}
	}
}

func TestEntryHashLeavesOutOnlyTheHashMember(test *testing.T) {
	entry := `{"sequence": 7, "hash": "ignored", "instance": "nightfall-0", "previous_hash": "00", "event_detail": {"hash": "kept"}}`
	got, failure := EntryHash([]byte(entry))
	if failure != nil {
		test.Fatal(failure)
	}
	digest := sha256.Sum256([]byte(`{"event_detail":{"hash":"kept"},"instance":"nightfall-0","previous_hash":"00","sequence":7}`))
	if want := hex.EncodeToString(digest[:]); got != want {
		test.Fatalf("got %s, want %s", got, want)
	}
	if _, failure := EntryHash([]byte(`[1]`)); failure == nil {
		test.Fatal("an array was hashed as an entry")
	}
}
