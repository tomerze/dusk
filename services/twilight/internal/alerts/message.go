package alerts

import (
	"encoding/json"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"dusk/services/twilight/internal/config"
)

type notification struct {
	alert        Alert
	transition   string
	severity     Severity
	actor        *string
	at           time.Time
	key          string
	receiver     string
	hostname     *string
	campaignName *string
	settings     config.Alerts
	fields       map[string]any
}

type fact struct {
	label string
	value string
}

var kindLabels = map[string]string{
	KindProcessWithoutIntent:      "Process nobody intended",
	KindDefaultShellWithoutIntent: "Default shell used without intent",
	KindTargetMismatch:            "Process on the wrong node",
	KindProcessAfterDeadline:      "Calls after the deadline",
	KindPidReused:                 "Pid reused",
	KindProcessAfterResult:        "Calls after the result",
	KindProcessShape:              "Unexpected process shape",
	KindResultWithoutLedger:       "Result without a ledger entry",
	KindLedgerChainBroken:         "Ledger chain broken",
	KindQuarantineOverride:        "Quarantine override",
	KindRevocationNotEnforced:     "Revocation not enforced",
	KindEnrollmentRate:            "Enrollment spike",
	KindCampaignConflict:          "Campaign conflict",
	KindCampaignPausedByGate:      "Campaign paused by its gate",
	KindCampaignFailedByPolicy:    "Campaign failed by its policy",
}

var kindSummaries = map[string]string{
	KindRevocationNotEnforced: "A revoked or retired node is still connected to nightfall.",
	KindQuarantineOverride:    "A principal used its override to call a quarantined node.",
}

var consumedDetail = map[string]bool{
	"message": true, "device_id": true, "installation_id": true, "principal": true, "campaign_id": true,
	"campaign_name": true, "credential_ref": true, "credential_kind": true, "tenant": true,
}

func kindLabel(kind string) string {
	if label, known := kindLabels[kind]; known {
		return label
	}
	return strings.ReplaceAll(kind, "_", " ")
}

func (message *notification) parseDetail() error {
	message.fields = map[string]any{}
	if len(message.alert.Detail) == 0 {
		return nil
	}
	return json.Unmarshal(message.alert.Detail, &message.fields)
}

func (message notification) detail() map[string]any {
	if message.fields == nil {
		return map[string]any{}
	}
	return message.fields
}

func detailString(detail map[string]any, key string) string {
	text, _ := detail[key].(string)
	return strings.TrimSpace(text)
}

func renderValue(value any) string {
	if text, isText := value.(string); isText {
		return text
	}
	encoded, failure := json.Marshal(value)
	if failure != nil {
		return ""
	}
	return string(encoded)
}

func (message notification) alertURL() string {
	if message.settings.PublicURL == "" {
		return ""
	}
	return strings.TrimRight(message.settings.PublicURL, "/") + "/alerts?alert=" + strconv.FormatInt(message.alert.ID, 10)
}

func (message notification) runbookURL() string {
	if message.settings.RunbookURL == "" {
		return ""
	}
	return strings.ReplaceAll(message.settings.RunbookURL, config.RunbookKind, url.PathEscape(message.alert.Kind))
}

func (message notification) node() string {
	detail := message.detail()
	device, installation := detailString(detail, "device_id"), detailString(detail, "installation_id")
	ids := strings.Trim(device+"/"+installation, "/")
	if message.hostname != nil && *message.hostname != "" {
		if ids == "" {
			return *message.hostname
		}
		return *message.hostname + " (" + ids + ")"
	}
	return ids
}

func (message notification) campaign() string {
	identifier := detailString(message.detail(), "campaign_id")
	name := detailString(message.detail(), "campaign_name")
	if message.campaignName != nil && *message.campaignName != "" {
		name = *message.campaignName
	}
	switch {
	case name == "":
		return identifier
	case identifier == "":
		return name
	}
	return name + " (" + identifier + ")"
}

func (message notification) subject() string {
	detail := message.detail()
	name := detailString(detail, "campaign_name")
	if message.campaignName != nil && *message.campaignName != "" {
		name = *message.campaignName
	}
	if name != "" {
		return "in campaign " + name
	}
	if message.hostname != nil && *message.hostname != "" {
		return "on " + *message.hostname
	}
	if device := detailString(detail, "device_id"); device != "" {
		return "on device " + device
	}
	return ""
}

