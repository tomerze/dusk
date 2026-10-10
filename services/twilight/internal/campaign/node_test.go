package campaign

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
)

const (
	device       = "3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13"
	installation = "a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70"
)

func testCampaign(kind Kind) *Campaign {
	campaign := &Campaign{ID: uuid.MustParse("0192f3a4-5b6c-7d8e-9f01-23456789abcd"), Kind: kind,
		Action: Action{Kind: kind, Script: "x", Version: "1.0.0", VersionKey: DefaultVersionKey, ConfigHash: "h"}}
	campaign.Policy.ApplyDefaults(kind)
	campaign.Policy.Retry.MaxAttempts = 2
	return campaign
}

func dispatched(test *testing.T, campaign *Campaign) Row {
	test.Helper()
	row := NewRow(campaign.ID, device, installation, 0, StatePending)
	row, resend := row.Dispatch(campaign.ID, Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 100}, now, now.Add(time.Hour), map[string]string{"os_build": "22631"})
	if resend || row.State != StateDispatching || row.Pid != DerivePid(campaign.ID, device, installation, 1) {
		test.Fatalf("dispatch: %+v resend=%v", row, resend)
	}
	row, accepted := row.Accepted(row.Pid)
	if !accepted || row.State != StateDispatched {
		test.Fatalf("accepted: %+v", row)
	}
	return row
}

func apply(test *testing.T, campaign *Campaign, row Row, status ResultStatus, delivered bool) (Row, Effect, bool) {
	test.Helper()
	return row.ApplyResult(campaign, Result{Pid: row.Pid, Status: status, Delivered: delivered, Error: "boom"}, now, 0.5)
}

func TestDispatchKeepsThePidForResends(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	row := dispatched(test, campaign)
	original := row.Pid
	row, changed := row.Undelivered("unreachable", "no route", now, time.Minute)
	if !changed || row.State != StatePending || row.Pid != original || row.NextAttemptAt == nil || !row.NextAttemptAt.Equal(now.Add(time.Minute)) {
		test.Fatalf("undelivered: %+v", row)
	}
	if row.Due(now) || !row.Due(now.Add(time.Minute)) {
		test.Fatal("undelivered row due at the wrong time")
	}
	if !row.DueOnNewSession(Session{Epoch: 101}, now) || row.DueOnNewSession(Session{Epoch: 100}, now) {
		test.Fatal("a newer session must make an undelivered row due at once")
	}
	row, resend := row.Dispatch(campaign.ID, Session{NamespaceID: "aa", Epoch: 101}, now, now.Add(time.Hour), map[string]string{"os_build": "other"})
	if !resend || row.Pid != original || row.Attempt != 1 || row.Breakdown["os_build"] != "22631" {
		test.Fatalf("resend: %+v", row)
	}
	if _, accepted := row.Accepted(original + 1); accepted {
		test.Fatal("accepted a different pid")
	}
}

func TestStartedAndRunningResults(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	row := dispatched(test, campaign)
	row, _, changed := apply(test, campaign, row, ResultStarted, true)
	if !changed || row.State != StateDelivered || row.DeliveredAt == nil {
		test.Fatalf("started: %+v", row)
	}
	if _, _, again := apply(test, campaign, row, ResultStarted, true); again {
		test.Fatal("a repeated started changed the row")
	}
	later := now.Add(10 * time.Hour)
	row, _, changed = row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultRunning, Delivered: true}, later, 0.5)
	if !changed || row.State != StateDelivered || !row.DeadlineAt.Equal(later.Add(900*time.Second)) {
		test.Fatalf("running: %+v", row)
	}
}

