package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeFile(test *testing.T, content string) string {
	test.Helper()
	path := filepath.Join(test.TempDir(), "twilight.yaml")
	if failure := os.WriteFile(path, []byte(content), 0o600); failure != nil {
		test.Fatal(failure)
	}
	return path
}

const minimal = `
instance: twilight-0
kafka:
  allow_plaintext: true
`

func TestDefaultsWithAMinimalFile(test *testing.T) {
	loaded, failure := Load(writeFile(test, minimal), true, nil)
	if failure != nil {
		test.Fatal(failure)
	}
	if loaded.Instance != "twilight-0" || loaded.HealthListen != "0.0.0.0:9102" || loaded.Kafka.Topics.Census != "dusk.census" {
		test.Fatalf("defaults not applied: %+v", loaded)
	}
	if loaded.Database.LeaderURL != loaded.Database.URL {
		test.Fatal("leader_url does not default to url")
	}
	if loaded.Engine.SweepIntervalSeconds != 300 || loaded.Engine.EvaluationQueue != 100000 || loaded.Engine.GateIntervalSeconds != 15 {
		test.Fatalf("engine defaults %+v", loaded.Engine)
	}
}

func TestEnvironmentOverridesTheFile(test *testing.T) {
	path := writeFile(test, minimal+`
database:
  url: postgres://file/inventory
dawn:
  endpoints: [a:1]
`)
	environment := []string{
		"TWILIGHT__DATABASE__URL=postgres://env/inventory",
		"TWILIGHT__DATABASE__LEADER_URL=postgres://primary/inventory",
		"TWILIGHT__KAFKA__BROKERS=kafka-0:9092, kafka-1:9092",
		"TWILIGHT__KAFKA__TOPICS__LEDGER=ledger-test",
		"TWILIGHT__ENGINE__SWEEP_INTERVAL_SECONDS=60",
		"TWILIGHT__RECONCILE__ENABLED=false",
		"TWILIGHT__DAWN__ENDPOINTS=",
		"PATH=/usr/bin",
	}
	loaded, failure := Load(path, true, environment)
	if failure != nil {
		test.Fatal(failure)
	}
	if loaded.Database.URL != "postgres://env/inventory" || loaded.Database.LeaderURL != "postgres://primary/inventory" {
		test.Fatalf("database %+v", loaded.Database)
	}
	if strings.Join(loaded.Kafka.Brokers, "|") != "kafka-0:9092|kafka-1:9092" || loaded.Kafka.Topics.Ledger != "ledger-test" {
		test.Fatalf("kafka %+v", loaded.Kafka)
	}
	if loaded.Engine.SweepIntervalSeconds != 60 || loaded.Reconcile.Enabled || len(loaded.Dawn.Endpoints) != 0 {
		test.Fatalf("overrides %+v %+v %v", loaded.Engine, loaded.Reconcile, loaded.Dawn.Endpoints)
	}
}

func TestEnvironmentMistakesAreRefused(test *testing.T) {
	path := writeFile(test, minimal)
	for _, entry := range []string{
		"TWILIGHT__ENGINE__SWEEP_INTERVAL_SECONDS=soon",
		"TWILIGHT__RECONCILE__ENABLED=maybe",
		"TWILIGHT__ENGINE__NO_SUCH_KEY=1",
		"TWILIGHT__ENGINE=1",
		"TWILIGHT__ALERTS__RECEIVERS=pager",
		"TWILIGHT__ALERTS__ROUTES=pager",
	} {
		if _, failure := Load(path, true, []string{entry}); failure == nil {
			test.Errorf("%s accepted", entry)
		}
	}
}

func TestFileMistakesAreRefused(test *testing.T) {
	if _, failure := Load(writeFile(test, minimal+"unknown_key: 1\n"), true, nil); failure == nil {
		test.Fatal("unknown key accepted")
	}
	if _, failure := Load(filepath.Join(test.TempDir(), "absent.yaml"), true, nil); failure == nil {
		test.Fatal("an explicit absent file was accepted")
	}
	if _, failure := Load(filepath.Join(test.TempDir(), "absent.yaml"), false, []string{"TWILIGHT__INSTANCE=twilight-1", "TWILIGHT__KAFKA__ALLOW_PLAINTEXT=true"}); failure != nil {
		test.Fatalf("the default path may be absent: %v", failure)
	}
	if _, failure := Load(writeFile(test, ""), true, []string{"TWILIGHT__INSTANCE=twilight-1", "TWILIGHT__KAFKA__ALLOW_PLAINTEXT=true"}); failure != nil {
		test.Fatalf("an empty file: %v", failure)
	}
}

