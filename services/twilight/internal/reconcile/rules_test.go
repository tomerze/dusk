package reconcile

import (
	"testing"
	"time"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/kafka"
)

const pidValue campaign.Pid = 12808937078074471924

func callFor(test *testing.T, entry kafka.LedgerEntry) (Call, bool) {
	test.Helper()
	at, failure := kafka.ParseTime(entry.Time)
	if failure != nil {
		test.Fatal(failure)
	}
	call, isCall, failure := CallOf(entry, at)
	if failure != nil {
		test.Fatal(failure)
	}
	return call, isCall
}

func TestCallsAreClientCallsWithTheirPid(test *testing.T) {
	writer := newWriter("nightfall-0", 0, start, nil)
	process, isCall := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if !isCall || !process.CreatesProcess() || process.Pid != pidValue || process.DeviceID != fixtureDevice || process.SessionID != fixtureSession {
		test.Fatalf("a client call: %+v %v", process, isCall)
	}
	shell, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "ShPortal.sh", 7))
	hostname, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.hostname", 3))
	if !shell.RunsCommand() || hostname.RunsCommand() || hostname.CreatesProcess() {
		test.Fatalf("commands: sh %+v hostname %+v", shell, hostname)
	}
	for _, code := range []string{"denied", "rate_limited"} {
		for _, action := range []string{"ShPortal.sh", "Dusk.process"} {
			refused := writer.call("dawn-0", pidFixture, fixtureSession, action, 7)
			refused.ResultCode = text(code)
			if _, isCall := callFor(test, refused); isCall {
				test.Fatalf("a %s %s, which never reached the node, is checked as a call", code, action)
			}
		}
	}
	kill, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.kill", 3))
	if !kill.Reaps() {
		test.Fatal("Dusk.kill is not part of a reap")
	}
	reverse := writer.call("dawn-0", pidFixture, fixtureSession, "Stream.send", 9)
	reverse.Direction = text("node_to_client")
	if _, isCall := callFor(test, reverse); isCall {
		test.Fatal("a node_to_client call is checked as a client call")
	}
	if _, isCall := callFor(test, writer.event("session_open", nil)); isCall {
		test.Fatal("a session_open event is a call")
	}
	broken := writer.call("dawn-0", "18446744073709551616", fixtureSession, "Dusk.process", 3)
	at, _ := kafka.ParseTime(broken.Time)
	if _, _, failure := CallOf(broken, at); failure == nil {
		test.Fatal("a pid past u64 was read")
	}
}

func TestProcessesWithoutAPidAndTheDefaultShellAreNotTracked(test *testing.T) {
	rules := Rules{}
	writer := newWriter("nightfall-0", 0, start, nil)
	chosen, _ := callFor(test, writer.call("dawn-0", "0", fixtureSession, "Dusk.process", 0))
	if !chosen.Unattributed() || chosen.Tracked() || rules.AgainstIntent(chosen, nil) != nil {
		test.Fatalf("a process the node chose the pid of: %+v", chosen)
	}
	ps, _ := callFor(test, writer.call("dawn-0", "0", fixtureSession, "Dusk.ps", 0))
	if ps.Unattributed() || ps.Tracked() {
		test.Fatalf("Dusk.ps under no process: %+v", ps)
	}
	shell := campaign.DefaultShellPid.String()
	for _, action := range []string{"Dusk.process", "ShPortal.sh", "Dusk.kill"} {
		call, _ := callFor(test, writer.call("dawn-0", shell, fixtureSession, action, 3))
		if call.Tracked() || call.Unattributed() {
			test.Errorf("%s under the default shell is tracked: %+v", action, call)
		}
		if raised := rules.AgainstIntent(call, nil); raised != nil {
			test.Errorf("%s under the default shell raised %+v", action, raised)
		}
	}
	attributed, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if attributed.Unattributed() || !attributed.Tracked() {
		test.Fatalf("a process at a named pid: %+v", attributed)
	}
}

