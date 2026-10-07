package engine

import (
	"strings"
	"testing"

	"dusk/services/twilight/internal/campaign"
)

func gates() campaign.Gates {
	return campaign.Gates{MinSample: 20, MaxFailureRate: 0.05, MaxSilentRate: 0.1, SilentWindowSeconds: 1800, Breakdown: []string{"os_build", "country"}}
}

func TestGateWaitsForSample(test *testing.T) {
	report := Judge(gates(), nil, Tally{Succeeded: 10, Failed: 0, Eligible: 10}, nil, 1000, false)
	if report.Verdict != VerdictHold || report.Reason != "waiting for sample 10 of 20" || report.MinSample != 20 || report.Required != 20 {
		test.Fatalf("%+v", report)
	}
	report = Judge(gates(), nil, Tally{Succeeded: 30, Eligible: 12}, nil, 1000, false)
	if report.Verdict != VerdictHold || !strings.Contains(report.Reason, "silent-window sample 12 of 20") {
		test.Fatalf("%+v", report)
	}
	report = Judge(gates(), nil, Tally{Succeeded: 30, Eligible: 30}, nil, 1000, false)
	if report.Verdict != VerdictPass {
		test.Fatalf("%+v", report)
	}
}

func TestGateFailsOnAGroupThatHasEnoughSample(test *testing.T) {
	groups := map[string]map[string]Tally{
		"os_build": {
			"22631.4317": {Succeeded: 25, Failed: 41, Eligible: 25},
			"19045.5011": {Succeeded: 400, Failed: 2, Eligible: 400},
			"rare":       {Succeeded: 1, Failed: 3, Eligible: 1},
		},
	}
	overall := Tally{Succeeded: 1000, Failed: 46, Eligible: 1000}
	report := Judge(gates(), nil, overall, groups, 1000, false)
	if report.Verdict != VerdictFail || report.Reason != "failure rate 0.62 in os_build=22631.4317, 41 of 66" || report.FailingGroup != "os_build=22631.4317" {
		test.Fatalf("%+v", report)
	}
	delete(groups["os_build"], "22631.4317")
	report = Judge(gates(), nil, Tally{Succeeded: 1000, Failed: 5, Eligible: 1000}, groups, 1000, false)
	if report.Verdict != VerdictPass {
		test.Fatalf("a failing group below min_sample failed the gate: %+v", report)
	}
}

func TestGateSilentRate(test *testing.T) {
	groups := map[string]map[string]Tally{"country": {"US": {Succeeded: 50, Eligible: 30, Silent: 9}}}
	overall := Tally{Succeeded: 100, Eligible: 80, Silent: 9}
	report := Judge(gates(), nil, overall, groups, 1000, false)
	if report.Verdict != VerdictFail || report.Reason != "silent rate 0.30 in country=US, 9 of 30" {
		test.Fatalf("%+v", report)
	}
	report = Judge(gates(), nil, overall, groups, 1000, true)
	if report.Verdict != VerdictHold || report.Reason != "the online view is degraded" {
		test.Fatalf("a degraded view must hold instead of judging silence: %+v", report)
	}
}

func TestGateFailureRateStillActsWhileDegraded(test *testing.T) {
	report := Judge(gates(), nil, Tally{Succeeded: 50, Failed: 50, Eligible: 50}, nil, 1000, true)
	if report.Verdict != VerdictFail || !strings.Contains(report.Reason, "failure rate 0.50 overall, 50 of 100") {
		test.Fatalf("%+v", report)
	}
}

func TestGateMaxTotalFailures(test *testing.T) {
	limit := 3
	open := gates()
	open.MaxFailureRate = 1
	report := Judge(open, &limit, Tally{Succeeded: 1000, Failed: 4, Eligible: 1000}, nil, 1000, false)
	if report.Verdict != VerdictFail || report.Reason != "4 failures, more than max_total_failures 3" {
		test.Fatalf("%+v", report)
	}
	report = Judge(open, &limit, Tally{Succeeded: 1000, Failed: 3, Eligible: 1000}, nil, 1000, false)
	if report.Verdict != VerdictPass {
		test.Fatalf("%+v", report)
	}
}

func TestGateWithZeroMinimumSample(test *testing.T) {
	open := gates()
	open.MinSample = 0
	if report := Judge(open, nil, Tally{}, nil, 1000, false); report.Verdict != VerdictPass {
		test.Fatalf("an empty phase with min_sample 0: %+v", report)
	}
	open.MaxFailureRate = 0
	if report := Judge(open, nil, Tally{Succeeded: 5, Failed: 1}, nil, 1000, false); report.Verdict != VerdictFail {
		test.Fatalf("max_failure_rate 0 tolerates no failure: %+v", report)
	}
}

func TestGateEmptyPhasePassesOnItsBakeAlone(test *testing.T) {
	if report := Judge(gates(), nil, Tally{}, nil, 0, false); report.Verdict != VerdictPass || report.Required != 0 {
		test.Fatalf("a phase whose buckets hold no node: %+v", report)
	}
}

func TestGateSmallPhaseNeedsEveryNode(test *testing.T) {
	small := gates()
	small.MinSample = 5
	report := Judge(small, nil, Tally{}, nil, 1, false)
	if report.Verdict != VerdictHold || report.Reason != "waiting for sample 0 of 1" || report.MinSample != 5 || report.Required != 1 {
		test.Fatalf("%+v", report)
	}
	report = Judge(small, nil, Tally{Succeeded: 1}, nil, 1, false)
	if report.Verdict != VerdictHold || report.Reason != "waiting for silent-window sample 0 of 1" {
		test.Fatalf("%+v", report)
	}
	if report = Judge(small, nil, Tally{Succeeded: 1, Eligible: 1}, nil, 1, false); report.Verdict != VerdictPass {
		test.Fatalf("%+v", report)
	}
	report = Judge(small, nil, Tally{Failed: 1}, nil, 1, false)
	if report.Verdict != VerdictFail || report.Reason != "failure rate 1.00 overall, 1 of 1" {
		test.Fatalf("%+v", report)
	}
}

func TestGateGroupsKeepMinimumSampleInASmallPhase(test *testing.T) {
	small := gates()
	small.MinSample = 5
	small.MaxSilentRate = 0.3
	groups := map[string]map[string]Tally{"os_build": {"a": {Succeeded: 3, Eligible: 3}, "b": {Succeeded: 1, Eligible: 1, Silent: 1}}}
	if report := Judge(small, nil, Tally{Succeeded: 4, Eligible: 4, Silent: 1}, groups, 4, false); report.Verdict != VerdictPass {
		test.Fatalf("a group below min_sample failed the gate of a phase with fewer rows than min_sample: %+v", report)
	}
}
