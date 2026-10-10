package campaign

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"math/big"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

var now = time.Date(2026, 10, 7, 12, 0, 0, 0, time.UTC)

func validDefinition() Definition {
	definition := Definition{
		Name:     "upgrade",
		Selector: `country == "US"`,
		Action:   Action{Kind: KindEnsureVersion, Version: "0.2.0", Script: "upgrade 0.2.0"},
		Policy: Policy{
			Rate:               Rate{PerSecond: 50, Burst: 100},
			Phases:             []Phase{{Name: "canary", Percent: 1, BakeSeconds: 3600}, {Name: "half", Percent: 50, BakeSeconds: 3600}, {Name: "all", Percent: 100, BakeSeconds: 3600}},
			Gates:              Gates{MinSample: 20, MaxFailureRate: 0.05, MaxSilentRate: 0.05, SilentWindowSeconds: 1800, Breakdown: []string{"os_build"}},
			NodeTimeoutSeconds: 900,
		},
	}
	definition.Normalize()
	return definition
}

func TestDefinitionValidation(test *testing.T) {
	if failure := validDefinition().Validate(now); failure != nil {
		test.Fatalf("valid definition rejected: %v", failure)
	}
	cases := []struct {
		field  string
		mutate func(*Definition)
	}{
		{"name", func(definition *Definition) { definition.Name = "" }},
		{"selector", func(definition *Definition) { definition.Selector = "" }},
		{"selector", func(definition *Definition) { definition.Selector = "country = 1" }},
		{"tenant", func(definition *Definition) { tenant := "Bad Tenant"; definition.Tenant = &tenant }},
		{"action.kind", func(definition *Definition) { definition.Action.Kind = "reboot" }},
		{"action.script", func(definition *Definition) { definition.Action.Script = "  " }},
		{"action.version", func(definition *Definition) { definition.Action.Version = "" }},
		{"action.version_key", func(definition *Definition) { definition.Action.VersionKey = "dusk.device.id" }},
		{"action.collect_files", func(definition *Definition) { definition.Action.CollectFiles = []string{"/var/log/x"} }},
		{"policy.rate.per_second", func(definition *Definition) { definition.Policy.Rate.PerSecond = -1 }},
		{"policy.rate.per_second", func(definition *Definition) { definition.Policy.Rate.PerSecond = math.NaN() }},
		{"policy.phases[1].percent", func(definition *Definition) { definition.Policy.Phases[1].Percent = 1 }},
		{"policy.phases", func(definition *Definition) { definition.Policy.Phases[2].Percent = 90 }},
		{"policy.phases[1].name", func(definition *Definition) { definition.Policy.Phases[1].Name = "canary" }},
		{"policy.phases[0].bake_seconds", func(definition *Definition) { definition.Policy.Phases[0].BakeSeconds = 600 }},
		{"policy.phases[0].bake_seconds", func(definition *Definition) {
			definition.Policy.Gates.SilentWindowSeconds = 7200
		}},
		{"policy.gates.min_sample", func(definition *Definition) { definition.Policy.Gates.MinSample = 0 }},
		{"policy.gates.max_failure_rate", func(definition *Definition) { definition.Policy.Gates.MaxFailureRate = 1.5 }},
		{"policy.gates.breakdown", func(definition *Definition) { definition.Policy.Gates.Breakdown = []string{"hostname"} }},
		{"policy.abort.on_gate_failure", func(definition *Definition) { definition.Policy.Abort.OnGateFailure = "ignore" }},
		{"policy.retry.max_backoff_seconds", func(definition *Definition) { definition.Policy.Retry.MaxBackoffSeconds = 1 }},
		{"policy.deadline", func(definition *Definition) { past := now.Add(-time.Hour); definition.Policy.Deadline = &past }},
	}
	for _, testCase := range cases {
		definition := validDefinition()
		definition.Policy.Phases = append([]Phase(nil), definition.Policy.Phases...)
		testCase.mutate(&definition)
		failure := definition.Validate(now)
		var validation *ValidationError
		if !errors.As(failure, &validation) || validation.Field != testCase.field {
			test.Errorf("want a %s error, got %v", testCase.field, failure)
		}
	}
}

