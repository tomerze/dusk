package main

import (
	"bytes"
	"strings"
	"testing"
)

func passingEvidence() evidence {
	first := nodeProcess{"00000000000000000000000000000001", "a1", "70000"}
	second := nodeProcess{"00000000000000000000000000000002", "a2", "80000"}
	duplicate := nodeProcess{"00000000000000000000000000000003", "a3", "90000"}
	return evidence{
		campaigns: []campaignRecord{{id: "c", name: "run/0", status: "completed", selected: 3, reached: 3}},
		intended:  map[nodeProcess]bool{first: true, second: true, duplicate: true},
		results: map[nodeProcess]resultCounts{
			first:     {started: 1, finals: 1, outcomes: 1, statuses: "succeeded"},
			second:    {finals: 1, statuses: "unreachable"},
			duplicate: {started: 1, finals: 2, outcomes: 1, statuses: "duplicate,succeeded"},
		},
		ledger: map[nodeProcess]ledgerCounts{first: {started: 1, repeated: 1}, duplicate: {started: 1, repeated: 1}},
		unsent: map[nodeProcess]bool{},
	}
}

func failing(checks []check) map[string]check {
	failed := map[string]check{}
	for _, current := range checks {
		if !current.passed {
			failed[current.name] = current
		}
	}
	return failed
}

func TestARunWhereEveryProcessRanOnceWithAResultPasses(test *testing.T) {
	checks := judge(passingEvidence())
	if failed := failing(checks); len(failed) > 0 {
		test.Fatalf("failed checks: %+v", failed)
	}
	var output bytes.Buffer
	if verdict := printReport(&output, "run", passingEvidence(), checks); verdict != 0 || !strings.HasSuffix(output.String(), "verdict: PASS\n") {
		test.Fatalf("verdict %d:\n%s", verdict, output.String())
	}
}

func TestAnIntendedProcessThatNeverReachedDawnIsNotALostResult(test *testing.T) {
	gathered := passingEvidence()
	unsent := nodeProcess{"00000000000000000000000000000004", "a4", "100000"}
	gathered.intended[unsent] = true
	gathered.unsent[unsent] = true
	checks := judge(gathered)
	if failed := failing(checks); len(failed) > 0 {
		test.Fatalf("failed checks: %+v", failed)
	}
	if !strings.Contains(checks[2].summary, "3 of 3, leaving out 1") {
		test.Errorf("summary %q", checks[2].summary)
	}
	started := nodeProcess{"00000000000000000000000000000001", "a1", "70000"}
	delete(gathered.results, started)
	gathered.unsent[started] = true
	failed := failing(judge(gathered))
	if _, present := failed["every intended process has a result"]; !present {
		test.Errorf("a process the ledger saw start passed as never sent: failed %+v", failed)
	}
}

func TestARunWaitsWhileARowIsOnItsWay(test *testing.T) {
	gathered := passingEvidence()
	if !gathered.finished() || len(gathered.inFlight()) != 0 {
		test.Fatalf("finished %t, in flight %q", gathered.finished(), gathered.inFlight())
	}
	gathered.campaigns[0].inFlight = 2
	if reasons := gathered.inFlight(); len(reasons) != 1 || reasons[0] != "2 rows of the run are still on their way" {
		test.Errorf("in flight %q", reasons)
	}
	gathered.campaigns[0].status = "running"
	if gathered.finished() {
		test.Error("a running campaign counts as finished")
	}
}

func TestEachKindOfFailureFailsItsOwnCheck(test *testing.T) {
	missing := nodeProcess{"00000000000000000000000000000001", "a1", "70000"}
	for _, current := range []struct {
		name   string
		change func(*evidence)
		failed string
	}{
		{"a running campaign", func(gathered *evidence) { gathered.campaigns[0].status = "running" }, "every campaign of the run finished"},
		{"a campaign that reached fewer nodes than it selects", func(gathered *evidence) { gathered.campaigns[0].reached = 2 }, "every campaign reached the nodes it selected while it ran"},
		{"no result", func(gathered *evidence) { delete(gathered.results, missing); delete(gathered.ledger, missing) }, "every intended process has a result"},
		{"a started process whose dispatch never reported", func(gathered *evidence) {
			gathered.results[missing] = resultCounts{started: 1, finals: 1, statuses: "duplicate"}
		}, "every started process reported its outcome"},
		{"a started process whose stream broke while the script ran", func(gathered *evidence) {
			gathered.results[missing] = resultCounts{started: 1, finals: 1, statuses: "running"}
		}, "every started process reported its outcome"},
		{"a process the ledger saw start and dawn never reported", func(gathered *evidence) {
			gathered.results[missing] = resultCounts{finals: 1, statuses: "error"}
		}, "every started process reported its outcome"},
		{"two started results", func(gathered *evidence) {
			gathered.results[missing] = resultCounts{started: 2, finals: 2, outcomes: 2, statuses: "succeeded"}
		}, "no pid was started twice on its node, by dawn's started results"},
		{"two Dusk.process results", func(gathered *evidence) {
			gathered.ledger[missing] = ledgerCounts{started: 2, repeated: 1}
		}, "no pid's Dusk.process was answered ok twice on its node, by the ledger"},
		{"one script forwarded twice", func(gathered *evidence) {
			gathered.ledger[missing] = ledgerCounts{started: 1, repeated: 2}
		}, "no command was forwarded twice under one pid, by the ledger"},
	} {
		gathered := passingEvidence()
		current.change(&gathered)
		failed := failing(judge(gathered))
		if len(failed) != 1 {
			test.Errorf("%s: failed checks %+v, want only %q", current.name, failed, current.failed)
			continue
		}
		found, present := failed[current.failed]
		if !present {
			test.Errorf("%s: failed %+v, want %q", current.name, failed, current.failed)
			continue
		}
		if len(found.examples) != 1 {
			test.Errorf("%s: examples %q", current.name, found.examples)
		}
		var output bytes.Buffer
		if verdict := printReport(&output, "run", gathered, judge(gathered)); verdict != 1 || !strings.HasSuffix(output.String(), "verdict: FAIL\n") {
			test.Errorf("%s: verdict %d:\n%s", current.name, verdict, output.String())
		}
	}
}