func TestRunningKeepsADuplicateDelivered(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	row := dispatched(test, campaign)
	far := now.Add(3 * time.Hour)
	row.DeadlineAt = &far
	row, _, changed := apply(test, campaign, row, ResultRunning, true)
	if !changed || row.State != StateDelivered || row.DeliveredAt == nil || !row.DeadlineAt.Equal(far) || row.LastStatus != string(ResultRunning) {
		test.Fatalf("running shortened the deadline or did not deliver the row: %+v", row)
	}
	expired, changed := row.Expire(campaign, far, time.Minute)
	if !changed || expired.State != StateUnknown || expired.FinishedAt == nil {
		test.Fatalf("a one-shot still running at its deadline: %+v", expired)
	}
	later := far.Add(time.Minute)
	revived, _, changed := expired.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultRunning, Delivered: true}, later, 0.5)
	if !changed || revived.State != StateDelivered || revived.FinishedAt != nil || !revived.DeadlineAt.Equal(later.Add(900*time.Second)) {
		test.Fatalf("running after unknown: %+v", revived)
	}
	pending, _ := dispatched(test, campaign).Undelivered("unreachable", "no route", now, time.Minute)
	resent, _, changed := pending.ApplyResult(campaign, Result{Pid: pending.Pid, Status: ResultRunning, Delivered: true}, now, 0.5)
	if !changed || resent.State != StateDelivered || resent.NextAttemptAt != nil {
		test.Fatalf("running on a pending row: %+v", resent)
	}
}

func TestReportedStateDecidesEnsureRows(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	satisfied, unsatisfied := true, false
	for _, status := range []ResultStatus{ResultRunning, ResultStarted, ResultFailed, ResultTimedOut, ResultSucceeded, ResultDuplicate, ResultEnded, ResultError} {
		row := dispatched(test, campaign)
		done, effect, changed := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: status, Delivered: true, Satisfied: &satisfied}, now, 0.5)
		if !changed || done.State != StateSucceeded || effect != EffectNone || done.LastStatus != string(status) {
			test.Errorf("%s reporting the desired state: %+v", status, done)
		}
	}
	row := dispatched(test, campaign)
	running, _, _ := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultRunning, Delivered: true, Satisfied: &unsatisfied}, now, 0.5)
	if running.State != StateDelivered {
		test.Fatalf("running with a reported state that differs: %+v", running)
	}
	expired, _ := running.Expire(campaign, running.DeadlineAt.Add(time.Second), time.Minute)
	if expired.State != StatePending || expired.Pid != row.Pid {
		test.Fatalf("an ensure row still running at its deadline is resent with its pid: %+v", expired)
	}
	denied, effect, _ := dispatched(test, campaign).ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultDenied, Delivered: true, Satisfied: &satisfied}, now, 0.5)
	if denied.State != StateFailed || effect != EffectPauseForDenied {
		test.Fatalf("a denied process is a failure whatever the node reports: %+v", denied)
	}
}

func TestResultsForOtherPidsAreIgnored(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	row := dispatched(test, campaign)
	if _, _, changed := row.ApplyResult(campaign, Result{Pid: row.Pid + 1, Status: ResultSucceeded}, now, 0); changed {
		test.Fatal("result for another pid applied")
	}
	if _, _, changed := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultReaped}, now, 0); changed {
		test.Fatal("a reap of the row's pid changed the row")
	}
	if _, _, changed := NewRow(campaign.ID, device, installation, 0, StatePending).ApplyResult(campaign, Result{Status: ResultSucceeded}, now, 0); changed {
		test.Fatal("a result without a pid applied to a row never dispatched")
	}
	succeeded, _, _ := apply(test, campaign, row, ResultSucceeded, true)
	if _, _, changed := apply(test, campaign, succeeded, ResultFailed, true); changed {
		test.Fatal("result changed a terminal row")
	}
}

