package inventory

import (
	"bytes"
	"encoding/json"
	"time"
)

type Key struct {
	DeviceID       string `json:"device_id"`
	InstallationID string `json:"installation_id"`
}

func (key Key) String() string {
	return key.DeviceID + "/" + key.InstallationID
}

type Node struct {
	DeviceID           string          `json:"device_id"`
	InstallationID     string          `json:"installation_id"`
	CertFingerprint    *string         `json:"cert_fingerprint"`
	Lifecycle          string          `json:"lifecycle"`
	LifecycleReason    *string         `json:"lifecycle_reason"`
	LifecycleChangedAt *time.Time      `json:"lifecycle_changed_at"`
	Country            *string         `json:"country"`
	OSName             *string         `json:"os_name"`
	OSVersion          *string         `json:"os_version"`
	OSBuild            *string         `json:"os_build"`
	DuskVersion        *string         `json:"dusk_version"`
	HardwareClass      *string         `json:"hardware_class"`
	Tenant             *string         `json:"tenant"`
	Locale             *string         `json:"locale"`
	Hostname           *string         `json:"hostname"`
	Impl               *string         `json:"impl"`
	TargetArch         *string         `json:"target_arch"`
	FactsJSON          json.RawMessage `json:"facts"`
	ReportedVersion    *string         `json:"reported_version"`
	ReportedConfigHash *string         `json:"reported_config_hash"`
	ReportedServices   []string        `json:"reported_services"`
	ReportedAt         *time.Time      `json:"reported_at"`
	FactsNamespaceID   *string         `json:"facts_namespace_id"`
	FactsReadAt        *time.Time      `json:"facts_read_at"`
	EnrolledAt         *time.Time      `json:"enrolled_at"`
	FirstSeenAt        time.Time       `json:"first_seen_at"`
	UpdatedAt          time.Time       `json:"updated_at"`
	CredentialKind     *string         `json:"credential_kind"`
	CredentialRef      *string         `json:"credential_ref"`
	CredentialIssuer   *string         `json:"credential_issuer"`
	facts              map[string]any
	sortValue          *string
}

func (node *Node) Key() Key {
	return Key{DeviceID: node.DeviceID, InstallationID: node.InstallationID}
}

func (node *Node) Column(name string) (string, bool) {
	var value *string
	switch name {
	case "device_id":
		return node.DeviceID, true
	case "installation_id":
		return node.InstallationID, true
	case "lifecycle":
		return node.Lifecycle, true
	case "cert_fingerprint":
		value = node.CertFingerprint
	case "country":
		value = node.Country
	case "os_name":
		value = node.OSName
	case "os_version":
		value = node.OSVersion
	case "os_build":
		value = node.OSBuild
	case "dusk_version":
		value = node.DuskVersion
	case "hardware_class":
		value = node.HardwareClass
	case "tenant":
		value = node.Tenant
	case "locale":
		value = node.Locale
	case "hostname":
		value = node.Hostname
	case "impl":
		value = node.Impl
	case "target_arch":
		value = node.TargetArch
	case "reported_version":
		value = node.ReportedVersion
	case "reported_config_hash":
		value = node.ReportedConfigHash
	case "credential_kind":
		value = node.CredentialKind
	case "credential_ref":
		value = node.CredentialRef
	case "credential_issuer":
		value = node.CredentialIssuer
	}
	if value == nil {
		return "", false
	}
	return *value, true
}

func (node *Node) decodedFacts() map[string]any {
	if node.facts == nil {
		node.facts = map[string]any{}
		if len(node.FactsJSON) > 0 {
			decoder := json.NewDecoder(bytes.NewReader(node.FactsJSON))
			decoder.UseNumber()
			_ = decoder.Decode(&node.facts)
		}
	}
	return node.facts
}

func (node *Node) Fact(key string) (any, bool) {
	value, found := node.decodedFacts()[key]
	return value, found
}

func (node *Node) Breakdown(dimensions []string) map[string]string {
	breakdown := make(map[string]string, len(dimensions))
	for _, dimension := range dimensions {
		if value, found := node.Column(dimension); found {
			breakdown[dimension] = value
		} else {
			breakdown[dimension] = ""
		}
	}
	return breakdown
}

type PresenceSession struct {
	NamespaceID  string    `json:"namespace_id"`
	Epoch        int64     `json:"epoch"`
	Instance     string    `json:"instance"`
	InnerAddress string    `json:"inner_address"`
	ConnectedAt  time.Time `json:"connected_at"`
}

type Presence struct {
	Key
	Online         bool              `json:"online"`
	LastSeenAt     *time.Time        `json:"last_seen_at"`
	ConnectedAt    *time.Time        `json:"connected_at"`
	DisconnectedAt *time.Time        `json:"disconnected_at"`
	Sessions       []PresenceSession `json:"sessions"`
}
