package alerts

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"strings"
	"time"
)

const (
	pagerDutySummaryLimit  = 1024
	pagerDutyDedupKeyLimit = 255
)

type pagerDutySender struct {
	client         *http.Client
	routingKeyFile string
	url            string
}

type pagerDutyPayload struct {
	Summary       string         `json:"summary"`
	Source        string         `json:"source"`
	Severity      string         `json:"severity"`
	Timestamp     string         `json:"timestamp,omitempty"`
	Component     string         `json:"component,omitempty"`
	Group         string         `json:"group,omitempty"`
	Class         string         `json:"class,omitempty"`
	CustomDetails map[string]any `json:"custom_details,omitempty"`
}

type pagerDutyLink struct {
	Href string `json:"href"`
	Text string `json:"text,omitempty"`
}

type pagerDutyEvent struct {
	RoutingKey  string            `json:"routing_key"`
	EventAction string            `json:"event_action"`
	DedupKey    string            `json:"dedup_key"`
	Payload     *pagerDutyPayload `json:"payload,omitempty"`
	Links       []pagerDutyLink   `json:"links,omitempty"`
	Client      string            `json:"client,omitempty"`
	ClientURL   string            `json:"client_url,omitempty"`
}

var pagerDutySeverities = map[Severity]string{Critical: "critical", High: "error", Medium: "warning", Low: "info"}

func truncate(text string, limit int) string {
	runes := []rune(text)
	if len(runes) <= limit {
		return text
	}
	return string(runes[:limit-1]) + "…"
}

func PagerDutyDedupKey(fingerprint string) string {
	if len(fingerprint) <= pagerDutyDedupKeyLimit {
		return fingerprint
	}
	digest := sha256.Sum256([]byte(fingerprint))
	return "sha256:" + hex.EncodeToString(digest[:])
}

func pagerDutyPayloadOf(routingKey string, message notification) ([]byte, error) {
	event := pagerDutyEvent{RoutingKey: routingKey, DedupKey: PagerDutyDedupKey(message.alert.Fingerprint)}
	switch message.transition {
	case TransitionAcknowledged:
		event.EventAction = "acknowledge"
		return json.Marshal(event)
	case TransitionResolved:
		event.EventAction = "resolve"
		return json.Marshal(event)
	}
	event.EventAction = "trigger"
	details := map[string]any{"summary": message.summary(), "change": message.change(), "alert_id": message.alert.ID}
	for _, entry := range message.facts() {
		details[strings.ToLower(entry.label)] = entry.value
	}
	evidence := map[string]string{}
	for _, entry := range message.evidence() {
		evidence[entry.label] = entry.value
	}
	if len(evidence) > 0 {
		details["evidence"] = evidence
	}
	source := "twilight"
	switch {
	case message.hostname != nil && *message.hostname != "":
		source = *message.hostname
	case message.node() != "":
		source = message.node()
	}
	payload := &pagerDutyPayload{
		Summary: truncate(message.title()+": "+message.summary(), pagerDutySummaryLimit), Source: source,
		Severity: pagerDutySeverities[message.severity], Timestamp: message.alert.Time.UTC().Format(time.RFC3339Nano),
		Component: message.node(), Class: message.alert.Kind, CustomDetails: details,
	}
	if message.alert.Tenant != nil {
		payload.Group = *message.alert.Tenant
	}
	event.Payload = payload
	for _, link := range message.links() {
		event.Links = append(event.Links, pagerDutyLink{Href: link.value, Text: link.label})
	}
	event.Client = "twilight"
	event.ClientURL = message.settings.PublicURL
	return json.Marshal(event)
}

func (sender pagerDutySender) send(operation context.Context, message notification) error {
	routingKey, failure := readSecret(sender.routingKeyFile)
	if failure != nil {
		return deliveryError{message: failure.Error()}
	}
	body, failure := pagerDutyPayloadOf(routingKey, message)
	if failure != nil {
		return deliveryError{message: "the event could not be encoded: " + failure.Error(), permanent: true}
	}
	return postJSON(operation, sender.client, sender.url, body, nil)
}