func TestOneShotOutcomes(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	cases := []struct {
		status    ResultStatus
		delivered bool
		state     NodeState
		effect    Effect
	}{
		{ResultSucceeded, true, StateSucceeded, EffectNone},
		{ResultFailed, true, StateBackoff, EffectNone},
		{ResultError, true, StateBackoff, EffectNone},
		{ResultDuplicate, false, StateDelivered, EffectNone},
		{ResultRunning, true, StateDelivered, EffectNone},
		{ResultDenied, true, StateFailed, EffectPauseForDenied},
		{ResultUnreachable, false, StatePending, EffectNone},
		{ResultTimedOut, false, StatePending, EffectNone},
		{ResultError, false, StatePending, EffectNone},
		{ResultTimedOut, true, StateUnknown, EffectNone},
		{ResultEnded, true, StateUnknown, EffectNone},
		{ResultAlreadySatisfied, false, StateSucceeded, EffectNone},
	}
	for _, testCase := range cases {
		row := dispatched(test, campaign)
		original := row.Pid
		row, effect, changed := apply(test, campaign, row, testCase.status, testCase.delivered)
		if !changed || row.State != testCase.state || effect != testCase.effect {
			test.Errorf("%s delivered=%v: state %s effect %v changed %v, want %s %v", testCase.status, testCase.delivered, row.State, effect, changed, testCase.state, testCase.effect)
		}
		switch row.State {
		case StatePending:
			if row.Pid != original || row.Attempt != 1 || row.Failures != 0 {
				test.Errorf("%s: undelivered row lost its pid or attempt: %+v", testCase.status, row)
			}
		case StateBackoff:
			if row.Pid != 0 || row.Attempt != 2 || row.Failures != 1 || row.NextAttemptAt == nil || row.LastStatus != string(testCase.status) {
				test.Errorf("%s: backoff row %+v", testCase.status, row)
			}
		case StateDelivered:
			if row.Pid != original || row.DeliveredAt == nil || row.DeadlineAt.Before(now.Add(campaign.ProcessTimeout())) || row.LastStatus != string(testCase.status) {
				test.Errorf("%s: a pid already on the node must be held delivered and running: %+v", testCase.status, row)
			}
		}
	}
}

func TestRetriesAreBounded(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	row := dispatched(test, campaign)
	row, _, _ = apply(test, campaign, row, ResultFailed, true)
	if row.State != StateBackoff || row.Due(now) || !row.Due(row.NextAttemptAt.Add(0)) {
		test.Fatalf("first failure: %+v", row)
	}
	row, resend := row.Dispatch(campaign.ID, Session{Epoch: 200}, *row.NextAttemptAt, now.Add(2*time.Hour), nil)
	if resend || row.Pid != DerivePid(campaign.ID, device, installation, 2) || row.Pid == DerivePid(campaign.ID, device, installation, 1) {
		test.Fatalf("second attempt reused a pid: %+v", row)
	}
	row, _, _ = row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultFailed, Delivered: true}, now, 0)
	if row.State != StateFailed || row.Failures != 2 || row.FinishedAt == nil {
		test.Fatalf("second failure: %+v", row)
	}
	row, changed := row.Retry(now)
	if !changed || row.State != StatePending || row.Attempt != 3 || row.Failures != 0 || row.Pid != 0 || row.NextAttemptAt == nil || !row.NextAttemptAt.Equal(now) || !row.Due(now) {
		test.Fatalf("operator retry: %+v", row)
	}
}

func TestAnUndeliveredWordNeverUndoesADelivery(test *testing.T) {
	for _, kind := range []Kind{KindRunScript, KindEnsureVersion} {
		target := testCampaign(kind)
		started, _, _ := apply(test, target, dispatched(test, target), ResultStarted, true)
		duplicate, _, _ := apply(test, target, dispatched(test, target), ResultDuplicate, false)
		for _, delivered := range []Row{started, duplicate} {
			for _, status := range []ResultStatus{ResultUnreachable, ResultTimedOut, ResultError} {
				next, _, changed := apply(test, target, delivered, status, false)
				if changed || next.State != StateDelivered {
					test.Errorf("%s: %s with delivered=false moved a delivered row to %s", kind, status, next.State)
				}
			}
			next, _, changed := apply(test, target, delivered, ResultUnreachable, true)
			if changed || next.State != StateDelivered {
				test.Errorf("%s: unreachable with delivered=true moved a delivered row to %s", kind, next.State)
			}
		}
	}
}

