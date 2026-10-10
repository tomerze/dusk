package reconcile

import (
	"fmt"
	"time"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/kafka"
)

const (
	clockSkew              = 5 * time.Second
	resultGrace            = 60 * time.Second
	ResultWithoutLedgerAge = 10 * time.Minute
	processAction          = "Dusk.process"
	commandAction          = "ShPortal.sh"
)

var reapActions = map[string]bool{"Dusk.kill": true, "Dusk.waitpid": true}

type IntendedProcess struct {
	Pid              campaign.Pid
	CampaignID       *string
	DeviceID         string
	InstallationID   string
	ActionKind       string
	ExpiresAt        time.Time
	LastDispatchedAt time.Time
	MaxCommands      int
}

type Call struct {
	Pid            campaign.Pid
	SessionID      string
	CallID         string
	Principal      string
	DeviceID       string
	InstallationID string
	NamespaceID    string
	Action         string
	Instance       string
	Partition      int64
	Sequence       int64
	Time           time.Time
}

func CallOf(entry kafka.LedgerEntry, at time.Time) (Call, bool, error) {
	if entry.Kind != "call" || entry.Direction == nil || *entry.Direction != "client_to_node" || entry.ResultCode != nil {
		return Call{}, false, nil
	}
	pid, failure := campaign.ParsePid(entry.Pid)
	if failure != nil {
		return Call{}, false, failure
	}
	return Call{
		Pid: pid, Principal: entry.Principal, Instance: entry.Instance, Partition: entry.Partition, Sequence: entry.Sequence, Time: at,
		SessionID: valueOf(entry.SessionID), CallID: valueOf(entry.CallID), DeviceID: valueOf(entry.DeviceID),
		InstallationID: valueOf(entry.InstallationID), NamespaceID: valueOf(entry.NamespaceID), Action: valueOf(entry.Action),
	}, true, nil
}

func valueOf(field *string) string {
	if field == nil {
		return ""
	}
	return *field
}

func (call Call) CreatesProcess() bool {
	return call.Action == processAction
}

func (call Call) Unattributed() bool {
	return call.CreatesProcess() && call.Pid == 0
}

func (call Call) Tracked() bool {
	return call.Pid != 0 && call.Pid != campaign.DefaultShellPid
}

func (call Call) RunsCommand() bool {
	return call.Action == commandAction
}

func (call Call) InDefaultShell() bool {
	return call.Pid == campaign.DefaultShellPid && call.RunsCommand()
}

func (call Call) Reaps() bool {
	return reapActions[call.Action]
}

func (call Call) detail(message string) map[string]any {
	return map[string]any{
		"message": message, "pid": call.Pid.String(), "session_id": call.SessionID, "call_id": call.CallID,
		"principal": call.Principal, "device_id": call.DeviceID, "installation_id": call.InstallationID,
		"namespace_id": call.NamespaceID, "action": call.Action, "instance": call.Instance,
		"partition": call.Partition, "sequence": call.Sequence, "time": kafka.FormatTime(call.Time),
	}
}

type Rules struct{}

func raised(severity alerts.Severity, kind, key string, detail map[string]any, at time.Time) alerts.Raised {
	return alerts.Raised{Severity: severity, Kind: kind, Fingerprint: kind + ":" + key, Detail: detail, At: at}
}

func (rules Rules) AgainstIntent(call Call, intended *IntendedProcess) *alerts.Raised {
	if !call.Tracked() {
		return nil
	}
	key := call.Pid.String()
	switch {
	case intended == nil:
		if !call.CreatesProcess() {
			return nil
		}
		found := raised(alerts.Critical, alerts.KindProcessWithoutIntent, key, call.detail("a client created a process at a pid twilight never intended"), call.Time)
		return &found
	case intended.DeviceID != call.DeviceID || intended.InstallationID != call.InstallationID:
		detail := call.detail("a call under a pid reached a node other than the one its intended process names")
		detail["intended_device_id"], detail["intended_installation_id"] = intended.DeviceID, intended.InstallationID
		found := raised(alerts.Critical, alerts.KindTargetMismatch, key, detail, call.Time)
		return &found
	case call.Time.After(intended.ExpiresAt) && !call.Reaps():
		detail := call.detail("a call under a pid arrived after its intended process expired")
		detail["expires_at"] = kafka.FormatTime(intended.ExpiresAt)
		found := raised(alerts.High, alerts.KindProcessAfterDeadline, key, detail, call.Time)
		return &found
	}
	return nil
}

type OpenProcess struct {
	CreatedAt            time.Time
	ExpiresAt            time.Time
	DefaultShellCommands int
}

func (rules Rules) InDefaultShell(call Call, open []OpenProcess) (time.Time, *alerts.Raised) {
	var window time.Time
	for _, process := range open {
		if call.Time.Before(process.CreatedAt.Add(-clockSkew)) || call.Time.After(process.ExpiresAt.Add(clockSkew)) {
			continue
		}
		if window.IsZero() || process.CreatedAt.Before(window) {
			window = process.CreatedAt
		}
	}
	if !window.IsZero() {
		return window, nil
	}
	found := raised(alerts.Critical, alerts.KindDefaultShellWithoutIntent, call.DeviceID+"/"+call.InstallationID+"/"+call.SessionID,
		call.detail("a shell command ran in the node's default shell while no process twilight intended for the node was open"), call.Time)
	return time.Time{}, &found
}

type DefaultShellWindow struct {
	DeviceID       string
	InstallationID string
	Start          time.Time
	Commands       int
	Allowed        int
	LastCallAt     time.Time
}