func TestDefaultShellCommandsAreHeldToTheOpenIntendedProcesses(test *testing.T) {
	rules := Rules{}
	writer := newWriter("nightfall-0", 0, start, nil)
	shell := campaign.DefaultShellPid.String()
	command, _ := callFor(test, writer.call("dawn-0", shell, fixtureSession, "ShPortal.sh", 3))
	if !command.InDefaultShell() {
		test.Fatalf("a shell command in the default shell: %+v", command)
	}
	for _, action := range []string{"Dusk.process", "Dusk.kill"} {
		if other, _ := callFor(test, writer.call("dawn-0", shell, fixtureSession, action, 3)); other.InDefaultShell() {
			test.Errorf("%s in the default shell is counted as a shell command", action)
		}
	}
	denied := writer.call("dawn-0", shell, fixtureSession, "ShPortal.sh", 3)
	denied.ResultCode = text("denied")
	if _, isCall := callFor(test, denied); isCall {
		test.Error("a denied shell command is counted")
	}
	if under, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "ShPortal.sh", 3)); under.InDefaultShell() {
		test.Error("a shell command under a process's own pid is counted against the default shell")
	}

	if _, raised := rules.InDefaultShell(command, nil); raised == nil || raised.Kind != alerts.KindDefaultShellWithoutIntent || raised.Severity != alerts.Critical ||
		raised.Fingerprint != "default_shell_without_intent:"+fixtureDevice+"/"+fixtureInstallation+"/"+fixtureSession {
		test.Fatalf("a shell command with no open intended process: %+v", raised)
	}
	closed := []OpenProcess{
		{CreatedAt: command.Time.Add(-time.Hour), ExpiresAt: command.Time.Add(-time.Minute), DefaultShellCommands: 8},
		{CreatedAt: command.Time.Add(time.Minute), ExpiresAt: command.Time.Add(time.Hour), DefaultShellCommands: 8},
	}
	if _, raised := rules.InDefaultShell(command, closed); raised == nil {
		test.Fatal("intended processes that expired before the call or opened after it let it through")
	}
	opened := append(closed,
		OpenProcess{CreatedAt: command.Time.Add(-10 * time.Minute), ExpiresAt: command.Time.Add(time.Minute), DefaultShellCommands: 3},
		OpenProcess{CreatedAt: command.Time.Add(-time.Minute), ExpiresAt: command.Time.Add(-2 * time.Second), DefaultShellCommands: 4},
		OpenProcess{CreatedAt: command.Time.Add(2 * time.Second), ExpiresAt: command.Time.Add(time.Hour), DefaultShellCommands: 4})
	if window, raised := rules.InDefaultShell(command, opened); raised != nil || !window.Equal(command.Time.Add(-10*time.Minute)) {
		test.Fatalf("the window of a call is the earliest open intended process's creation: %v %+v", window, raised)
	}

	window := DefaultShellWindow{DeviceID: fixtureDevice, InstallationID: fixtureInstallation, Start: start, Commands: 4, Allowed: 4, LastCallAt: start.Add(time.Minute)}
	if raised := DefaultShellShape(window); raised != nil {
		test.Fatalf("a window within its budget raised %+v", raised)
	}
	window.Commands = 5
	raised := DefaultShellShape(window)
	if raised == nil || raised.Kind != alerts.KindProcessShape || raised.Severity != alerts.High ||
		raised.Fingerprint != "process_shape:"+fixtureDevice+"/"+fixtureInstallation+"@"+kafka.FormatTime(start) ||
		raised.Detail["commands"] != 5 || raised.Detail["max_commands"] != 4 || raised.Detail["pid"] != shell {
		test.Fatalf("a window past its budget: %+v", raised)
	}
}

