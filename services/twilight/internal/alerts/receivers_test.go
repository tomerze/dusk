package alerts

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"mime"
	"mime/multipart"
	"net/mail"
	"net/smtp"
	"strconv"
	"strings"
	"testing"

	"dusk/services/twilight/internal/config"
)

const routingKey = "0123456789abcdef0123456789abcdef"

func TestPagerDutyEventsFollowTheEventsAPI(test *testing.T) {
	schema := compileSchema(test, "pagerduty-events-v2.schema.json")
	actions := map[string]string{TransitionOpened: "trigger", TransitionReEscalated: "trigger", TransitionAcknowledged: "acknowledge", TransitionResolved: "resolve"}
	for _, transition := range transitions {
		payload, failure := pagerDutyPayloadOf(routingKey, sampleNotification(transition))
		if failure != nil {
			test.Fatal(failure)
		}
		conforms(test, schema, payload)
		var event pagerDutyEvent
		if failure := json.Unmarshal(payload, &event); failure != nil {
			test.Fatal(failure)
		}
		if event.EventAction != actions[transition] || event.DedupKey != "process_without_intent:12808937078074471924" {
			test.Errorf("%s: action %q dedup %q", transition, event.EventAction, event.DedupKey)
		}
		if transition == TransitionOpened {
			if event.Payload.Severity != "critical" || event.Payload.Source != "camera-17" || event.Payload.Group != "acme" || event.Payload.Class != KindProcessWithoutIntent {
				test.Errorf("payload %+v", event.Payload)
			}
			if len(event.Links) != 2 || event.Links[1].Href != "https://docs.example.org/stack/runbooks/process_without_intent/" {
				test.Errorf("links %+v", event.Links)
			}
		}
	}
	for severity, want := range map[Severity]string{High: "error", Medium: "warning", Low: "info"} {
		message := sampleNotification(TransitionOpened)
		message.severity = severity
		payload, _ := pagerDutyPayloadOf(routingKey, message)
		conforms(test, schema, payload)
		if !strings.Contains(string(payload), `"severity":"`+want+`"`) {
			test.Errorf("%s is not sent as %s: %s", severity, want, payload)
		}
	}
	long := strings.Repeat("x", 300)
	if key := PagerDutyDedupKey(long); len(key) != 71 || key != PagerDutyDedupKey(long) || !strings.HasPrefix(key, "sha256:") {
		test.Errorf("a long fingerprint gives %q", key)
	}
	message := sampleNotification(TransitionOpened)
	withDetail(test, &message, []byte(`{"message": "`+strings.Repeat("y", 2000)+`"}`))
	payload, _ := pagerDutyPayloadOf(routingKey, message)
	conforms(test, schema, payload)
}

func TestSlackMessagesFollowBlockKit(test *testing.T) {
	schema := compileSchema(test, "slack-incoming-webhook.schema.json")
	for _, transition := range transitions {
		payload, failure := slackPayload(sampleNotification(transition))
		if failure != nil {
			test.Fatal(failure)
		}
		conforms(test, schema, payload)
	}
	resolved, _ := slackPayload(sampleNotification(TransitionResolved))
	if !strings.Contains(string(resolved), "[RESOLVED] Process nobody intended on camera-17") || !strings.Contains(string(resolved), "Resolved by operator@example.org") {
		test.Errorf("the resolve follow-up does not say it resolved: %s", resolved)
	}
	message := sampleNotification(TransitionOpened)
	withDetail(test, &message, []byte(`{"message": "<!channel> & <https://evil.example|click>", "blob": "`+strings.Repeat("z", 5000)+`"}`))
	payload, _ := slackPayload(message)
	conforms(test, schema, payload)
	if strings.Contains(string(payload), "<!channel>") || strings.Contains(string(payload), "<https://evil.example") {
		test.Errorf("values from a node reach Slack unescaped: %s", payload)
	}
}

func TestTeamsCardsFollowTheAdaptiveCardFormat(test *testing.T) {
	schema := compileSchema(test, "teams-adaptive-card.schema.json")
	for _, transition := range transitions {
		payload, failure := teamsPayload(sampleNotification(transition))
		if failure != nil {
			test.Fatal(failure)
		}
		conforms(test, schema, payload)
	}
	message := sampleNotification(TransitionOpened)
	huge := map[string]string{"message": "[click](https://evil.example)"}
	for index := range 200 {
		huge[strings.Repeat("k", 5)+string(rune('a'+index%26))+strings.Repeat("_", index%7)+string(rune('A'+index/26))] = strings.Repeat("*", 2000)
	}
	encoded, failure := json.Marshal(huge)
	if failure != nil {
		test.Fatal(failure)
	}
	withDetail(test, &message, encoded)
	payload, _ := teamsPayload(message)
	conforms(test, schema, payload)
	if len(payload) > 28*1024 {
		test.Errorf("a card of %d bytes exceeds Teams' 28 KB", len(payload))
	}
	if strings.Contains(string(payload), "[click](https://evil.example)") {
		test.Errorf("Markdown from a node reaches Teams unescaped")
	}
}

func TestWebhookBodiesFollowTheDocumentedContract(test *testing.T) {
	schema := compileSchema(test, "dusk.alert-notification.schema.json")
	for _, transition := range transitions {
		payload, failure := webhookPayload(sampleNotification(transition))
		if failure != nil {
			test.Fatal(failure)
		}
		conforms(test, schema, payload)
	}
	first, _ := webhookPayload(sampleNotification(TransitionOpened))
	second, _ := webhookPayload(sampleNotification(TransitionOpened))
	if !bytes.Equal(first, second) {
		test.Error("the body of one notification differs between attempts")
	}
}