func TestActionValidation(test *testing.T) {
	valid := []Action{
		{Kind: KindRunScript, Script: "ps", CollectFiles: []string{"/var/log/syslog", `C:\logs\x.log`}, StreamLogs: &StreamLogs{Level: "info", DurationSeconds: 60}},
		{Kind: KindQuarantine},
		{Kind: KindQuarantine, Script: "isolate", RequireScriptSuccess: true},
		{Kind: KindEnsureConfig, ConfigHash: "abc", Script: "apply"},
	}
	for _, action := range valid {
		action.Normalize()
		if failure := action.Validate(); failure != nil {
			test.Errorf("%+v rejected: %v", action, failure)
		}
	}
	invalidActions := []Action{
		{Kind: KindRunScript, Script: "ps", CollectFiles: []string{"relative/path"}},
		{Kind: KindRunScript, Script: "ps", CollectFiles: []string{"/a", "/a"}},
		{Kind: KindRunScript, Script: "ps", CollectFiles: strings.Split(strings.Repeat("/x,", 17), ",")[:17]},
		{Kind: KindRunScript, Script: "ps", StreamLogs: &StreamLogs{Level: "loud", DurationSeconds: 60}},
		{Kind: KindRunScript, Script: "ps", StreamLogs: &StreamLogs{Level: "info", DurationSeconds: 0}},
		{Kind: KindRunScript, Script: strings.Repeat("x", MaximumScriptBytes+1)},
		{Kind: KindQuarantine, RequireScriptSuccess: true},
		{Kind: KindRunScript, Script: "ps", RequireScriptSuccess: true},
		{Kind: KindRunScript, Script: "ps", Version: "1"},
		{Kind: KindEnsureConfig, Script: "apply"},
	}
	for _, action := range invalidActions {
		action.Normalize()
		if failure := action.Validate(); failure == nil {
			test.Errorf("%+v accepted", action)
		}
	}
}

func TestPolicyDefaults(test *testing.T) {
	var oneShot, converging Policy
	oneShot.Gates = DefaultGates()
	oneShot.ApplyDefaults(KindRunScript)
	converging.ApplyDefaults(KindEnsureVersion)
	if oneShot.Retry.MaxAttempts != 1 || converging.Retry.MaxAttempts != 3 {
		test.Fatalf("max attempts %d and %d", oneShot.Retry.MaxAttempts, converging.Retry.MaxAttempts)
	}
	if len(oneShot.Phases) != 1 || oneShot.Phases[0].Percent != 100 {
		test.Fatalf("default phases %+v", oneShot.Phases)
	}
	if failure := oneShot.Validate(now); failure != nil {
		test.Fatalf("defaulted policy invalid: %v", failure)
	}
	var decoded Policy
	if failure := json.Unmarshal([]byte(`{}`), &decoded); failure != nil {
		test.Fatal(failure)
	}
	decoded.ApplyDefaults(KindEnsureVersion)
	if !reflect.DeepEqual(decoded.Gates, DefaultGates()) || decoded.Phases[0].BakeSeconds != 900 {
		test.Fatalf("a policy without gates: %+v, phases %+v", decoded.Gates, decoded.Phases)
	}
	if failure := decoded.Validate(now); failure != nil {
		test.Fatalf("a policy decoded from {} is invalid: %v", failure)
	}
	decoded.Gates.SilentWindowSeconds, decoded.Phases = 1200, nil
	decoded.ApplyDefaults(KindEnsureVersion)
	if decoded.Phases[0].BakeSeconds != 1200 {
		test.Fatalf("the default phase bakes for %d, not the silent window", decoded.Phases[0].BakeSeconds)
	}
}

func TestGateKeysLeftOutTakeTheirDefaults(test *testing.T) {
	cases := map[string]Gates{
		`{"gates": {"min_sample": 5}}`: {MinSample: 5, MaxFailureRate: 0.05, MaxSilentRate: 0.05, SilentWindowSeconds: 600, Breakdown: BreakdownDimensions},
		`{"gates": {"max_failure_rate": 0, "max_silent_rate": 0, "silent_window_seconds": 0}}`: {MinSample: 1, Breakdown: BreakdownDimensions},
		`{"gates": {"breakdown": []}}`: {MinSample: 1, MaxFailureRate: 0.05, MaxSilentRate: 0.05, SilentWindowSeconds: 600, Breakdown: []string{}},
		`{"gates": null}`:              DefaultGates(),
	}
	for document, want := range cases {
		var decoded Policy
		if failure := json.Unmarshal([]byte(document), &decoded); failure != nil {
			test.Fatal(failure)
		}
		decoded.ApplyDefaults(KindRunScript)
		if !reflect.DeepEqual(decoded.Gates, want) {
			test.Errorf("%s: got %+v, want %+v", document, decoded.Gates, want)
		}
		encoded, _ := json.Marshal(decoded)
		var again Policy
		if failure := json.Unmarshal(encoded, &again); failure != nil || !reflect.DeepEqual(again, decoded) {
			test.Errorf("%s does not survive a round trip: %+v", document, again)
		}
	}
}