func intendedFixture() *IntendedProcess {
	return &IntendedProcess{Pid: pidValue, DeviceID: fixtureDevice, InstallationID: fixtureInstallation, ActionKind: "ensure_version",
		ExpiresAt: start.Add(time.Hour), LastDispatchedAt: start.Add(-time.Minute), MaxCommands: 8}
}

func TestCallsAgainstTheirIntendedProcess(test *testing.T) {
	rules := Rules{}
	writer := newWriter("nightfall-0", 0, start, nil)
	process, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "Dusk.process", 3))
	if raised := rules.AgainstIntent(process, intendedFixture()); raised != nil {
		test.Fatalf("a process within its intent: %+v", raised)
	}
	raised := rules.AgainstIntent(process, nil)
	if raised == nil || raised.Kind != alerts.KindProcessWithoutIntent || raised.Severity != alerts.Critical || raised.Fingerprint != "process_without_intent:"+pidFixture {
		test.Fatalf("a process twilight never intended: %+v", raised)
	}
	shell, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, "ShPortal.sh", 7))
	if raised := rules.AgainstIntent(shell, nil); raised != nil {
		test.Fatalf("a shell command under an unintended pid alerts on its Dusk.process, not on each command: %+v", raised)
	}
	elsewhere := intendedFixture()
	elsewhere.InstallationID = "00000000000000000000000000000001"
	for _, call := range []Call{process, shell} {
		if raised := rules.AgainstIntent(call, elsewhere); raised == nil || raised.Kind != alerts.KindTargetMismatch || raised.Severity != alerts.Critical {
			test.Fatalf("%s on another node: %+v", call.Action, raised)
		}
	}
	expired := intendedFixture()
	expired.ExpiresAt = start.Add(-time.Second)
	if raised := rules.AgainstIntent(shell, expired); raised == nil || raised.Kind != alerts.KindProcessAfterDeadline || raised.Severity != alerts.High {
		test.Fatalf("a call after its intended process expired: %+v", raised)
	}
	for _, action := range []string{"Dusk.kill", "Dusk.waitpid"} {
		reap, _ := callFor(test, writer.call("dawn-0", pidFixture, fixtureSession, action, 3))
		if raised := rules.AgainstIntent(reap, expired); raised != nil {
			test.Errorf("%s after the deadline is the reap: %+v", action, raised)
		}
	}
	unattributed, _ := callFor(test, writer.call("dawn-0", "0", fixtureSession, "Dusk.ps", 0))
	if raised := rules.AgainstIntent(unattributed, nil); raised != nil {
		test.Fatalf("a call under no process checked against intended processes: %+v", raised)
	}
}