func TestSignatureIsHMACOverTimestampColonBody(test *testing.T) {
	if got := Sign([]byte("secret"), "1760065445", []byte(`{"a":1}`)); got != "bd6fec91f74db3f481b8afba92032ef215dd567d2df5b26ec23b86c6907e197b" {
		test.Fatalf("signature %q", got)
	}
	if Sign([]byte("secret"), "1", []byte("body")) == Sign([]byte("secret"), "2", []byte("body")) {
		test.Error("the timestamp is not signed")
	}
}

func TestEmailIsMultipartTextAndHTML(test *testing.T) {
	settings := config.EmailReceiver{Host: "smtp.example.org", From: "dusk@example.org", To: []string{"soc@example.org", "noc@example.org"}}
	opened, failure := emailPayload(settings, sampleNotification(TransitionOpened), sampleTime)
	if failure != nil {
		test.Fatal(failure)
	}
	parsed, failure := mail.ReadMessage(bufio.NewReader(bytes.NewReader(opened)))
	if failure != nil {
		test.Fatal(failure)
	}
	subject, _ := new(mime.WordDecoder).DecodeHeader(parsed.Header.Get("Subject"))
	thread := "<alert-42-" + strconv.FormatInt(sampleTime.UnixMicro(), 10) + ".on-call@example.org>"
	if subject != "[CRITICAL] Process nobody intended on camera-17" || parsed.Header.Get("Message-ID") != thread ||
		parsed.Header.Get("To") != "soc@example.org, noc@example.org" || parsed.Header.Get("Auto-Submitted") != "auto-generated" {
		test.Fatalf("headers %v", parsed.Header)
	}
	parts := emailParts(test, parsed)
	if !strings.Contains(parts["text/plain"], "Runbook: https://docs.example.org/stack/runbooks/process_without_intent/") ||
		!strings.Contains(parts["text/plain"], "pid: 12808937078074471924") {
		test.Errorf("text part %q", parts["text/plain"])
	}
	if !strings.Contains(parts["text/html"], `<a href="https://twilight.example.org/alerts?alert=42">Open in twilight</a>`) ||
		!strings.Contains(parts["text/html"], "camera-17") {
		test.Errorf("html part %q", parts["text/html"])
	}
	resolved, _ := emailPayload(settings, sampleNotification(TransitionResolved), sampleTime)
	answer, _ := mail.ReadMessage(bufio.NewReader(bytes.NewReader(resolved)))
	if answer.Header.Get("In-Reply-To") != thread || answer.Header.Get("References") != thread ||
		answer.Header.Get("Message-ID") != "<0192f0a4-5b6c-7d8e-9f01-23456789abcd@example.org>" {
		test.Errorf("a resolve is not threaded under its alert: %v", answer.Header)
	}
	hostile := sampleNotification(TransitionOpened)
	withDetail(test, &hostile, []byte(`{"message": "<script>alert(1)</script>"}`))
	escaped, _ := emailPayload(settings, hostile, sampleTime)
	hostileMessage, _ := mail.ReadMessage(bufio.NewReader(bytes.NewReader(escaped)))
	if html := emailParts(test, hostileMessage)["text/html"]; strings.Contains(html, "<script>") || !strings.Contains(html, "&lt;script&gt;") {
		test.Errorf("HTML from a node reaches the email unescaped: %s", html)
	}
}

func emailParts(test *testing.T, parsed *mail.Message) map[string]string {
	test.Helper()
	mediaType, parameters, failure := mime.ParseMediaType(parsed.Header.Get("Content-Type"))
	if failure != nil || mediaType != "multipart/alternative" {
		test.Fatalf("content type %q %v", mediaType, failure)
	}
	reader := multipart.NewReader(parsed.Body, parameters["boundary"])
	parts := map[string]string{}
	for {
		part, failure := reader.NextPart()
		if failure == io.EOF {
			return parts
		}
		if failure != nil {
			test.Fatal(failure)
		}
		decoded, failure := io.ReadAll(part)
		if failure != nil {
			test.Fatal(failure)
		}
		parts[strings.Split(part.Header.Get("Content-Type"), ";")[0]] = string(decoded)
	}
}

func TestLoginAuthAnswersTheChallenges(test *testing.T) {
	auth := loginAuth{username: "dusk", password: "secret"}
	if _, _, failure := auth.Start(&smtp.ServerInfo{Name: "smtp.example.org", TLS: false}); failure == nil {
		test.Fatal("LOGIN over plaintext was allowed")
	}
	mechanism, _, failure := auth.Start(&smtp.ServerInfo{Name: "smtp.example.org", TLS: true})
	if failure != nil || mechanism != "LOGIN" {
		test.Fatalf("start %q %v", mechanism, failure)
	}
	if answer, _ := auth.Next([]byte("Username:"), true); string(answer) != "dusk" {
		test.Errorf("username answer %q", answer)
	}
	if answer, _ := auth.Next([]byte("Password:"), true); string(answer) != "secret" {
		test.Errorf("password answer %q", answer)
	}
	if _, failure := auth.Next([]byte("Token:"), true); failure == nil {
		test.Error("an unknown challenge was answered")
	}
	if answer, failure := auth.Next(nil, false); answer != nil || failure != nil {
		test.Errorf("the end of the exchange %q %v", answer, failure)
	}
}
