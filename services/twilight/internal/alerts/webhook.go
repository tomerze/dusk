package alerts

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"strconv"
	"time"
)

const (
	WebhookSchema          = "dusk.alert-notification/v1"
	SignatureHeader        = "Dusk-Signature"
	TimestampHeader        = "Dusk-Timestamp"
	IdempotencyKeyHeader   = "Idempotency-Key"
	webhookTimestampFormat = time.RFC3339Nano
)

type webhookSender struct {
	client     *http.Client
	url        string
	secretFile string
}

type webhookAlert struct {
	ID             int64           `json:"id"`
	Time           string          `json:"time"`
	LastSeenAt     string          `json:"last_seen_at"`
	Occurrences    int64           `json:"occurrences"`
	Severity       Severity        `json:"severity"`
	Kind           string          `json:"kind"`
	Fingerprint    string          `json:"fingerprint"`
	Tenant         *string         `json:"tenant"`
	Detail         json.RawMessage `json:"detail"`
	AcknowledgedBy *string         `json:"acknowledged_by"`
	AcknowledgedAt *string         `json:"acknowledged_at"`
	ResolvedBy     *string         `json:"resolved_by"`
	ResolvedAt     *string         `json:"resolved_at"`
}

type webhookLink struct {
	Alert   *string `json:"alert"`
	Runbook *string `json:"runbook"`
}

type webhookBody struct {
	Schema       string       `json:"schema"`
	ID           string       `json:"id"`
	Receiver     string       `json:"receiver"`
	Transition   string       `json:"transition"`
	TransitionAt string       `json:"transition_at"`
	Actor        *string      `json:"actor"`
	Severity     Severity     `json:"severity"`
	Title        string       `json:"title"`
	Summary      string       `json:"summary"`
	Hostname     *string      `json:"hostname"`
	CampaignName *string      `json:"campaign_name"`
	Alert        webhookAlert `json:"alert"`
	Links        webhookLink  `json:"links"`
}

func formatOptionalTime(at *time.Time) *string {
	if at == nil {
		return nil
	}
	formatted := at.UTC().Format(webhookTimestampFormat)
	return &formatted
}

func optionalText(text string) *string {
	if text == "" {
		return nil
	}
	return &text
}

func webhookPayload(message notification) ([]byte, error) {
	alert := message.alert
	detail := alert.Detail
	if len(detail) == 0 {
		detail = json.RawMessage(`{}`)
	}
	return json.Marshal(webhookBody{
		Schema: WebhookSchema, ID: message.key, Receiver: message.receiver, Transition: message.transition,
		TransitionAt: message.at.UTC().Format(webhookTimestampFormat), Actor: message.actor, Severity: message.severity,
		Title: message.title(), Summary: message.summary(), Hostname: message.hostname, CampaignName: message.campaignName,
		Alert: webhookAlert{
			ID: alert.ID, Time: alert.Time.UTC().Format(webhookTimestampFormat), LastSeenAt: alert.LastSeenAt.UTC().Format(webhookTimestampFormat),
			Occurrences: alert.Occurrences, Severity: alert.Severity, Kind: alert.Kind, Fingerprint: alert.Fingerprint, Tenant: alert.Tenant,
			Detail: detail, AcknowledgedBy: alert.AcknowledgedBy, AcknowledgedAt: formatOptionalTime(alert.AcknowledgedAt),
			ResolvedBy: alert.ResolvedBy, ResolvedAt: formatOptionalTime(alert.ResolvedAt),
		},
		Links: webhookLink{Alert: optionalText(message.alertURL()), Runbook: optionalText(message.runbookURL())},
	})
}

func Sign(secret []byte, timestamp string, body []byte) string {
	signer := hmac.New(sha256.New, secret)
	signer.Write([]byte(timestamp))
	signer.Write([]byte(":"))
	signer.Write(body)
	return hex.EncodeToString(signer.Sum(nil))
}

func (sender webhookSender) send(operation context.Context, message notification) error {
	secret, failure := readSecret(sender.secretFile)
	if failure != nil {
		return deliveryError{message: failure.Error()}
	}
	body, failure := webhookPayload(message)
	if failure != nil {
		return deliveryError{message: "the payload could not be encoded: " + failure.Error(), permanent: true}
	}
	timestamp := strconv.FormatInt(time.Now().Unix(), 10)
	return postJSON(operation, sender.client, sender.url, body, map[string]string{
		TimestampHeader:      timestamp,
		SignatureHeader:      Sign([]byte(secret), timestamp, body),
		IdempotencyKeyHeader: message.key,
	})
}
