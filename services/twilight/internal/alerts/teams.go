package alerts

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
)

const (
	teamsCardVersion  = "1.4"
	teamsValueLimit   = 250
	teamsEvidenceRows = 20
)

type teamsSender struct {
	client         *http.Client
	webhookURLFile string
}

type teamsFact struct {
	Title string `json:"title"`
	Value string `json:"value"`
}

type teamsElement struct {
	Type     string      `json:"type"`
	Text     string      `json:"text,omitempty"`
	Weight   string      `json:"weight,omitempty"`
	Size     string      `json:"size,omitempty"`
	Color    string      `json:"color,omitempty"`
	IsSubtle bool        `json:"isSubtle,omitempty"`
	Wrap     bool        `json:"wrap,omitempty"`
	Spacing  string      `json:"spacing,omitempty"`
	Facts    []teamsFact `json:"facts,omitempty"`
}

type teamsAction struct {
	Type  string `json:"type"`
	Title string `json:"title"`
	URL   string `json:"url"`
}

type teamsCard struct {
	Schema  string         `json:"$schema"`
	Type    string         `json:"type"`
	Version string         `json:"version"`
	Body    []teamsElement `json:"body"`
	Actions []teamsAction  `json:"actions,omitempty"`
}

type teamsAttachment struct {
	ContentType string    `json:"contentType"`
	ContentURL  *string   `json:"contentUrl"`
	Content     teamsCard `json:"content"`
}

type teamsMessage struct {
	Type        string            `json:"type"`
	Attachments []teamsAttachment `json:"attachments"`
}

var teamsEscaper = strings.NewReplacer(`\`, `\\`, "*", `\*`, "_", `\_`, "[", `\[`, "]", `\]`, "(", `\(`, ")", `\)`, "`", "\\`", "~", `\~`, "#", `\#`, ">", `\>`)

func teamsEscape(text string, limit int) string {
	return teamsEscaper.Replace(truncate(text, limit))
}

func teamsPayload(message notification) ([]byte, error) {
	color := "Default"
	switch {
	case message.transition == TransitionResolved:
		color = "Good"
	case message.transition == TransitionAcknowledged:
	case message.severity == Critical || message.severity == High:
		color = "Attention"
	default:
		color = "Warning"
	}
	body := []teamsElement{
		{Type: "TextBlock", Text: teamsEscape(message.title(), teamsValueLimit), Weight: "Bolder", Size: "Medium", Color: color, Wrap: true},
		{Type: "TextBlock", Text: teamsEscape(message.summary(), 4*teamsValueLimit), Wrap: true},
		{Type: "TextBlock", Text: teamsEscape(message.change(), teamsValueLimit), IsSubtle: true, Spacing: "None", Wrap: true},
	}
	if message.transition == TransitionOpened || message.transition == TransitionReEscalated {
		var facts []teamsFact
		for _, entry := range message.facts() {
			facts = append(facts, teamsFact{Title: entry.label, Value: teamsEscape(entry.value, teamsValueLimit)})
		}
		body = append(body, teamsElement{Type: "FactSet", Facts: facts})
		if evidence := message.evidence(); len(evidence) > 0 {
			var rows []teamsFact
			for _, entry := range evidence[:min(len(evidence), teamsEvidenceRows)] {
				rows = append(rows, teamsFact{Title: teamsEscape(entry.label, 100), Value: teamsEscape(entry.value, teamsValueLimit)})
			}
			body = append(body, teamsElement{Type: "TextBlock", Text: "Evidence", Weight: "Bolder", Wrap: true}, teamsElement{Type: "FactSet", Facts: rows})
		}
	}
	var actions []teamsAction
	for _, link := range message.links() {
		actions = append(actions, teamsAction{Type: "Action.OpenUrl", Title: link.label, URL: link.value})
	}
	return json.Marshal(teamsMessage{Type: "message", Attachments: []teamsAttachment{{
		ContentType: "application/vnd.microsoft.card.adaptive",
		Content: teamsCard{Schema: "http://adaptivecards.io/schemas/adaptive-card.json", Type: "AdaptiveCard", Version: teamsCardVersion,
			Body: body, Actions: actions},
	}}})
}

func (sender teamsSender) send(operation context.Context, message notification) error {
	webhookURL, failure := readSecret(sender.webhookURLFile)
	if failure != nil {
		return deliveryError{message: failure.Error()}
	}
	body, failure := teamsPayload(message)
	if failure != nil {
		return deliveryError{message: "the card could not be encoded: " + failure.Error(), permanent: true}
	}
	return postJSON(operation, sender.client, webhookURL, body, nil)
}