func TestConvergingOutcomesVerifyAgainstReportedState(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	satisfied, unsatisfied := true, false
	row := dispatched(test, campaign)
	succeeded, _, _ := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultSucceeded, Delivered: true, Satisfied: &satisfied}, now, 0)
	if succeeded.State != StateSucceeded {
		test.Fatalf("satisfied success: %+v", succeeded)
	}
	verifying, _, _ := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultSucceeded, Delivered: true, Satisfied: &unsatisfied}, now, 0)
	if verifying.State != StateVerifying {
		test.Fatalf("unsatisfied success: %+v", verifying)
	}
	if _, changed := verifying.Verify(false, row.NamespaceID, campaign.Policy, now, 0); changed {
		test.Fatal("verification judged in the same session")
	}
	if verified, changed := verifying.Verify(true, row.NamespaceID, campaign.Policy, now, 0); !changed || verified.State != StateSucceeded {
		test.Fatalf("satisfied facts: %+v", verified)
	}
	failed, changed := verifying.Verify(false, "ffffffffffffffff", campaign.Policy, now, 0)
	if !changed || failed.State != StateBackoff || failed.Failures != 1 || failed.LastStatus != string(ResultFailed) {
		test.Fatalf("unsatisfied facts in a new session: %+v", failed)
	}
	timedOut, _, _ := apply(test, campaign, row, ResultTimedOut, true)
	if timedOut.State != StatePending || timedOut.Pid != row.Pid {
		test.Fatalf("converging timed_out after delivery must resend the same pid: %+v", timedOut)
	}
	drifted, changed := succeeded.Drifted()
	if !changed || drifted.State != StatePending || drifted.Attempt != 2 || drifted.Pid != 0 {
		test.Fatalf("drift: %+v", drifted)
	}
}

func TestQuarantineOutcomes(test *testing.T) {
	campaign := testCampaign(KindQuarantine)
	row := dispatched(test, campaign)
	verifying, _, _ := apply(test, campaign, row, ResultFailed, true)
	if verifying.State != StateVerifying || verifying.LastError != "boom" {
		test.Fatalf("script failure without require_script_success: %+v", verifying)
	}
	flipped, changed := verifying.Flipped(now)
	if !changed || flipped.State != StateSucceeded {
		test.Fatalf("flip: %+v", flipped)
	}
	campaign.Action.RequireScriptSuccess = true
	backoff, _, _ := apply(test, campaign, row, ResultFailed, true)
	if backoff.State != StateBackoff {
		test.Fatalf("script failure with require_script_success: %+v", backoff)
	}
	pending := NewRow(campaign.ID, device, installation, 0, StatePending)
	direct, changed := pending.QuarantineWithoutScript(now)
	if !changed || direct.State != StateVerifying {
		test.Fatalf("quarantine without a script: %+v", direct)
	}
}

