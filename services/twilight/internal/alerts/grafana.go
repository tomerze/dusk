package alerts

import (
	"crypto/sha256"
	"encoding/hex"
	"regexp"
	"strings"

	"gopkg.in/yaml.v3"

	"dusk/services/twilight/internal/config"
)

const grafanaUIDLimit = 40

type grafanaReceiver struct {
	UID                   string         `yaml:"uid"`
	Type                  string         `yaml:"type"`
	DisableResolveMessage bool           `yaml:"disableResolveMessage"`
	Settings              map[string]any `yaml:"settings"`
}

type grafanaContactPoint struct {
	OrgID     int               `yaml:"orgId"`
	Name      string            `yaml:"name"`
	Receivers []grafanaReceiver `yaml:"receivers"`
}

type grafanaRoute struct {
	Receiver       string     `yaml:"receiver"`
	ObjectMatchers [][]string `yaml:"object_matchers,omitempty"`
	Continue       bool       `yaml:"continue"`
}

type grafanaPolicy struct {
	OrgID          int            `yaml:"orgId"`
	Receiver       string         `yaml:"receiver"`
	GroupBy        []string       `yaml:"group_by"`
	GroupWait      string         `yaml:"group_wait"`
	GroupInterval  string         `yaml:"group_interval"`
	RepeatInterval string         `yaml:"repeat_interval"`
	Routes         []grafanaRoute `yaml:"routes"`
}

type grafanaTemplate struct {
	OrgID    int    `yaml:"orgId"`
	Name     string `yaml:"name"`
	Template string `yaml:"template"`
}

type grafanaProvisioning struct {
	APIVersion    int                   `yaml:"apiVersion"`
	ContactPoints []grafanaContactPoint `yaml:"contactPoints,omitempty"`
	Policies      []grafanaPolicy       `yaml:"policies,omitempty"`
	Templates     []grafanaTemplate     `yaml:"templates"`
}

var grafanaMatcherSpecial = regexp.MustCompile(`[.*+?()|\[\]{}^$\\]`)

func grafanaUID(name string) string {
	uid := "dusk-" + name
	if len(uid) <= grafanaUIDLimit {
		return uid
	}
	digest := sha256.Sum256([]byte(name))
	return "dusk-" + hex.EncodeToString(digest[:])[:grafanaUIDLimit-5]
}

func grafanaFile(path string) string {
	return "$__file{" + path + "}"
}

func grafanaAlternatives(values []string) string {
	quoted := make([]string, len(values))
	for index, value := range values {
		quoted[index] = grafanaMatcherSpecial.ReplaceAllStringFunc(value, func(special string) string { return `\` + special })
	}
	return strings.Join(quoted, "|")
}

func grafanaTemplates(settings config.Alerts) string {
	runbook := strings.NewReplacer("{{", `{{ "{{" }}`, "}}", `{{ "}}" }}`).Replace(settings.RunbookURL)
	runbook = strings.ReplaceAll(runbook, config.RunbookKind, "{{ . }}")
	return `{{ define "dusk.runbook" }}` + runbook + `{{ end }}
{{ define "dusk.title" }}[{{ if eq .Status "firing" }}{{ .CommonLabels.severity | toUpper }}{{ else }}RESOLVED{{ end }}] {{ .CommonLabels.alertname }}{{ end }}
{{ define "dusk.message" }}{{ range .Alerts }}{{ if eq .Status "resolved" }}Resolved: {{ end }}{{ .Annotations.summary }}
{{ .Annotations.description }}
Severity: {{ .Labels.severity }}
Kind: {{ .Labels.kind }}
Runbook: {{ if .Labels.kind }}{{ template "dusk.runbook" .Labels.kind }}{{ end }}
{{ if .GeneratorURL }}Rule: {{ .GeneratorURL }}
{{ end }}
{{ end }}{{ end }}
`
}

func grafanaSettings(receiver config.Receiver) (string, map[string]any) {
	title := `{{ template "dusk.title" . }}`
	message := `{{ template "dusk.message" . }}`
	switch {
	case receiver.PagerDuty != nil:
		settings := map[string]any{
			"integrationKey": grafanaFile(receiver.PagerDuty.RoutingKeyFile),
			"severity":       `{{ if eq .CommonLabels.severity "critical" }}critical{{ else if eq .CommonLabels.severity "high" }}error{{ else if eq .CommonLabels.severity "medium" }}warning{{ else }}info{{ end }}`,
			"summary":        title,
			"class":          `{{ .CommonLabels.kind }}`,
			"component":      "dusk",
			"details":        map[string]string{"message": message},
		}
		if receiver.PagerDuty.URL != "" {
			settings["url"] = receiver.PagerDuty.URL
		}
		return "pagerduty", settings
	case receiver.Slack != nil:
		return "slack", map[string]any{"url": grafanaFile(receiver.Slack.WebhookURLFile), "title": title, "text": message}
	case receiver.Teams != nil:
		return "teams", map[string]any{"url": grafanaFile(receiver.Teams.WebhookURLFile), "title": title, "message": message}
	case receiver.Email != nil:
		return "email", map[string]any{"addresses": strings.Join(receiver.Email.To, ";"), "singleEmail": true, "subject": title, "message": message}
	case receiver.Webhook != nil:
		return "webhook", map[string]any{"url": receiver.Webhook.URL, "httpMethod": "POST", "title": title, "message": message,
			"hmacConfig": map[string]string{"secret": grafanaFile(receiver.Webhook.SecretFile), "header": SignatureHeader, "timestampHeader": TimestampHeader}}
	}
	return "", nil
}

func GrafanaProvisioning(settings config.Alerts) ([]byte, error) {
	provisioning := grafanaProvisioning{APIVersion: 1, Templates: []grafanaTemplate{{OrgID: 1, Name: "dusk", Template: grafanaTemplates(settings)}}}
	for _, receiver := range settings.Receivers {
		kind, values := grafanaSettings(receiver)
		provisioning.ContactPoints = append(provisioning.ContactPoints, grafanaContactPoint{OrgID: 1, Name: receiver.Name,
			Receivers: []grafanaReceiver{{UID: grafanaUID(receiver.Name), Type: kind, Settings: values}}})
	}
	if len(settings.Routes) > 0 {
		policy := grafanaPolicy{OrgID: 1, Receiver: settings.Routes[0].Receiver, GroupBy: []string{"alertname", "kind"},
			GroupWait: "30s", GroupInterval: "5m", RepeatInterval: "4h"}
		for _, route := range settings.Routes {
			entry := grafanaRoute{Receiver: route.Receiver, Continue: true}
			for _, matcher := range []struct {
				label  string
				values []string
			}{{"severity", route.Severities}, {"kind", route.Kinds}, {"tenant", route.Tenants}} {
				if len(matcher.values) > 0 {
					entry.ObjectMatchers = append(entry.ObjectMatchers, []string{matcher.label, "=~", grafanaAlternatives(matcher.values)})
				}
			}
			policy.Routes = append(policy.Routes, entry)
		}
		provisioning.Policies = []grafanaPolicy{policy}
	}
	return yaml.Marshal(provisioning)
}