func TestValidation(test *testing.T) {
	cases := map[string]string{
		"kafka.tls.ca":                   "instance: twilight-0\n",
		"instance":                       "instance: a/b\nkafka: {allow_plaintext: true}\n",
		"kafka.sasl.mechanism":           minimal + "  sasl: {mechanism: PLAIN}\n",
		"kafka.sasl needs":               minimal + "  sasl: {mechanism: SCRAM-SHA-512}\n",
		"kafka.topics.ledger":            minimal + "  topics: {ledger: \"\"}\n",
		"dawn.ca":                        minimal + "dawn: {ca: \"\"}\n",
		"engine.dispatch_workers":        minimal + "engine: {dispatch_workers: 0}\n",
		"alerts.public_url must":         minimal + "alerts: {public_url: \"ftp://x\"}\n",
		"alerts.runbook_url must":        minimal + "alerts: {runbook_url: \"https://docs.example.org/runbooks/\"}\n",
		"alerts.delivery_horizon":        minimal + "alerts: {delivery_horizon_seconds: 0}\n",
		"log_level":                      minimal + "log_level: loud\n",
		"reconcile.ledger_keys":          minimal + "reconcile: {ledger_keys: \"\"}\n",
		"reconcile.commands_per_session": minimal + "reconcile: {commands_per_session: 0}\n",
		"reconcile.default_shell":        minimal + "reconcile: {default_shell: {kill: -1}}\n",
		"engine.reaps_per_second":        minimal + "engine: {reaps_per_second: 0}\n",
		"max_requests":                   minimal + "max_requests: 0\n",
		"kafka.reconcile_results":        minimal + "  reconcile_results_group: \"\"\n",
		"tls.certificate and":            minimal + "tls: {certificate: /a.crt}\n",
		"tls.client_ca needs":            minimal + "tls: {client_ca: /ca.crt}\n",
		"principals needs":               minimal + "principals: {\"dawn-*\": viewer}\n",
		"principals maps":                minimal + "tls: {certificate: /a.crt, key: /a.key, client_ca: /ca.crt}\nprincipals: {\"dawn-*\": root}\n",
		"principals holds":               minimal + "tls: {certificate: /a.crt, key: /a.key, client_ca: /ca.crt}\nprincipals: {\"[dawn\": viewer}\n",
		"oidc.issuer must":               minimal + oidc("http://issuer.example", "https://twilight.example/api/v1/auth/callback"),
		"oidc.redirect_url must":         minimal + oidc("https://issuer.example", "https://twilight.example/callback"),
		"oidc.client_id":                 minimal + "oidc: {issuer: \"https://issuer.example\", redirect_url: \"https://twilight.example/api/v1/auth/callback\", role_map: {ops: admin}}\n",
		"oidc.role_map must":             minimal + "oidc: {issuer: \"https://issuer.example\", client_id: twilight, redirect_url: \"https://twilight.example/api/v1/auth/callback\"}\n",
		"oidc.role_map maps":             minimal + "oidc: {issuer: \"https://issuer.example\", client_id: twilight, redirect_url: \"https://twilight.example/api/v1/auth/callback\", role_map: {ops: root}}\n",
		"sessions.lifetime":              minimal + "sessions: {lifetime_seconds: 600, idle_seconds: 3600}\n",
		"max_streams":                    minimal + "max_streams: 0\n",
	}
	for want, content := range cases {
		_, failure := Load(writeFile(test, content), true, nil)
		if failure == nil || !strings.Contains(failure.Error(), want) {
			test.Errorf("%q: got %v", want, failure)
		}
	}
}

func TestDefaultShellCommandsFollowDawnsReads(test *testing.T) {
	shell := Default().Reconcile.DefaultShell
	for kind, want := range map[string]int{"run_script": 3, "quarantine": 3, "ensure_config": 6, "ensure_version": 8, "collect_facts": 4, "interactive": 4} {
		if got := shell.Commands(kind, 0); got != want {
			test.Errorf("%s allows %d default-shell commands, want %d", kind, got, want)
		}
	}
	if got := shell.Commands("reap", 256); got != 4 {
		test.Errorf("a reap of 256 pids allows %d default-shell commands, want 4", got)
	}
	shell.ReapedPid = 1
	if got := shell.Commands("reap", 3); got != 7 {
		test.Errorf("a reap of 3 pids at one command each allows %d, want 7", got)
	}
}

func oidc(issuer, redirect string) string {
	return "oidc: {issuer: \"" + issuer + "\", client_id: twilight, redirect_url: \"" + redirect + "\", role_map: {ops: operator}}\n"
}