func TestDeadlines(test *testing.T) {
	oneShot := testCampaign(KindRunScript)
	row := dispatched(test, oneShot)
	if _, changed := row.Expire(oneShot, now, time.Minute); changed {
		test.Fatal("expired before the deadline")
	}
	late := now.Add(2 * time.Hour)
	undelivered, changed := row.Expire(oneShot, late, time.Minute)
	if !changed || undelivered.State != StatePending || undelivered.Pid != row.Pid {
		test.Fatalf("undelivered past deadline: %+v", undelivered)
	}
	delivered, _, _ := apply(test, oneShot, row, ResultStarted, true)
	unknown, changed := delivered.Expire(oneShot, late, time.Minute)
	if !changed || unknown.State != StateUnknown {
		test.Fatalf("delivered one-shot past deadline: %+v", unknown)
	}
	late2 := unknown
	lateResult, _, changed := late2.ApplyResult(oneShot, Result{Pid: row.Pid, Status: ResultSucceeded, Delivered: true}, late, 0)
	if !changed || lateResult.State != StateSucceeded {
		test.Fatalf("late result on an unknown row: %+v", lateResult)
	}
	resolved, changed := unknown.Resolve(oneShot, false, "checked by hand", late)
	if !changed || resolved.State != StateFailed || resolved.LastError != "checked by hand" {
		test.Fatalf("resolve: %+v", resolved)
	}
	converging := testCampaign(KindEnsureConfig)
	row = dispatched(test, converging)
	delivered, _, _ = apply(test, converging, row, ResultStarted, true)
	resend, changed := delivered.Expire(converging, late.Add(time.Hour), time.Minute)
	if !changed || resend.State != StatePending || resend.Pid != row.Pid {
		test.Fatalf("delivered ensure_config past deadline: %+v", resend)
	}
	quarantine := testCampaign(KindQuarantine)
	row = dispatched(test, quarantine)
	delivered, _, _ = apply(test, quarantine, row, ResultStarted, true)
	flipping, changed := delivered.Expire(quarantine, late, time.Minute)
	if !changed || flipping.State != StateVerifying {
		test.Fatalf("a quarantine whose script may have run still flips its node when the script is not required: %+v", flipping)
	}
	quarantine.Action.RequireScriptSuccess = true
	unresolved, _ := delivered.Expire(quarantine, late, time.Minute)
	if unresolved.State != StateUnknown {
		test.Fatalf("a required script with no result: %+v", unresolved)
	}
	resolved, changed = unresolved.Resolve(quarantine, true, "the script ran", late)
	if !changed || resolved.State != StateVerifying {
		test.Fatalf("a quarantine resolved as succeeded succeeds only once its flip is recorded: %+v", resolved)
	}
	quarantine.Status = StatusAborted
	if cancelled, _ := unresolved.Resolve(quarantine, true, "the script ran", late); cancelled.State != StateCancelled {
		test.Fatalf("an aborted quarantine never flips: %+v", cancelled)
	}
}

func TestFinishedCampaignsLeaveNothingWaiting(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	campaign.Status = StatusAborted
	row := dispatched(test, campaign)
	if failed, _, _ := apply(test, campaign, row, ResultFailed, true); failed.State != StateCancelled || failed.FinishedAt == nil {
		test.Fatalf("a failure after the campaign ended: %+v", failed)
	}
	if unreachable, _, _ := apply(test, campaign, row, ResultUnreachable, false); unreachable.State != StateCancelled {
		test.Fatalf("an undelivered process after the campaign ended: %+v", unreachable)
	}
	if expired, _ := row.Expire(campaign, now.Add(2*time.Hour), time.Minute); expired.State != StateCancelled {
		test.Fatalf("a deadline after the campaign ended: %+v", expired)
	}
	succeeded, _, _ := apply(test, campaign, row, ResultSucceeded, true)
	if succeeded.State != StateCancelled {
		test.Fatalf("an ensure row that would wait for verification after the campaign ended: %+v", succeeded)
	}
	satisfied := true
	if done, _, _ := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultSucceeded, Delivered: true, Satisfied: &satisfied}, now, 0); done.State != StateSucceeded {
		test.Fatalf("a finished result is still recorded: %+v", done)
	}
}

