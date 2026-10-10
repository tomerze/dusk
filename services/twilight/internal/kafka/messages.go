package kafka

import (
	"encoding/json"
	"time"

	"github.com/google/uuid"
)

const TimeLayout = "2006-01-02T15:04:05.000000000Z"

func FormatTime(at time.Time) string {
	return at.UTC().Format(TimeLayout)
}

func ParseTime(text string) (time.Time, error) {
	return time.Parse(TimeLayout, text)
}

func NewMessageID() string {
	return uuid.Must(uuid.NewV7()).String()
}

type Envelope struct {
	Schema string `json:"schema"`
	ID     string `json:"id"`
	Time   string `json:"time"`
}

type Connection struct {
	Envelope
	Event            string  `json:"event"`
	DeviceID         string  `json:"device_id"`
	InstallationID   string  `json:"installation_id"`
	NamespaceID      string  `json:"namespace_id"`
	Epoch            int64   `json:"epoch"`
	Instance         string  `json:"instance"`
	InnerAddress     string  `json:"inner_address"`
	RemoteAddress    string  `json:"remote_address"`
	Tenant           *string `json:"tenant"`
	CertFingerprint  string  `json:"cert_fingerprint"`
	CertNotAfter     string  `json:"cert_not_after"`
	ConnectedAt      string  `json:"connected_at"`
	DisconnectReason *string `json:"disconnect_reason"`
}

type CensusRecord struct {
	Envelope
	Record           string          `json:"record"`
	Instance         string          `json:"instance"`
	InnerAddress     string          `json:"inner_address"`
	RelayAddress     string          `json:"relay_address"`
	Generation       int64           `json:"generation"`
	SnapshotEpoch    int64           `json:"snapshot_epoch"`
	StartedAt        string          `json:"started_at"`
	Full             bool            `json:"full"`
	ChunkCount       int             `json:"chunk_count"`
	SessionCount     int             `json:"session_count"`
	HeartbeatSeconds int             `json:"heartbeat_seconds"`
	Index            int             `json:"index"`
	Sessions         []CensusSession `json:"sessions"`
}

type CensusSession struct {
	DeviceID       string  `json:"device_id"`
	InstallationID string  `json:"installation_id"`
	NamespaceID    string  `json:"namespace_id"`
	Epoch          int64   `json:"epoch"`
	ConnectedAt    string  `json:"connected_at"`
	LastSeen       string  `json:"last_seen"`
	Tenant         *string `json:"tenant"`
}

type Enrollment struct {
	Envelope
	Operation               string  `json:"operation"`
	Outcome                 string  `json:"outcome"`
	Reason                  *string `json:"reason"`
	DeviceID                *string `json:"device_id"`
	InstallationID          *string `json:"installation_id"`
	Tenant                  *string `json:"tenant"`
	CredentialKind          string  `json:"credential_kind"`
	CredentialRef           *string `json:"credential_ref"`
	CredentialIssuer        *string `json:"credential_issuer"`
	HardwareFingerprintHash *string `json:"hardware_fingerprint_hash"`
	RemoteAddress           string  `json:"remote_address"`
	CertSerial              *string `json:"cert_serial"`
	CertFingerprint         *string `json:"cert_fingerprint"`
	CertNotAfter            *string `json:"cert_not_after"`
	DuskVersion             *string `json:"dusk_version"`
	Impl                    *string `json:"impl"`
	TargetOS                *string `json:"target_os"`
	TargetArch              *string `json:"target_arch"`
	Hostname                *string `json:"hostname"`
	Instance                string  `json:"instance"`
}

type Reported struct {
	VersionKey *string                    `json:"version_key"`
	Version    *string                    `json:"version"`
	ConfigHash *string                    `json:"config_hash"`
	Services   []string                   `json:"services"`
	Facts      map[string]json.RawMessage `json:"facts"`
}

type ProcessResult struct {
	Envelope
	Pid             string    `json:"pid"`
	CampaignID      *string   `json:"campaign_id"`
	Attempt         *int      `json:"attempt"`
	DeviceID        string    `json:"device_id"`
	InstallationID  string    `json:"installation_id"`
	NamespaceID     string    `json:"namespace_id"`
	ActionKind      string    `json:"action_kind"`
	Status          string    `json:"status"`
	Delivered       bool      `json:"delivered"`
	Error           *string   `json:"error"`
	StartedAt       string    `json:"started_at"`
	FinishedAt      *string   `json:"finished_at"`
	DawnInstance    string    `json:"dawn_instance"`
	OutputDigest    string    `json:"output_digest"`
	OutputCount     int64     `json:"output_count"`
	OutputTruncated bool      `json:"output_truncated"`
	Reported        *Reported `json:"reported"`
}