func TestAPISettings(test *testing.T) {
	path := writeFile(test, minimal+`
tls: {certificate: /etc/twilight/tls/server.crt, key: /etc/twilight/tls/server.key, client_ca: /etc/twilight/pki/internal-ca.crt}
principals: {"dawn-*": viewer}
oidc:
  issuer: https://login.example.org/realms/fleet
  client_id: twilight
  redirect_url: https://twilight.example.org/api/v1/auth/callback
  role_map: {fleet-viewers: viewer}
`)
	environment := []string{
		"TWILIGHT__OIDC__ROLE_MAP=fleet-admins=admin, fleet-operators = operator",
		"TWILIGHT__PRINCIPALS=automation-*=operator",
		"TWILIGHT__OIDC__ROLE_CLAIM=realm_access.roles",
	}
	loaded, failure := Load(path, true, environment)
	if failure != nil {
		test.Fatal(failure)
	}
	if len(loaded.OIDC.RoleMap) != 2 || loaded.OIDC.RoleMap["fleet-admins"] != "admin" || loaded.OIDC.RoleMap["fleet-operators"] != "operator" {
		test.Fatalf("role map %v", loaded.OIDC.RoleMap)
	}
	if len(loaded.Principals) != 1 || loaded.Principals["automation-*"] != "operator" {
		test.Fatalf("principals %v", loaded.Principals)
	}
	if loaded.OIDC.RoleClaim != "realm_access.roles" || strings.Join(loaded.OIDC.Scopes, " ") != "openid profile email" {
		test.Fatalf("oidc %+v", loaded.OIDC)
	}
	if loaded.Sessions.LifetimeSeconds != 43200 || loaded.Sessions.IdleSeconds != 3600 || loaded.MaxStreams != 1000 {
		test.Fatalf("defaults %+v %d", loaded.Sessions, loaded.MaxStreams)
	}
	if _, failure := Load(path, true, []string{"TWILIGHT__OIDC__ROLE_MAP=admins"}); failure == nil || !strings.Contains(failure.Error(), "key=value") {
		test.Fatalf("a map entry without = was accepted: %v", failure)
	}
	loopback := writeFile(test, minimal+oidc("http://127.0.0.1:5556/dex", "http://localhost:8080/api/v1/auth/callback"))
	if _, failure := Load(loopback, true, nil); failure != nil {
		test.Fatalf("http on a loopback address is refused: %v", failure)
	}
}

const alerting = `
alerts:
  public_url: https://twilight.example.org
  runbook_url: https://docs.example.org/stack/runbooks/{kind}/
  receivers:
    - name: on-call
      pagerduty: {routing_key_file: /run/secrets/pagerduty/routing-key}
    - name: team
      slack: {webhook_url_file: /run/secrets/slack/url}
    - name: soc
      email: {host: smtp.example.org, username: dusk, password_file: /run/secrets/smtp/password, from: dusk@example.org, to: [soc@example.org]}
    - name: noc
      teams: {webhook_url_file: /run/secrets/teams/url}
    - name: siem
      webhook: {url: "https://siem.example.org/dusk", secret_file: /run/secrets/siem/secret}
  routes:
    - {receiver: on-call, severities: [critical]}
    - {receiver: team, severities: [medium, low]}
    - {receiver: soc, tenants: [cn]}
    - {receiver: noc, kinds: [process_without_intent]}
    - {receiver: siem}
`

func TestAlertReceiversAndRoutes(test *testing.T) {
	loaded, failure := Load(writeFile(test, minimal+alerting), true, nil)
	if failure != nil {
		test.Fatal(failure)
	}
	receivers := loaded.Alerts.Receivers
	if len(receivers) != 5 || receivers[0].PagerDuty == nil || receivers[1].Slack == nil || receivers[2].Email == nil || receivers[3].Teams == nil || receivers[4].Webhook == nil {
		test.Fatalf("receivers %+v", receivers)
	}
	if receivers[0].PagerDuty.EventsURL() != PagerDutyEventsURL || receivers[2].Email.SecurityMode() != EmailStartTLS || receivers[2].Email.PortNumber() != 587 {
		test.Fatalf("defaults %+v %+v", receivers[0].PagerDuty, receivers[2].Email)
	}
	if (EmailReceiver{Security: EmailTLS}).PortNumber() != 465 || (EmailReceiver{Security: EmailPlaintext}).PortNumber() != 25 || (EmailReceiver{Port: 2525}).PortNumber() != 2525 {
		test.Fatal("email port defaults")
	}
	if loaded.Alerts.DeliveryHorizonSeconds != 86400 {
		test.Fatalf("delivery horizon %d", loaded.Alerts.DeliveryHorizonSeconds)
	}
}