func TestMembershipTransitions(test *testing.T) {
	row := NewRow(uuid.New(), device, installation, 1, StatePending)
	excluded, changed := row.Exclude()
	if !changed || excluded.State != StateExcluded {
		test.Fatal("exclude")
	}
	if included, changed := excluded.Include(); !changed || included.State != StatePending {
		test.Fatal("include")
	}
	conflicted, changed := row.Conflict()
	if !changed || conflicted.State != StateConflict {
		test.Fatal("conflict")
	}
	if cleared, changed := conflicted.Unconflict(); !changed || cleared.State != StatePending {
		test.Fatal("unconflict")
	}
	cancelled, changed := conflicted.Cancel(now)
	if !changed || cancelled.State != StateCancelled || !cancelled.State.Terminal() {
		test.Fatal("cancel")
	}
	if _, changed := cancelled.Exclude(); changed {
		test.Fatal("excluded a terminal row")
	}
	satisfied, changed := row.Satisfied(now)
	if !changed || satisfied.State != StateSucceeded {
		test.Fatal("satisfied")
	}
	if _, changed := satisfied.Satisfied(now); changed {
		test.Fatal("satisfied a terminal row twice")
	}
	inFlight := Row{State: StateDelivered}
	if _, changed := inFlight.Exclude(); changed {
		test.Fatal("excluded an in-flight row")
	}
	if _, changed := inFlight.Cancel(now); changed {
		test.Fatal("cancelled an in-flight row")
	}
}

func TestSilentTracking(test *testing.T) {
	window := 30 * time.Minute
	finished := now
	row := Row{State: StateSucceeded, Epoch: 100, FinishedAt: &finished}
	if _, changed := row.Disconnected(now.Add(time.Minute), 99); changed {
		test.Fatal("an older session's disconnect counted")
	}
	gone, changed := row.Disconnected(now.Add(time.Minute), 100)
	if !changed || gone.EventAt == nil {
		test.Fatal("disconnect after success not recorded")
	}
	if _, changed := gone.Disconnected(now.Add(2*time.Minute), 101); changed {
		test.Fatal("second disconnect overwrote the first")
	}
	back, changed := gone.Reconnected(now.Add(10*time.Minute), window)
	if !changed || back.Silent == nil || *back.Silent {
		test.Fatalf("quick reconnect: %+v", back)
	}
	slow, _ := gone.Reconnected(now.Add(2*time.Hour), window)
	if slow.Silent == nil || !*slow.Silent {
		test.Fatalf("slow reconnect: %+v", slow)
	}
	if _, changed := gone.Lapsed(now.Add(20*time.Minute), window); changed {
		test.Fatal("lapsed inside the window")
	}
	lapsed, changed := gone.Lapsed(now.Add(time.Hour), window)
	if !changed || lapsed.Silent == nil || !*lapsed.Silent {
		test.Fatal("lapse not recorded")
	}
	if _, changed := row.Lapsed(now.Add(10*time.Hour), window); changed {
		test.Fatal("a node connected since its success lapsed")
	}
}

func TestProcessTimeoutCoversCollectedFilesAndLogStreams(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	campaign.Action.CollectFiles = []string{"/var/log/syslog", "/var/log/auth.log"}
	campaign.Action.StreamLogs = &StreamLogs{Level: "info", DurationSeconds: 3600}
	if timeout := campaign.ProcessTimeout(); timeout != (3*900+3600)*time.Second {
		test.Fatalf("process timeout %s", timeout)
	}
	started, _, _ := apply(test, campaign, dispatched(test, campaign), ResultStarted, true)
	if !started.DeadlineAt.Equal(now.Add(campaign.ProcessTimeout())) {
		test.Fatalf("a started process that streams logs after its script must not pass its deadline before the stream ends: %v", started.DeadlineAt)
	}
	last := now.Add(time.Hour)
	campaign.LastDispatchAt = &last
	if !campaign.ArchivableAt().Equal(last.Add(campaign.ProcessTimeout() + time.Hour)) {
		test.Fatalf("archivable at %v", campaign.ArchivableAt())
	}
}

