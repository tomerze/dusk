package engine

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"sync"
	"testing"
	"time"

	"dusk/services/twilight/internal/nodestate"
)

type scriptedResult struct {
	report  nodestate.Report
	failure error
}

type scriptedChecker struct {
	mutex   sync.Mutex
	results []scriptedResult
	calls   []time.Time
}

func (checker *scriptedChecker) Check(context.Context) (nodestate.Report, error) {
	checker.mutex.Lock()
	defer checker.mutex.Unlock()
	checker.calls = append(checker.calls, time.Now())
	result := checker.results[min(len(checker.calls), len(checker.results))-1]
	return result.report, result.failure
}

func (checker *scriptedChecker) times() []time.Time {
	checker.mutex.Lock()
	defer checker.mutex.Unlock()
	return append([]time.Time(nil), checker.calls...)
}

func TestNodeStatesAreCheckedAtElectionSoonAgainAfterAFailureAndThenEveryPeriod(test *testing.T) {
	checker := &scriptedChecker{results: []scriptedResult{
		{failure: errors.New("the topic could not be described")},
		{report: nodestate.Report{Checked: true, Incomplete: []int32{0}}, failure: errors.New("the rebuild failed")},
		{report: nodestate.Report{Checked: true, Rebuilt: true}},
		{report: nodestate.Report{Skipped: true}},
	}}
	pace := checkPace{period: 400 * time.Millisecond, base: 10 * time.Millisecond, ceiling: 40 * time.Millisecond}
	operation, cancel := context.WithCancel(context.Background())
	started := time.Now()
	stopped := make(chan struct{})
	go func() {
		checkNodeStates(operation, checker, slog.New(slog.NewJSONHandler(io.Discard, nil)), 1, pace)
		close(stopped)
	}()
	deadline := time.Now().Add(5 * time.Second)
	for len(checker.times()) < 5 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	cancel()
	<-stopped
	calls := checker.times()
	if len(calls) < 5 {
		test.Fatalf("%d checks in 5 s", len(calls))
	}
	if first := calls[0].Sub(started); first > 100*time.Millisecond {
		test.Errorf("the first check came %s after the election", first)
	}
	for index := 1; index <= 2; index++ {
		if gap := calls[index].Sub(calls[index-1]); gap > pace.ceiling+50*time.Millisecond {
			test.Errorf("check %d came %s after a failed one, past the retry ceiling of %s", index, gap, pace.ceiling)
		}
	}
	for index := 3; index <= 4; index++ {
		if gap := calls[index].Sub(calls[index-1]); gap < pace.period-50*time.Millisecond {
			test.Errorf("check %d came %s after a good one, before the period of %s", index, gap, pace.period)
		}
	}
}

func TestRetriesAfterFailuresStayWithinTheirCeiling(test *testing.T) {
	pace := checkPace{period: time.Minute, base: 5 * time.Second, ceiling: 5 * time.Minute}
	for failures := 1; failures < 40; failures++ {
		for range 50 {
			if wait := pace.retry(failures); wait <= 0 || wait > pace.ceiling {
				test.Fatalf("retry %s after %d failures", wait, failures)
			}
		}
	}
	if wait := pace.retry(1); wait > 10*time.Second {
		test.Errorf("the first retry waits %s, past twice the base", wait)
	}
}
