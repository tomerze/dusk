package config

import (
	"net/url"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

type Alerts struct {
	EnrollmentRatePerMinute int        `yaml:"enrollment_rate_per_minute"`
	PublicURL               string     `yaml:"public_url"`
	RunbookURL              string     `yaml:"runbook_url"`
	DeliveryHorizonSeconds  int        `yaml:"delivery_horizon_seconds"`
	Receivers               []Receiver `yaml:"receivers"`
	Routes                  []Route    `yaml:"routes"`
}

type Receiver struct {
	Name      string             `yaml:"name"`
	PagerDuty *PagerDutyReceiver `yaml:"pagerduty"`
	Slack     *SlackReceiver     `yaml:"slack"`
	Teams     *TeamsReceiver     `yaml:"teams"`
	Webhook   *WebhookReceiver   `yaml:"webhook"`
}

type PagerDutyReceiver struct {
	RoutingKeyFile string `yaml:"routing_key_file"`
	URL            string `yaml:"url"`
}

type SlackReceiver struct {
	WebhookURLFile string `yaml:"webhook_url_file"`
}

type TeamsReceiver struct {
	WebhookURLFile string `yaml:"webhook_url_file"`
}

type WebhookReceiver struct {
	URL        string `yaml:"url"`
	SecretFile string `yaml:"secret_file"`
}

type Route struct {
	Receiver   string   `yaml:"receiver"`
	Severities []string `yaml:"severities"`
	Kinds      []string `yaml:"kinds"`
	Tenants    []string `yaml:"tenants"`
}

const (
	PagerDutyEventsURL = "https://events.pagerduty.com/v2/enqueue"
	RunbookKind        = "{kind}"
)

var (
	Severities    = []string{"critical", "high", "medium", "low"}
	receiverName  = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,62}$`)
	receiverTypes = []string{
		"pagerduty",
		"slack",
		"teams",
		"webhook",
	}
)

func (pagerDuty PagerDutyReceiver) EventsURL() string {
	if pagerDuty.URL == "" {
		return PagerDutyEventsURL
	}
	return pagerDuty.URL
}

func (route Route) Matches(severity, kind string, tenant *string) bool {
	if len(route.Severities) > 0 && !slices.Contains(route.Severities, severity) {
		return false
	}
	if len(route.Kinds) > 0 && !slices.Contains(route.Kinds, kind) {
		return false
	}
	return len(route.Tenants) == 0 || (tenant != nil && slices.Contains(route.Tenants, *tenant))
}

func webURL(text string) bool {
	parsed, failure := url.Parse(text)
	return failure == nil && (parsed.Scheme == "https" || parsed.Scheme == "http") && parsed.Host != ""
}

func (alerts Alerts) validate(problem func(format string, arguments ...any)) {
	if alerts.PublicURL != "" && !webURL(alerts.PublicURL) {
		problem("alerts.public_url must be an http or https URL")
	}
	if alerts.RunbookURL != "" && (!webURL(strings.ReplaceAll(alerts.RunbookURL, RunbookKind, "kind")) || !strings.Contains(alerts.RunbookURL, RunbookKind)) {
		problem("alerts.runbook_url must be an http or https URL holding %s", RunbookKind)
	}
	if len(alerts.Receivers) > 0 && (alerts.PublicURL == "" || alerts.RunbookURL == "") {
		problem("alerts.public_url and alerts.runbook_url are required with alerts.receivers, so every notification links its alert and its runbook")
	}
	named := map[string]bool{}
	for index, receiver := range alerts.Receivers {
		where := "alerts.receivers[" + receiver.Name + "]"
		if !receiverName.MatchString(receiver.Name) {
			where = "alerts.receivers entry #" + strconv.Itoa(index+1)
			problem("%s needs a name of lowercase letters, digits and dashes, at most 63 characters", where)
		} else if named[receiver.Name] {
			problem("alerts.receivers names %q twice", receiver.Name)
		}
		named[receiver.Name] = true
		types := 0
		for _, set := range []bool{
			receiver.PagerDuty != nil,
			receiver.Slack != nil,
			receiver.Teams != nil,
			receiver.Webhook != nil,
		} {
			if set {
				types++
			}
		}
		if types != 1 {
			problem("%s needs exactly one of %s", where, strings.Join(receiverTypes, ", "))
			continue
		}
		switch {
		case receiver.PagerDuty != nil:
			if receiver.PagerDuty.RoutingKeyFile == "" {
				problem("%s.pagerduty.routing_key_file is required", where)
			}
			if receiver.PagerDuty.URL != "" && !webURL(receiver.PagerDuty.URL) {
				problem("%s.pagerduty.url must be an http or https URL", where)
			}
		case receiver.Slack != nil:
			if receiver.Slack.WebhookURLFile == "" {
				problem("%s.slack.webhook_url_file is required", where)
			}
		case receiver.Teams != nil:
			if receiver.Teams.WebhookURLFile == "" {
				problem("%s.teams.webhook_url_file is required", where)
			}
		case receiver.Webhook != nil:
			if !webURL(receiver.Webhook.URL) {
				problem("%s.webhook.url must be an http or https URL", where)
			}
			if receiver.Webhook.SecretFile == "" {
				problem("%s.webhook.secret_file is required", where)
			}
		}
	}
	routed := map[string]bool{}
	for index, route := range alerts.Routes {
		where := "alerts.routes entry #" + strconv.Itoa(index+1)
		if !named[route.Receiver] {
			problem("%s names receiver %q, which alerts.receivers does not hold", where, route.Receiver)
		}
		routed[route.Receiver] = true
		for _, severity := range route.Severities {
			if !slices.Contains(Severities, severity) {
				problem("%s holds severity %q, which is not one of %s", where, severity, strings.Join(Severities, ", "))
			}
		}
		for _, value := range slices.Concat(route.Kinds, route.Tenants) {
			if strings.TrimSpace(value) == "" {
				problem("%s holds an empty kind or tenant", where)
			}
		}
	}
	for _, receiver := range alerts.Receivers {
		if receiverName.MatchString(receiver.Name) && !routed[receiver.Name] {
			problem("alerts.receivers[%s] is named by no route in alerts.routes", receiver.Name)
		}
	}
}
