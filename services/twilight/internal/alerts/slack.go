package alerts

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
)

const (
	slackHeaderLimit  = 150
	slackSectionLimit = 3000
	slackFieldLimit   = 2000
	slackFieldCount   = 10
)

type slackSender struct {
	client         *http.Client
	webhookURLFile string
}

type slackText struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

type slackBlock struct {
	Type   string      `json:"type"`
	Text   *slackText  `json:"text,omitempty"`
	Fields []slackText `json:"fields,omitempty"`
}

type slackMessage struct {
	Text   string       `json:"text"`
	Blocks []slackBlock `json:"blocks"`
}

var slackEscaper = strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;")

func slackEscape(text string) string {
	return slackEscaper.Replace(text)
}

func slackPayload(message notification) ([]byte, error) {
	title := message.title()
	blocks := []slackBlock{
		{Type: "header", Text: &slackText{Type: "plain_text", Text: truncate(title, slackHeaderLimit)}},
		{Type: "section", Text: &slackText{Type: "mrkdwn", Text: truncate(slackEscape(message.summary())+"\n"+slackEscape(message.change()), slackSectionLimit)}},
	}
	if message.transition == TransitionOpened || message.transition == TransitionReEscalated {
		var fields []slackText
		for _, entry := range message.facts() {
			if len(fields) == slackFieldCount {
				break
			}
			fields = append(fields, slackText{Type: "mrkdwn", Text: truncate("*"+entry.label+"*\n"+slackEscape(entry.value), slackFieldLimit)})
		}
		blocks = append(blocks, slackBlock{Type: "section", Fields: fields})
		if evidence := message.evidence(); len(evidence) > 0 {
			lines := []string{"*Evidence*"}
			for _, entry := range evidence {
				lines = append(lines, "`"+slackEscape(entry.label)+"` "+slackEscape(entry.value))
			}
			blocks = append(blocks, slackBlock{Type: "section", Text: &slackText{Type: "mrkdwn", Text: truncate(strings.Join(lines, "\n"), slackSectionLimit)}})
		}
	}
	if links := message.links(); len(links) > 0 {
		var parts []string
		for _, link := range links {
			parts = append(parts, "<"+link.value+"|"+link.label+">")
		}
		blocks = append(blocks, slackBlock{Type: "section", Text: &slackText{Type: "mrkdwn", Text: strings.Join(parts, "  ·  ")}})
	}
	return json.Marshal(slackMessage{Text: slackEscape(title + "\n" + message.summary()), Blocks: blocks})
}

func (sender slackSender) send(operation context.Context, message notification) error {
	webhookURL, failure := readSecret(sender.webhookURLFile)
	if failure != nil {
		return deliveryError{message: failure.Error()}
	}
	body, failure := slackPayload(message)
	if failure != nil {
		return deliveryError{message: "the message could not be encoded: " + failure.Error(), permanent: true}
	}
	return postJSON(operation, sender.client, webhookURL, body, nil)
}