func (message notification) title() string {
	var prefix string
	switch message.transition {
	case TransitionReEscalated:
		prefix = "[" + strings.ToUpper(string(message.severity)) + "] Escalated: "
	case TransitionAcknowledged:
		prefix = "[ACKNOWLEDGED] "
	case TransitionResolved:
		prefix = "[RESOLVED] "
	default:
		prefix = "[" + strings.ToUpper(string(message.severity)) + "] "
	}
	return strings.TrimSpace(prefix + kindLabel(message.alert.Kind) + " " + message.subject())
}

func (message notification) summary() string {
	text := detailString(message.detail(), "message")
	if text == "" {
		text = kindSummaries[message.alert.Kind]
	}
	if text == "" {
		text = kindLabel(message.alert.Kind) + "."
	}
	first, size := utf8.DecodeRuneInString(text)
	return string(unicode.ToUpper(first)) + text[size:]
}

func (message notification) change() string {
	actor := "twilight"
	if message.actor != nil && *message.actor != "" {
		actor = *message.actor
	}
	at := message.at.UTC().Format(time.RFC3339)
	switch message.transition {
	case TransitionReEscalated:
		return "Raised again at severity " + string(message.severity) + " at " + at + "."
	case TransitionAcknowledged:
		return "Acknowledged by " + actor + " at " + at + "."
	case TransitionResolved:
		return "Resolved by " + actor + " at " + at + "."
	}
	return "Raised at " + at + "."
}

func (message notification) facts() []fact {
	detail := message.detail()
	facts := []fact{{"Severity", string(message.severity)}, {"Kind", message.alert.Kind}}
	if message.alert.Tenant != nil && *message.alert.Tenant != "" {
		facts = append(facts, fact{"Tenant", *message.alert.Tenant})
	}
	if node := message.node(); node != "" {
		facts = append(facts, fact{"Node", node})
	}
	if campaign := message.campaign(); campaign != "" {
		facts = append(facts, fact{"Campaign", campaign})
	}
	if principal := detailString(detail, "principal"); principal != "" {
		facts = append(facts, fact{"Principal", principal})
	}
	if credential := strings.TrimSpace(detailString(detail, "credential_kind") + " " + detailString(detail, "credential_ref")); credential != "" {
		facts = append(facts, fact{"Credential", credential})
	}
	facts = append(facts, fact{"Raised", message.alert.Time.UTC().Format(time.RFC3339)})
	if message.alert.Occurrences > 1 {
		facts = append(facts, fact{"Occurrences", strconv.FormatInt(message.alert.Occurrences, 10)})
	}
	return facts
}

func (message notification) evidence() []fact {
	detail := message.detail()
	keys := make([]string, 0, len(detail))
	for key := range detail {
		if !consumedDetail[key] {
			keys = append(keys, key)
		}
	}
	sort.Strings(keys)
	evidence := make([]fact, 0, len(keys))
	for _, key := range keys {
		evidence = append(evidence, fact{key, renderValue(detail[key])})
	}
	return evidence
}

func (message notification) links() []fact {
	var links []fact
	if alert := message.alertURL(); alert != "" {
		links = append(links, fact{"Open in twilight", alert})
	}
	if runbook := message.runbookURL(); runbook != "" {
		links = append(links, fact{"Runbook", runbook})
	}
	return links
}

func (message notification) plainText() string {
	var text strings.Builder
	text.WriteString(message.title() + "\n\n" + message.summary() + "\n" + message.change() + "\n\n")
	for _, entry := range message.facts() {
		text.WriteString(entry.label + ": " + entry.value + "\n")
	}
	if evidence := message.evidence(); len(evidence) > 0 {
		text.WriteString("\nEvidence\n")
		for _, entry := range evidence {
			text.WriteString(entry.label + ": " + entry.value + "\n")
		}
	}
	if links := message.links(); len(links) > 0 {
		text.WriteString("\n")
		for _, entry := range links {
			text.WriteString(entry.label + ": " + entry.value + "\n")
		}
	}
	return text.String()
}