func TestUndeliveredAttemptsBackOffFurther(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	campaign.Policy.Retry = Retry{MaxAttempts: 1, InitialBackoffSeconds: 10, MaxBackoffSeconds: 600, Multiplier: 2}
	row := dispatched(test, campaign)
	for expected := 1; expected <= 3; expected++ {
		row, _, _ = row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultUnreachable}, now, 1)
		if row.Unreached != expected || !row.NextAttemptAt.Equal(now.Add(10*time.Second<<(expected-1))) {
			test.Fatalf("undelivered %d times: %+v", expected, row)
		}
		row, _ = row.Dispatch(campaign.ID, Session{NamespaceID: "5d2e9a1c7b3f8e04", Epoch: 100}, now, now.Add(time.Hour), nil)
		row, _ = row.Accepted(row.Pid)
	}
	row, _, _ = apply(test, campaign, row, ResultStarted, true)
	if row.Unreached != 0 {
		test.Fatalf("a delivered process still counts its undelivered dispatches: %+v", row)
	}
}

func TestDuplicatesOnEnsureRowsAreDecidedByTheReportedState(test *testing.T) {
	campaign := testCampaign(KindEnsureVersion)
	unsatisfied := false
	row := dispatched(test, campaign)
	verifying, _, changed := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultDuplicate, Satisfied: &unsatisfied}, now, 0)
	if !changed || verifying.State != StateVerifying || verifying.LastStatus != string(ResultDuplicate) {
		test.Fatalf("a duplicate reporting another version: %+v", verifying)
	}
	unreported, _, changed := row.ApplyResult(campaign, Result{Pid: row.Pid, Status: ResultDuplicate}, now, 0)
	if !changed || unreported.State != StateDelivered || unreported.DeadlineAt.Before(now.Add(campaign.ProcessTimeout())) {
		test.Fatalf("a duplicate whose reported state dawn could not read waits like running: %+v", unreported)
	}
	quarantine := testCampaign(KindQuarantine)
	held, _, changed := dispatched(test, quarantine).ApplyResult(quarantine, Result{Pid: DerivePid(quarantine.ID, device, installation, 1), Status: ResultDuplicate}, now, 0)
	if !changed || held.State != StateDelivered {
		test.Fatalf("a quarantine duplicate is a one-shot held delivered: %+v", held)
	}
}

func TestReapedAtFollowsThePid(test *testing.T) {
	campaign := testCampaign(KindRunScript)
	row := dispatched(test, campaign)
	if encoded, failure := json.Marshal(NewRow(campaign.ID, device, installation, 0, StatePending)); failure != nil || !strings.Contains(string(encoded), `"pid":null`) {
		test.Fatalf("a row with no pid encodes as %s (%v)", encoded, failure)
	}
	encoded, failure := json.Marshal(row)
	if failure != nil || !strings.Contains(string(encoded), `"pid":"`+row.Pid.String()+`"`) || !strings.Contains(string(encoded), `"reaped_at":null`) {
		test.Fatalf("a dispatched row encodes as %s (%v)", encoded, failure)
	}
}

func TestAnEndedScriptIsNeverRunAgain(test *testing.T) {
	oneShot := testCampaign(KindRunScript)
	delivered, _, _ := apply(test, oneShot, dispatched(test, oneShot), ResultStarted, true)
	unknown, _, changed := apply(test, oneShot, delivered, ResultEnded, true)
	if !changed || unknown.State != StateUnknown || unknown.FinishedAt == nil || unknown.Pid != delivered.Pid || unknown.LastStatus != string(ResultEnded) {
		test.Fatalf("a one-shot whose script ended without a known outcome: %+v", unknown)
	}
	quarantine := testCampaign(KindQuarantine)
	flipping, _, _ := apply(test, quarantine, dispatched(test, quarantine), ResultEnded, true)
	if flipping.State != StateVerifying {
		test.Fatalf("a quarantine whose script ended is still flipped when the script is not required: %+v", flipping)
	}
	converging := testCampaign(KindEnsureVersion)
	unsatisfied := false
	row := dispatched(test, converging)
	verifying, _, changed := row.ApplyResult(converging, Result{Pid: row.Pid, Status: ResultEnded, Delivered: true, Satisfied: &unsatisfied}, now, 0)
	if !changed || verifying.State != StateVerifying {
		test.Fatalf("an ensure row whose script ended is judged on the facts of its next session: %+v", verifying)
	}
}
