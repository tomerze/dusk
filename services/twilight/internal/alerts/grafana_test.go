package alerts

import (
	"bytes"
	"strings"
	"testing"
	"text/template"

	"gopkg.in/yaml.v3"

	"dusk/services/twilight/internal/config"
)

func TestGrafanaGetsTheSameReceiversAndRoutes(test *testing.T) {
	settings := config.Alerts{
		PublicURL: "https://twilight.example.org", RunbookURL: "https://docs.example.org/stack/runbooks/{kind}/",
		Receivers: []config.Receiver{
			{Name: "on-call", PagerDuty: &config.PagerDutyReceiver{RoutingKeyFile: "/run/secrets/alerts/pagerduty", URL: "http://sink.example.org/pagerduty"}},
			{Name: "chat", Slack: &config.SlackReceiver{WebhookURLFile: "/run/secrets/alerts/slack"}},
			{Name: "soc", Email: &config.EmailReceiver{Host: "smtp.example.org", From: "dusk@example.org", To: []string{"a@example.org", "b@example.org"}}},
			{Name: "noc", Teams: &config.TeamsReceiver{WebhookURLFile: "/run/secrets/alerts/teams"}},
			{Name: "siem", Webhook: &config.WebhookReceiver{URL: "https://siem.example.org/dusk", SecretFile: "/run/secrets/alerts/webhook"}},
			{Name: strings.Repeat("long-name-", 6), Slack: &config.SlackReceiver{WebhookURLFile: "/s"}},
		},
		Routes: []config.Route{
			{Receiver: "on-call", Severities: []string{"critical", "high"}},
			{Receiver: "chat", Severities: []string{"medium"}, Kinds: []string{"vector_messages_dropped"}, Tenants: []string{"a.b"}},
			{Receiver: "siem"},
		},
	}
	content, failure := GrafanaProvisioning(settings)
	if failure != nil {
		test.Fatal(failure)
	}
	var parsed grafanaProvisioning
	if failure := yaml.Unmarshal(content, &parsed); failure != nil {
		test.Fatal(failure)
	}
	types := map[string]grafanaReceiver{}
	for _, point := range parsed.ContactPoints {
		if len(point.Receivers) != 1 || len(point.Receivers[0].UID) > grafanaUIDLimit {
			test.Fatalf("contact point %+v", point)
		}
		types[point.Name] = point.Receivers[0]
	}
	if types["on-call"].Type != "pagerduty" || types["on-call"].Settings["integrationKey"] != "$__file{/run/secrets/alerts/pagerduty}" ||
		types["on-call"].Settings["url"] != "http://sink.example.org/pagerduty" {
		test.Errorf("pagerduty %+v", types["on-call"])
	}
	if types["chat"].Type != "slack" || types["chat"].Settings["url"] != "$__file{/run/secrets/alerts/slack}" {
		test.Errorf("slack %+v", types["chat"])
	}
	if types["soc"].Type != "email" || types["soc"].Settings["addresses"] != "a@example.org;b@example.org" {
		test.Errorf("email %+v", types["soc"])
	}
	if types["noc"].Type != "teams" || types["noc"].Settings["url"] != "$__file{/run/secrets/alerts/teams}" {
		test.Errorf("teams %+v", types["noc"])
	}
	hmacSettings, _ := types["siem"].Settings["hmacConfig"].(map[string]any)
	if types["siem"].Type != "webhook" || hmacSettings["secret"] != "$__file{/run/secrets/alerts/webhook}" || hmacSettings["header"] != SignatureHeader || hmacSettings["timestampHeader"] != TimestampHeader {
		test.Errorf("webhook %+v", types["siem"])
	}
	if len(parsed.Policies) != 1 || parsed.Policies[0].Receiver != "on-call" || len(parsed.Policies[0].Routes) != 3 {
		test.Fatalf("policies %+v", parsed.Policies)
	}
	routes := parsed.Policies[0].Routes
	if strings.Join(routes[0].ObjectMatchers[0], " ") != "severity =~ critical|high" || !routes[0].Continue {
		test.Errorf("first route %+v", routes[0])
	}
	if len(routes[1].ObjectMatchers) != 3 || routes[1].ObjectMatchers[2][2] != `a\.b` || len(routes[2].ObjectMatchers) != 0 {
		test.Errorf("routes %+v", routes)
	}
	if len(parsed.Templates) != 1 {
		test.Fatalf("templates %+v", parsed.Templates)
	}
	rendered, failure := template.New("dusk").Funcs(template.FuncMap{"toUpper": strings.ToUpper}).Parse(parsed.Templates[0].Template)
	if failure != nil {
		test.Fatal(failure)
	}
	type alert struct {
		Status       string
		Labels       map[string]string
		Annotations  map[string]string
		GeneratorURL string
	}
	data := map[string]any{"Status": "firing", "CommonLabels": map[string]string{"severity": "high", "alertname": "Messages dropped by Vector", "kind": "vector_messages_dropped"},
		"Alerts": []alert{{Status: "firing", Labels: map[string]string{"severity": "high", "kind": "vector_messages_dropped"},
			Annotations: map[string]string{"summary": "Vector dropped 3 record(s).", "description": "Check the producer."}, GeneratorURL: "https://grafana.example.org/alerting/grafana/x/view"}}}
	var output bytes.Buffer
	if failure := rendered.ExecuteTemplate(&output, "dusk.title", data); failure != nil || output.String() != "[HIGH] Messages dropped by Vector" {
		test.Fatalf("title %q %v", output.String(), failure)
	}
	output.Reset()
	if failure := rendered.ExecuteTemplate(&output, "dusk.message", data); failure != nil ||
		!strings.Contains(output.String(), "Runbook: https://docs.example.org/stack/runbooks/vector_messages_dropped/") {
		test.Fatalf("message %q %v", output.String(), failure)
	}
	empty, failure := GrafanaProvisioning(config.Alerts{RunbookURL: "https://docs.example.org/{kind}/"})
	if failure != nil || strings.Contains(string(empty), "contactPoints") || strings.Contains(string(empty), "policies") {
		test.Fatalf("no receivers: %s %v", empty, failure)
	}
}