func DefaultShellShape(window DefaultShellWindow) *alerts.Raised {
	if window.Commands <= window.Allowed {
		return nil
	}
	found := raised(alerts.High, alerts.KindProcessShape, window.DeviceID+"/"+window.InstallationID+"@"+kafka.FormatTime(window.Start), map[string]any{
		"message": fmt.Sprintf("the node's default shell ran %d shell commands while the processes twilight intended for the node allowed %d", window.Commands, window.Allowed),
		"pid":     campaign.DefaultShellPid.String(), "device_id": window.DeviceID, "installation_id": window.InstallationID,
		"window_start": kafka.FormatTime(window.Start), "last_call_at": kafka.FormatTime(window.LastCallAt), "commands": window.Commands, "max_commands": window.Allowed,
	}, window.LastCallAt)
	return &found
}

type ProcessState struct {
	Pid          campaign.Pid
	Sessions     []string
	CreatedAt    *time.Time
	LastCallAt   *time.Time
	Commands     int
	DeliveredAt  *time.Time
	ResultStatus *string
	ResultAt     *time.Time
	ResultSource map[string]any
}

func (process ProcessState) detail(message string, intended *IntendedProcess) map[string]any {
	detail := map[string]any{"message": message, "pid": process.Pid.String(), "sessions": process.Sessions, "commands": process.Commands}
	if intended != nil {
		detail["device_id"], detail["installation_id"], detail["action_kind"], detail["max_commands"] = intended.DeviceID, intended.InstallationID, intended.ActionKind, intended.MaxCommands
		if intended.CampaignID != nil {
			detail["campaign_id"] = *intended.CampaignID
		}
	}
	if process.LastCallAt != nil {
		detail["last_call_at"] = kafka.FormatTime(*process.LastCallAt)
	}
	if process.ResultAt != nil {
		detail["result_at"] = kafka.FormatTime(*process.ResultAt)
	}
	if process.ResultStatus != nil {
		detail["result_status"] = *process.ResultStatus
	}
	return detail
}

func (rules Rules) AgainstProcess(process ProcessState, intended *IntendedProcess, at time.Time) []alerts.Raised {
	if intended == nil {
		return nil
	}
	key := process.Pid.String()
	var found []alerts.Raised
	if len(process.Sessions) > 1 && (process.CreatedAt == nil || !intended.LastDispatchedAt.After(*process.CreatedAt)) {
		found = append(found, raised(alerts.High, alerts.KindPidReused, key,
			process.detail(fmt.Sprintf("a process at one pid was created on %d client sessions", len(process.Sessions)), intended), at))
	}
	if process.Commands > intended.MaxCommands {
		found = append(found, raised(alerts.High, alerts.KindProcessShape, key,
			process.detail(fmt.Sprintf("a %s process ran %d shell commands, more than the %d it was intended to", intended.ActionKind, process.Commands, intended.MaxCommands), intended), at))
	}
	if process.ResultAt != nil && process.LastCallAt != nil && process.LastCallAt.After(process.ResultAt.Add(resultGrace)) && !intended.LastDispatchedAt.After(*process.ResultAt) {
		found = append(found, raised(alerts.High, alerts.KindProcessAfterResult, key,
			process.detail("calls arrived under a pid more than a minute after its final result", intended), at))
	}
	return found
}

func ResultWithoutLedger(process ProcessState) alerts.Raised {
	detail := process.detail("dawn reported a process started at a pid, but the ledger shows no Dusk.process for that pid within ten minutes", nil)
	for key, value := range process.ResultSource {
		detail[key] = value
	}
	if process.DeliveredAt != nil {
		detail["delivered_at"] = kafka.FormatTime(*process.DeliveredAt)
	}
	at := time.Now()
	if process.DeliveredAt != nil {
		at = *process.DeliveredAt
	}
	return raised(alerts.Critical, alerts.KindResultWithoutLedger, process.Pid.String(), detail, at)
}

func QuarantineOverride(entry kafka.LedgerEntry, at time.Time) *alerts.Raised {
	if entry.Kind != "event" || entry.Event == nil || *entry.Event != "quarantine_override" {
		return nil
	}
	call := Call{SessionID: valueOf(entry.SessionID), CallID: valueOf(entry.CallID), Principal: entry.Principal,
		DeviceID: valueOf(entry.DeviceID), InstallationID: valueOf(entry.InstallationID), NamespaceID: valueOf(entry.NamespaceID),
		Action: valueOf(entry.Action), Instance: entry.Instance, Partition: entry.Partition, Sequence: entry.Sequence, Time: at}
	if pid, failure := campaign.ParsePid(entry.Pid); failure == nil {
		call.Pid = pid
	}
	found := raised(alerts.High, alerts.KindQuarantineOverride, call.SessionID+"/"+call.Pid.String(),
		call.detail("a principal allowed past quarantine sent a call to a quarantined node"), at)
	return &found
}

func ChainBroken(found Break, kafkaPartition int32, offset int64, at time.Time) alerts.Raised {
	return alerts.Raised{Severity: alerts.Critical, Kind: alerts.KindLedgerChainBroken,
		Fingerprint: fmt.Sprintf("%s:%s/%d/%s", alerts.KindLedgerChainBroken, found.Instance, found.Partition, found.Kind),
		Detail: map[string]any{
			"message": "the ledger chain is broken: " + found.Detail, "break": found.Kind, "instance": found.Instance,
			"partition": found.Partition, "sequence": found.Sequence, "kafka_partition": kafkaPartition, "offset": offset,
		}, At: at}
}