func TestAlertRouteMatching(test *testing.T) {
	tenant := "cn"
	other := "eu"
	cases := []struct {
		route    Route
		severity string
		kind     string
		tenant   *string
		want     bool
	}{
		{Route{}, "low", "anything", nil, true},
		{Route{Severities: []string{"critical", "high"}}, "high", "pid_reused", nil, true},
		{Route{Severities: []string{"critical"}}, "high", "pid_reused", nil, false},
		{Route{Kinds: []string{"pid_reused"}}, "low", "pid_reused", &tenant, true},
		{Route{Kinds: []string{"pid_reused"}}, "low", "process_shape", &tenant, false},
		{Route{Tenants: []string{"cn"}}, "critical", "pid_reused", &tenant, true},
		{Route{Tenants: []string{"cn"}}, "critical", "pid_reused", &other, false},
		{Route{Tenants: []string{"cn"}}, "critical", "pid_reused", nil, false},
		{Route{Severities: []string{"critical"}, Kinds: []string{"pid_reused"}, Tenants: []string{"cn"}}, "critical", "pid_reused", &tenant, true},
	}
	for index, entry := range cases {
		if got := entry.route.Matches(entry.severity, entry.kind, entry.tenant); got != entry.want {
			test.Errorf("case %d: %+v matched %v", index, entry.route, got)
		}
	}
}

func TestAlertConfigurationMistakes(test *testing.T) {
	base := minimal + "alerts:\n  public_url: https://twilight.example.org\n  runbook_url: https://docs.example.org/{kind}/\n"
	cases := map[string]string{
		"alerts.public_url and alerts.runbook_url are required": minimal + "alerts:\n  receivers: [{name: a, webhook: {url: \"https://x.example.org\", secret_file: /s}}]\n  routes: [{receiver: a}]\n",
		"needs a name":                         base + "  receivers: [{name: A, webhook: {url: \"https://x.example.org\", secret_file: /s}}]\n",
		"names \"a\" twice":                    base + "  receivers: [{name: a, slack: {webhook_url_file: /s}}, {name: a, slack: {webhook_url_file: /s}}]\n  routes: [{receiver: a}]\n",
		"needs exactly one of":                 base + "  receivers: [{name: a, slack: {webhook_url_file: /s}, teams: {webhook_url_file: /s}}]\n  routes: [{receiver: a}]\n",
		"routing_key_file":                     base + "  receivers: [{name: a, pagerduty: {}}]\n  routes: [{receiver: a}]\n",
		"pagerduty.url":                        base + "  receivers: [{name: a, pagerduty: {routing_key_file: /k, url: \"ftp://x\"}}]\n  routes: [{receiver: a}]\n",
		"slack.webhook_url_file":               base + "  receivers: [{name: a, slack: {}}]\n  routes: [{receiver: a}]\n",
		"teams.webhook_url_file":               base + "  receivers: [{name: a, teams: {}}]\n  routes: [{receiver: a}]\n",
		"webhook.url":                          base + "  receivers: [{name: a, webhook: {secret_file: /s}}]\n  routes: [{receiver: a}]\n",
		"webhook.secret_file":                  base + "  receivers: [{name: a, webhook: {url: \"https://x.example.org\"}}]\n  routes: [{receiver: a}]\n",
		"email.host":                           base + "  receivers: [{name: a, email: {from: a@example.org, to: [b@example.org]}}]\n  routes: [{receiver: a}]\n",
		"email.security must":                  base + "  receivers: [{name: a, email: {host: h, security: ssl, from: a@example.org, to: [b@example.org]}}]\n  routes: [{receiver: a}]\n",
		"security none sends no":               base + "  receivers: [{name: a, email: {host: h, security: none, username: u, password_file: /p, from: a@example.org, to: [b@example.org]}}]\n  routes: [{receiver: a}]\n",
		"username and password_file":           base + "  receivers: [{name: a, email: {host: h, username: u, from: a@example.org, to: [b@example.org]}}]\n  routes: [{receiver: a}]\n",
		"email.from":                           base + "  receivers: [{name: a, email: {host: h, from: \"Dusk <a@example.org>\", to: [b@example.org]}}]\n  routes: [{receiver: a}]\n",
		"email.to needs":                       base + "  receivers: [{name: a, email: {host: h, from: a@example.org}}]\n  routes: [{receiver: a}]\n",
		"not a bare email":                     base + "  receivers: [{name: a, email: {host: h, from: a@example.org, to: [nobody]}}]\n  routes: [{receiver: a}]\n",
		"which alerts.receivers does not hold": base + "  routes: [{receiver: ghost}]\n",
		"holds severity":                       base + "  receivers: [{name: a, slack: {webhook_url_file: /s}}]\n  routes: [{receiver: a, severities: [urgent]}]\n",
		"empty kind or tenant":                 base + "  receivers: [{name: a, slack: {webhook_url_file: /s}}]\n  routes: [{receiver: a, kinds: [\"\"]}]\n",
		"is named by no route":                 base + "  receivers: [{name: a, slack: {webhook_url_file: /s}}]\n",
	}
	for want, content := range cases {
		_, failure := Load(writeFile(test, content), true, nil)
		if failure == nil || !strings.Contains(failure.Error(), want) {
			test.Errorf("%q: got %v", want, failure)
		}
	}
}