type ParameterField struct {
	Name     string `json:"name"`
	Redacted bool   `json:"redacted"`
}

type LedgerEntry struct {
	Envelope
	Kind             string           `json:"kind"`
	Instance         string           `json:"instance"`
	Partition        int64            `json:"partition"`
	Sequence         int64            `json:"sequence"`
	PreviousHash     string           `json:"previous_hash"`
	Hash             string           `json:"hash"`
	DeviceID         *string          `json:"device_id"`
	InstallationID   *string          `json:"installation_id"`
	NamespaceID      *string          `json:"namespace_id"`
	Epoch            *int64           `json:"epoch"`
	Principal        string           `json:"principal"`
	Pid              string           `json:"pid"`
	IntentCampaignID *string          `json:"intent_campaign_id"`
	IntentPrincipal  *string          `json:"intent_principal"`
	IntentSubject    *string          `json:"intent_subject"`
	SessionID        *string          `json:"session_id"`
	CallID           *string          `json:"call_id"`
	CapID            *int64           `json:"cap_id"`
	ParentCapID      *int64           `json:"parent_cap_id"`
	Direction        *string          `json:"direction"`
	Action           *string          `json:"action"`
	InterfaceID      *string          `json:"interface_id"`
	MethodID         *int64           `json:"method_id"`
	ParameterFields  []ParameterField `json:"param_fields"`
	ParameterCapIDs  []int64          `json:"param_cap_ids"`
	ParameterHash    string           `json:"param_hash"`
	ResultCode       *string          `json:"result_code"`
	ResultCapIDs     []int64          `json:"result_cap_ids"`
	Event            *string          `json:"event"`
	EventDetail      json.RawMessage  `json:"event_detail"`
	KeyID            *string          `json:"key_id"`
	Signature        *string          `json:"signature"`
}

type NodeState struct {
	Envelope
	Scope          string  `json:"scope"`
	DeviceID       string  `json:"device_id"`
	InstallationID *string `json:"installation_id"`
	Lifecycle      string  `json:"lifecycle"`
	Reason         *string `json:"reason"`
	Actor          string  `json:"actor"`
}

func NewInstallationState(device, installation, lifecycle string, reason *string, actor string, at time.Time) NodeState {
	return NodeState{
		Envelope:       Envelope{Schema: "dusk.node-state/v1", ID: NewMessageID(), Time: FormatTime(at)},
		Scope:          "installation",
		DeviceID:       device,
		InstallationID: &installation,
		Lifecycle:      lifecycle,
		Reason:         reason,
		Actor:          actor,
	}
}

func NewDeviceState(device, lifecycle string, reason *string, actor string, at time.Time) NodeState {
	state := NewInstallationState(device, "", lifecycle, reason, actor, at)
	state.Scope, state.InstallationID = "device", nil
	return state
}

func (state NodeState) Key() string {
	if state.Scope == "device" || state.InstallationID == nil {
		return "device/" + state.DeviceID
	}
	return "installation/" + state.DeviceID + "/" + *state.InstallationID
}

type IntendedProcess struct {
	Envelope
	Pid                  string  `json:"pid"`
	DeviceID             string  `json:"device_id"`
	InstallationID       string  `json:"installation_id"`
	CampaignID           *string `json:"campaign_id"`
	ActionKind           string  `json:"action_kind"`
	Principal            string  `json:"principal"`
	Subject              string  `json:"subject"`
	CreatedAt            string  `json:"created_at"`
	ExpiresAt            string  `json:"expires_at"`
	MaxCommands          int     `json:"max_commands"`
	DefaultShellCommands int     `json:"default_shell_commands"`
}

func IntendedProcessKey(device, installation, pid string) string {
	return device + "/" + installation + "/" + pid
}

func (intended IntendedProcess) Key() string {
	return IntendedProcessKey(intended.DeviceID, intended.InstallationID, intended.Pid)
}