func TestBackoffSchedule(test *testing.T) {
	policy := Policy{Retry: Retry{MaxAttempts: 5, InitialBackoffSeconds: 10, MaxBackoffSeconds: 60, Multiplier: 3}}
	cases := []struct {
		failures int
		random   float64
		want     time.Duration
	}{
		{1, 1, 10 * time.Second},
		{1, 0.5, 5 * time.Second},
		{1, 0, 0},
		{2, 1, 30 * time.Second},
		{3, 1, 60 * time.Second},
		{9, 1, 60 * time.Second},
		{9, 0.25, 15 * time.Second},
	}
	for _, testCase := range cases {
		if got := policy.Backoff(testCase.failures, testCase.random); got != testCase.want {
			test.Errorf("backoff(%d, %v) = %v, want %v", testCase.failures, testCase.random, got, testCase.want)
		}
	}
}

func TestBucketIsStableAndUniform(test *testing.T) {
	salt := []byte("0123456789abcdef0123456789abcdef")
	first := Bucket("3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", salt)
	if again := Bucket("3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", salt); again != first {
		test.Fatal("bucket is not deterministic")
	}
	if first != 0x02ef331f8d0448fe {
		test.Fatalf("bucket %#x changed; phase membership of running campaigns depends on it", first)
	}
	policy := Policy{Phases: []Phase{{Name: "canary", Percent: 1}, {Name: "ten", Percent: 10}, {Name: "half", Percent: 50}, {Name: "all", Percent: 100}}}
	thresholds := policy.Thresholds()
	counts := make([]int, len(thresholds))
	total := 200000
	for index := range total {
		counts[thresholds.PhaseOf(Bucket(fmt.Sprintf("%032x", index), "installation", salt))]++
	}
	want := []float64{0.01, 0.09, 0.40, 0.50}
	for phase, count := range counts {
		share := float64(count) / float64(total)
		if math.Abs(share-want[phase]) > 0.005 {
			test.Errorf("phase %d holds %.4f of nodes, want %.2f", phase, share, want[phase])
		}
	}
}

func TestThresholdsAreExact(test *testing.T) {
	thresholds := Policy{Phases: []Phase{{Name: "a", Percent: 25}, {Name: "b", Percent: 100}}}.Thresholds()
	quarter := new(big.Int).Lsh(big.NewInt(1), 62)
	if thresholds[0].Cmp(quarter) != 0 {
		test.Fatalf("25%% threshold %v, want %v", thresholds[0], quarter)
	}
	if thresholds.PhaseOf(quarter.Uint64()-1) != 0 || thresholds.PhaseOf(quarter.Uint64()) != 1 {
		test.Fatal("bucket at the boundary landed in the wrong phase")
	}
	if thresholds.PhaseOf(math.MaxUint64) != 1 || !thresholds.Open(math.MaxUint64, 1) || thresholds.Open(math.MaxUint64, 0) {
		test.Fatal("the last bucket is not in the 100% phase")
	}
}

func TestPidDerivationVectors(test *testing.T) {
	campaign := uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e8f")
	device, installation := "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
	cases := []struct {
		campaign     uuid.UUID
		device       string
		installation string
		attempt      int
		want         Pid
	}{
		{campaign, device, installation, 1, 12808937078074471924},
		{campaign, device, installation, 2, 17352987455937552665},
		{uuid.Nil, strings.Repeat("0", 32), strings.Repeat("0", 32), 1, 1715447980763406553},
	}
	for _, testCase := range cases {
		if got := DerivePid(testCase.campaign, testCase.device, testCase.installation, testCase.attempt); got != testCase.want {
			test.Errorf("DerivePid(%s, %s, %s, %d) = %d, want %d", testCase.campaign, testCase.device, testCase.installation, testCase.attempt, got, testCase.want)
		}
	}
	if DerivePid(campaign, device, installation, 1) == DerivePid(campaign, installation, device, 1) {
		test.Error("swapping the device and installation ids gave the same pid")
	}
}