func TestProcessesAgainstTheirIntent(test *testing.T) {
	rules := Rules{}
	at := start.Add(time.Hour)
	kinds := func(raised []alerts.Raised) []string {
		var found []string
		for _, alert := range raised {
			found = append(found, alert.Kind)
		}
		return found
	}
	process := ProcessState{Pid: pidValue, Sessions: []string{fixtureSession}, Commands: 8}
	if raised := rules.AgainstProcess(process, intendedFixture(), at); len(raised) != 0 {
		test.Fatalf("a process within its shape: %v", kinds(raised))
	}
	if raised := rules.AgainstProcess(process, nil, at); len(raised) != 0 {
		test.Fatalf("a process without an intent: %v", kinds(raised))
	}

	reused := process
	reused.Sessions = []string{fixtureSession, "0192f3a4-0000-7c6b-8a59-483726150f1e"}
	if raised := kinds(rules.AgainstProcess(reused, intendedFixture(), at)); len(raised) != 1 || raised[0] != alerts.KindPidReused {
		test.Fatalf("one pid created on two sessions: %v", raised)
	}
	created := start.Add(time.Minute)
	reused.CreatedAt = &created
	if raised := kinds(rules.AgainstProcess(reused, intendedFixture(), at)); len(raised) != 1 || raised[0] != alerts.KindPidReused {
		test.Fatalf("one pid created on two sessions with no resend after the first: %v", raised)
	}
	resentAfterCreation := intendedFixture()
	resentAfterCreation.LastDispatchedAt = created.Add(time.Minute)
	if raised := rules.AgainstProcess(reused, resentAfterCreation, at); len(raised) != 0 {
		test.Fatalf("a pid twilight resent after its first creation ran again on a new session: %v", kinds(raised))
	}

	shaped := process
	shaped.Commands = 9
	if raised := kinds(rules.AgainstProcess(shaped, intendedFixture(), at)); len(raised) != 1 || raised[0] != alerts.KindProcessShape {
		test.Fatalf("nine shell commands where eight were intended: %v", raised)
	}
	interactive := intendedFixture()
	interactive.ActionKind, interactive.MaxCommands = "interactive", 10000
	shaped.Commands = 500
	if raised := rules.AgainstProcess(shaped, interactive, at); len(raised) != 0 {
		test.Fatalf("an interactive session within its commands: %v", kinds(raised))
	}

	resultAt := start.Add(10 * time.Minute)
	late := start.Add(12 * time.Minute)
	afterResult := process
	afterResult.ResultAt, afterResult.LastCallAt, afterResult.ResultStatus = &resultAt, &late, text("succeeded")
	if raised := kinds(rules.AgainstProcess(afterResult, intendedFixture(), at)); len(raised) != 1 || raised[0] != alerts.KindProcessAfterResult {
		test.Fatalf("calls two minutes after the final result: %v", raised)
	}
	resent := intendedFixture()
	resent.LastDispatchedAt = start.Add(11 * time.Minute)
	if raised := rules.AgainstProcess(afterResult, resent, at); len(raised) != 0 {
		test.Fatalf("calls after a resend of the pid: %v", kinds(raised))
	}
	soon := resultAt.Add(30 * time.Second)
	afterResult.LastCallAt = &soon
	if raised := rules.AgainstProcess(afterResult, intendedFixture(), at); len(raised) != 0 {
		test.Fatalf("calls within the minute after the result: %v", kinds(raised))
	}
}

func TestQuarantineOverridesAndChainBreaksRaiseAlerts(test *testing.T) {
	writer := newWriter("nightfall-0", 0, start, nil)
	override := writer.event("quarantine_override", map[string]any{"role": "incident-response"})
	override.Pid, override.SessionID, override.CallID = pidFixture, text(fixtureSession), text("0192f3a4-b001-7b2c-9d3e-4f5061728394")
	raised := QuarantineOverride(override, start)
	if raised == nil || raised.Severity != alerts.High || raised.Fingerprint != "quarantine_override:"+fixtureSession+"/"+pidFixture || raised.Detail["pid"] != pidFixture {
		test.Fatalf("a quarantine override: %+v", raised)
	}
	if raised := QuarantineOverride(writer.event("session_open", nil), start); raised != nil {
		test.Fatalf("a session_open: %+v", raised)
	}
	broken := ChainBroken(Break{Kind: BreakSequenceGap, Instance: "nightfall-2", Partition: 2, Sequence: 41, Detail: "sequences 39 to 40 are missing"}, 2, 9001, start)
	if broken.Severity != alerts.Critical || broken.Fingerprint != "ledger_chain_broken:nightfall-2/2/sequence_gap" || broken.Detail["offset"] != int64(9001) {
		test.Fatalf("a chain break: %+v", broken)
	}
	delivered := start
	unledgered := ResultWithoutLedger(ProcessState{Pid: pidValue, DeliveredAt: &delivered, ResultSource: map[string]any{"dawn_instance": "dawn-1"}})
	if unledgered.Severity != alerts.Critical || unledgered.Fingerprint != "result_without_ledger:"+pidFixture || unledgered.Detail["dawn_instance"] != "dawn-1" || !unledgered.At.Equal(start) {
		test.Fatalf("a result with no Dusk.process in the ledger: %+v", unledgered)
	}
}