func TestReservedPidsAreRemappedByRehashingWithACounter(test *testing.T) {
	input := pidInput(uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e8f"), "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13", "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70", 1)
	first, second := Pid(12808937078074471924), Pid(13099267363847254876)
	if got := derive(input, func(pid Pid) bool { return pid == first }); got != second {
		test.Fatalf("a reserved first hash remapped to %d, want %d", got, second)
	}
	third := Pid(16004681274841385295)
	if got := derive(input, func(pid Pid) bool { return pid == first || pid == second }); got != third {
		test.Fatalf("two reserved hashes in a row remapped to %d, want %d", got, third)
	}
	for _, pid := range []Pid{0, 1, 65535, DefaultShellPid} {
		if !Reserved(pid) {
			test.Errorf("pid %d is not reserved", pid)
		}
	}
	for _, pid := range []Pid{65536, DefaultShellPid - 1, DefaultShellPid + 1, math.MaxUint64} {
		if Reserved(pid) {
			test.Errorf("pid %d is reserved", pid)
		}
	}
	if DefaultShellPid != 0xf2efce60e8c425d0 {
		test.Fatal("DefaultShellPid is not sh.capnp's defaultPid")
	}
}

func TestRandomPidsAreNeverReserved(test *testing.T) {
	seen := map[Pid]bool{}
	for range 1000 {
		pid, failure := RandomPid()
		if failure != nil || Reserved(pid) || seen[pid] {
			test.Fatalf("random pid %d (%v)", pid, failure)
		}
		seen[pid] = true
	}
}

func TestPidText(test *testing.T) {
	for _, text := range []string{"65536", "18446744073709551615", "17352987455937552665"} {
		pid, failure := ParsePid(text)
		if failure != nil || pid.String() != text {
			test.Errorf("ParsePid(%q) = %d, %v", text, pid, failure)
		}
	}
	for _, text := range []string{"", "-1", "+5", "0123", "18446744073709551616", "1e3", " 7"} {
		if _, failure := ParsePid(text); !errors.Is(failure, ErrMalformedPid) {
			test.Errorf("ParsePid(%q) accepted it", text)
		}
	}
	encoded, failure := json.Marshal(struct {
		Pid  Pid `json:"pid"`
		None Pid `json:"none"`
	}{Pid: 17352987455937552665})
	if failure != nil || string(encoded) != `{"pid":"17352987455937552665","none":null}` {
		test.Fatalf("pids encode as %s (%v)", encoded, failure)
	}
	var decoded struct {
		Pid  Pid `json:"pid"`
		None Pid `json:"none"`
	}
	if failure := json.Unmarshal(encoded, &decoded); failure != nil || decoded.Pid != 17352987455937552665 || decoded.None != 0 {
		test.Fatalf("pids decode as %+v (%v)", decoded, failure)
	}
	if failure := json.Unmarshal([]byte(`{"pid":17352987455937552665}`), &decoded); failure == nil {
		test.Fatal("a pid as a JSON number was accepted")
	}
}

func TestStatusTransitions(test *testing.T) {
	allowed := map[Status]map[Transition]Status{
		StatusDraft:     {TransitionStart: StatusRunning, TransitionAbort: StatusAborted},
		StatusRunning:   {TransitionPause: StatusPaused, TransitionAbort: StatusAborted, TransitionFail: StatusFailed, TransitionComplete: StatusCompleted},
		StatusPaused:    {TransitionResume: StatusRunning, TransitionAbort: StatusAborted, TransitionFail: StatusFailed, TransitionComplete: StatusCompleted},
		StatusCompleted: {TransitionArchive: StatusArchived},
		StatusAborted:   {TransitionArchive: StatusArchived},
		StatusFailed:    {TransitionArchive: StatusArchived},
		StatusArchived:  {},
	}
	all := []Transition{TransitionStart, TransitionPause, TransitionResume, TransitionAbort, TransitionFail, TransitionComplete, TransitionArchive}
	for status, permitted := range allowed {
		for _, transition := range all {
			next, failure := NextStatus(status, transition)
			want, ok := permitted[transition]
			if ok && (failure != nil || next != want) {
				test.Errorf("%s --%s--> %s, %v; want %s", status, transition, next, failure, want)
			}
			if !ok && !errors.Is(failure, ErrInvalidTransition) {
				test.Errorf("%s --%s--> %s allowed", status, transition, next)
			}
		}
	}
}

func TestBakeAccrualExcludesPauses(test *testing.T) {
	started := now.Add(-2 * time.Hour)
	paused := now.Add(-30 * time.Minute)
	campaign := &Campaign{Status: StatusPaused, PhaseStartedAt: &started, PausedAt: &paused, PhasePausedSeconds: 1800,
		Policy: Policy{Phases: []Phase{{Name: "a", Percent: 100, BakeSeconds: 3600}}}}
	if accrued := campaign.BakeAccrued(now); accrued != time.Hour {
		test.Fatalf("accrued %v, want 1h", accrued)
	}
	if !campaign.BakeComplete(now) {
		test.Fatal("bake not complete")
	}
	campaign.Status = StatusRunning
	campaign.PhasePausedSeconds = 3600
	if accrued := campaign.BakeAccrued(now); accrued != time.Hour {
		test.Fatalf("accrued %v after resume, want 1h", accrued)
	}
}
