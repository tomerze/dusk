package config

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"os"
	"path"
	"reflect"
	"sort"
	"strconv"
	"strings"

	"gopkg.in/yaml.v3"
)

const (
	EnvironmentPrefix = "TWILIGHT"
	DefaultPath       = "/etc/twilight/twilight.yaml"
)

type Database struct {
	URL            string `yaml:"url"`
	LeaderURL      string `yaml:"leader_url"`
	MaxConnections int    `yaml:"max_connections"`
}

type KafkaTLS struct {
	CA          string `yaml:"ca"`
	Certificate string `yaml:"certificate"`
	Key         string `yaml:"key"`
	ServerName  string `yaml:"server_name"`
}

type KafkaSASL struct {
	Mechanism    string `yaml:"mechanism"`
	Username     string `yaml:"username"`
	PasswordFile string `yaml:"password_file"`
}

type Topics struct {
	Connections       string `yaml:"connections"`
	Census            string `yaml:"census"`
	Ledger            string `yaml:"ledger"`
	Enrollments       string `yaml:"enrollments"`
	NodeState         string `yaml:"node_state"`
	ProcessResults    string `yaml:"process_results"`
	IntendedProcesses string `yaml:"intended_processes"`
}

type Kafka struct {
	Brokers               []string  `yaml:"brokers"`
	AllowPlaintext        bool      `yaml:"allow_plaintext"`
	TLS                   KafkaTLS  `yaml:"tls"`
	SASL                  KafkaSASL `yaml:"sasl"`
	ClientID              string    `yaml:"client_id"`
	Topics                Topics    `yaml:"topics"`
	ResultsGroup          string    `yaml:"results_group"`
	InventoryGroup        string    `yaml:"inventory_group"`
	ReconcileGroup        string    `yaml:"reconcile_group"`
	ReconcileResultsGroup string    `yaml:"reconcile_results_group"`
}

type Dawn struct {
	Service                string   `yaml:"service"`
	Port                   int      `yaml:"port"`
	Endpoints              []string `yaml:"endpoints"`
	ServerName             string   `yaml:"server_name"`
	CA                     string   `yaml:"ca"`
	Certificate            string   `yaml:"certificate"`
	Key                    string   `yaml:"key"`
	AllowPlaintext         bool     `yaml:"allow_plaintext"`
	RequestTimeoutSeconds  int      `yaml:"request_timeout_seconds"`
	ResolveIntervalSeconds int      `yaml:"resolve_interval_seconds"`
}

type Engine struct {
	SweepIntervalSeconds         int `yaml:"sweep_interval_seconds"`
	GateIntervalSeconds          int `yaml:"gate_interval_seconds"`
	EvaluationQueue              int `yaml:"evaluation_queue"`
	DispatchWorkers              int `yaml:"dispatch_workers"`
	FactsPerSecond               int `yaml:"facts_per_second"`
	FactsWorkers                 int `yaml:"facts_workers"`
	FactsMaxAgeSeconds           int `yaml:"facts_max_age_seconds"`
	CensusIntervalSeconds        int `yaml:"census_interval_seconds"`
	SkipLagRecords               int `yaml:"skip_lag_records"`
	SkipAgeSeconds               int `yaml:"skip_age_seconds"`
	CountersFlushSeconds         int `yaml:"counters_flush_seconds"`
	ProcessLifetimeSeconds       int `yaml:"process_lifetime_seconds"`
	IntendedProcessRetentionDays int `yaml:"intended_process_retention_days"`
	PresenceFlushMillis          int `yaml:"presence_flush_millis"`
	LastSeenBucketSeconds        int `yaml:"last_seen_bucket_seconds"`
	DispatchAttempts             int `yaml:"dispatch_attempts"`
	ReapsPerSecond               int `yaml:"reaps_per_second"`
	ReapIntervalSeconds          int `yaml:"reap_interval_seconds"`
	ReapRetrySeconds             int `yaml:"reap_retry_seconds"`
}

type Reconcile struct {
	Enabled                     bool         `yaml:"enabled"`
	LedgerKeys                  string       `yaml:"ledger_keys"`
	CheckpointIntervalMS        int          `yaml:"checkpoint_interval_ms"`
	CommandsPerSession          int          `yaml:"commands_per_session"`
	IntendedProcessCacheEntries int          `yaml:"intended_process_cache_entries"`
	DefaultShell                DefaultShell `yaml:"default_shell"`
}

type DefaultShell struct {
	Ps         int `yaml:"ps"`
	StateReads int `yaml:"state_reads"`
	LogsDump   int `yaml:"logs_dump"`
	Kill       int `yaml:"kill"`
	ReapRounds int `yaml:"reap_rounds"`
	ReapedPid  int `yaml:"reaped_pid"`
}

func (shell DefaultShell) Commands(actionKind string, reapedPids int) int {
	switch actionKind {
	case "run_script", "quarantine":
		return shell.Ps + shell.LogsDump + shell.Kill
	case "ensure_config":
		return 2*shell.Ps + 2*shell.StateReads + shell.LogsDump + shell.Kill
	case "ensure_version":
		return 2*shell.Ps + 4*shell.StateReads + shell.LogsDump + shell.Kill
	case "reap":
		return shell.Kill + shell.ReapRounds + reapedPids*shell.ReapedPid
	}
	return shell.Kill + shell.ReapRounds
}

type TLS struct {
	Certificate string `yaml:"certificate"`
	Key         string `yaml:"key"`
	ClientCA    string `yaml:"client_ca"`
}

type OIDC struct {
	Issuer           string            `yaml:"issuer"`
	ClientID         string            `yaml:"client_id"`
	ClientSecretFile string            `yaml:"client_secret_file"`
	RedirectURL      string            `yaml:"redirect_url"`
	Scopes           []string          `yaml:"scopes"`
	RoleClaim        string            `yaml:"role_claim"`
	RoleMap          map[string]string `yaml:"role_map"`
}

type Sessions struct {
	LifetimeSeconds int `yaml:"lifetime_seconds"`
	IdleSeconds     int `yaml:"idle_seconds"`
}

type Config struct {
	Instance              string            `yaml:"instance"`
	Listen                string            `yaml:"listen"`
	HealthListen          string            `yaml:"health_listen"`
	DrainSeconds          int               `yaml:"drain_seconds"`
	LogLevel              string            `yaml:"log_level"`
	MaxStreams            int               `yaml:"max_streams"`
	MaxRequests           int               `yaml:"max_requests"`
	RequestTimeoutSeconds int               `yaml:"request_timeout_seconds"`
	TLS                   TLS               `yaml:"tls"`
	Principals            map[string]string `yaml:"principals"`
	OIDC                  OIDC              `yaml:"oidc"`
	Sessions              Sessions          `yaml:"sessions"`
	Database              Database          `yaml:"database"`
	Kafka                 Kafka             `yaml:"kafka"`
	Dawn                  Dawn              `yaml:"dawn"`
	Engine                Engine            `yaml:"engine"`
	Alerts                Alerts            `yaml:"alerts"`
	Reconcile             Reconcile         `yaml:"reconcile"`
}

var Roles = []string{"viewer", "operator", "admin"}

func validRole(role string) bool {
	for _, known := range Roles {
		if role == known {
			return true
		}
	}
	return false
}

func Default() Config {
	hostname, _ := os.Hostname()
	return Config{
		Instance:              hostname,
		Listen:                "0.0.0.0:8080",
		HealthListen:          "0.0.0.0:9102",
		DrainSeconds:          30,
		LogLevel:              "info",
		MaxStreams:            1000,
		MaxRequests:           8,
		RequestTimeoutSeconds: 15,
		OIDC:                  OIDC{Scopes: []string{"openid", "profile", "email"}, RoleClaim: "groups"},
		Sessions:              Sessions{LifetimeSeconds: 43200, IdleSeconds: 3600},
		Database:              Database{URL: "postgres://twilight@postgres:5432/inventory?sslmode=verify-full", MaxConnections: 20},
		Kafka: Kafka{
			Brokers:  []string{"kafka:9092"},
			ClientID: "twilight",
			Topics: Topics{
				Connections:       "dusk.connections",
				Census:            "dusk.census",
				Ledger:            "dusk.ledger",
				Enrollments:       "dusk.enrollments",
				NodeState:         "dusk.node-state",
				ProcessResults:    "dusk.process-results",
				IntendedProcesses: "dusk.intended-processes",
			},
			ResultsGroup:          "twilight-results",
			InventoryGroup:        "twilight-inventory",
			ReconcileGroup:        "twilight-reconcile",
			ReconcileResultsGroup: "twilight-reconcile-results",
		},
		Dawn: Dawn{
			Service:                "dawn",
			Port:                   8443,
			ServerName:             "dawn",
			CA:                     "/etc/twilight/pki/internal-ca.crt",
			Certificate:            "/etc/twilight/tls/client.crt",
			Key:                    "/etc/twilight/tls/client.key",
			RequestTimeoutSeconds:  30,
			ResolveIntervalSeconds: 30,
		},
		Engine: Engine{
			SweepIntervalSeconds:         300,
			GateIntervalSeconds:          15,
			EvaluationQueue:              100000,
			DispatchWorkers:              64,
			FactsPerSecond:               50,
			FactsWorkers:                 16,
			FactsMaxAgeSeconds:           86400,
			CensusIntervalSeconds:        300,
			SkipLagRecords:               200000,
			SkipAgeSeconds:               60,
			CountersFlushSeconds:         5,
			PresenceFlushMillis:          1000,
			LastSeenBucketSeconds:        10,
			DispatchAttempts:             3,
			ReapsPerSecond:               20,
			ReapIntervalSeconds:          300,
			ReapRetrySeconds:             600,
			ProcessLifetimeSeconds:       900,
			IntendedProcessRetentionDays: 31,
		},
		Alerts: Alerts{EnrollmentRatePerMinute: 600, DeliveryHorizonSeconds: 86400},
		Reconcile: Reconcile{Enabled: true, LedgerKeys: "/etc/twilight/pki/ledger-keys.json", CheckpointIntervalMS: 1000,
			CommandsPerSession: 10000, IntendedProcessCacheEntries: 1000000,
			DefaultShell: DefaultShell{Ps: 1, StateReads: 1, LogsDump: 1, Kill: 1, ReapRounds: 3}},
	}
}

func Load(path string, explicit bool, environment []string) (Config, error) {
	loaded := Default()
	content, failure := os.ReadFile(path)
	switch {
	case failure == nil:
		decoder := yaml.NewDecoder(bytes.NewReader(content))
		decoder.KnownFields(true)
		if failure := decoder.Decode(&loaded); failure != nil && !errors.Is(failure, io.EOF) {
			return Config{}, fmt.Errorf("parse %s: %w", path, failure)
		}
	case explicit || !errors.Is(failure, os.ErrNotExist):
		return Config{}, fmt.Errorf("read %s: %w", path, failure)
	}
	if failure := applyEnvironment(&loaded, environment); failure != nil {
		return Config{}, failure
	}
	if loaded.Database.LeaderURL == "" {
		loaded.Database.LeaderURL = loaded.Database.URL
	}
	if failure := loaded.Validate(); failure != nil {
		return Config{}, failure
	}
	return loaded, nil
}

func applyEnvironment(target *Config, environment []string) error {
	values := map[string]string{}
	prefix := EnvironmentPrefix + "__"
	for _, entry := range environment {
		name, value, found := strings.Cut(entry, "=")
		if found && strings.HasPrefix(name, prefix) {
			values[name] = value
		}
	}
	if len(values) == 0 {
		return nil
	}
	used := map[string]bool{}
	if failure := walk(reflect.ValueOf(target).Elem(), EnvironmentPrefix, values, used); failure != nil {
		return failure
	}
	var unknown []string
	for name := range values {
		if !used[name] {
			unknown = append(unknown, name)
		}
	}
	if len(unknown) > 0 {
		sort.Strings(unknown)
		return fmt.Errorf("unknown configuration variables: %s", strings.Join(unknown, ", "))
	}
	return nil
}

func walk(value reflect.Value, prefix string, values map[string]string, used map[string]bool) error {
	for index := range value.NumField() {
		field := value.Type().Field(index)
		tag := strings.Split(field.Tag.Get("yaml"), ",")[0]
		if tag == "" || tag == "-" {
			continue
		}
		name := prefix + "__" + strings.ToUpper(tag)
		target := value.Field(index)
		if target.Kind() == reflect.Struct {
			if failure := walk(target, name, values, used); failure != nil {
				return failure
			}
			continue
		}
		text, present := values[name]
		if !present {
			continue
		}
		used[name] = true
		if failure := assign(target, text); failure != nil {
			return fmt.Errorf("%s: %w", name, failure)
		}
	}
	return nil
}

func assign(target reflect.Value, text string) error {
	switch target.Kind() {
	case reflect.String:
		target.SetString(text)
	case reflect.Bool:
		parsed, failure := strconv.ParseBool(text)
		if failure != nil {
			return fmt.Errorf("%q is not true or false", text)
		}
		target.SetBool(parsed)
	case reflect.Int, reflect.Int64:
		parsed, failure := strconv.ParseInt(text, 10, 64)
		if failure != nil {
			return fmt.Errorf("%q is not an integer", text)
		}
		target.SetInt(parsed)
	case reflect.Slice:
		if target.Type().Elem().Kind() != reflect.String {
			return errors.New("this list is set in the configuration file only")
		}
		var items []string
		for item := range strings.SplitSeq(text, ",") {
			if trimmed := strings.TrimSpace(item); trimmed != "" {
				items = append(items, trimmed)
			}
		}
		target.Set(reflect.ValueOf(items))
	case reflect.Map:
		entries := map[string]string{}
		for item := range strings.SplitSeq(text, ",") {
			if strings.TrimSpace(item) == "" {
				continue
			}
			key, value, found := strings.Cut(item, "=")
			key, value = strings.TrimSpace(key), strings.TrimSpace(value)
			if !found || key == "" {
				return fmt.Errorf("%q is not a comma-separated list of key=value", text)
			}
			entries[key] = value
		}
		target.Set(reflect.ValueOf(entries))
	default:
		return fmt.Errorf("cannot set a %s from the environment", target.Kind())
	}
	return nil
}

func (loaded Config) Validate() error {
	var problems []string
	problem := func(format string, arguments ...any) {
		problems = append(problems, fmt.Sprintf(format, arguments...))
	}
	if strings.TrimSpace(loaded.Instance) == "" || strings.Contains(loaded.Instance, "/") {
		problem("instance must be set and hold no slash")
	}
	if loaded.Database.URL == "" {
		problem("database.url is required")
	}
	if loaded.Database.MaxConnections < 4 {
		problem("database.max_connections must be at least 4")
	}
	if len(loaded.Kafka.Brokers) == 0 {
		problem("kafka.brokers is required")
	}
	if !loaded.Kafka.AllowPlaintext && loaded.Kafka.TLS.CA == "" {
		problem("kafka.tls.ca is required unless kafka.allow_plaintext is true")
	}
	switch loaded.Kafka.SASL.Mechanism {
	case "", "SCRAM-SHA-256", "SCRAM-SHA-512":
	default:
		problem("kafka.sasl.mechanism must be SCRAM-SHA-256 or SCRAM-SHA-512")
	}
	if loaded.Kafka.SASL.Mechanism != "" && (loaded.Kafka.SASL.Username == "" || loaded.Kafka.SASL.PasswordFile == "") {
		problem("kafka.sasl needs username and password_file")
	}
	topics := reflect.ValueOf(loaded.Kafka.Topics)
	for index := range topics.NumField() {
		if topics.Field(index).String() == "" {
			problem("kafka.topics.%s is required", topics.Type().Field(index).Tag.Get("yaml"))
		}
	}
	if loaded.Dawn.Service == "" && len(loaded.Dawn.Endpoints) == 0 {
		problem("dawn.service or dawn.endpoints is required")
	}
	if !loaded.Dawn.AllowPlaintext && (loaded.Dawn.CA == "" || loaded.Dawn.Certificate == "" || loaded.Dawn.Key == "") {
		problem("dawn.ca, dawn.certificate and dawn.key are required unless dawn.allow_plaintext is true")
	}
	if (loaded.TLS.Certificate == "") != (loaded.TLS.Key == "") {
		problem("tls.certificate and tls.key are set together")
	}
	if loaded.TLS.ClientCA != "" && loaded.TLS.Certificate == "" {
		problem("tls.client_ca needs tls.certificate and tls.key")
	}
	if len(loaded.Principals) > 0 && loaded.TLS.ClientCA == "" {
		problem("principals needs tls.client_ca")
	}
	for _, pattern := range sortedKeys(loaded.Principals) {
		if _, failure := path.Match(pattern, ""); failure != nil || pattern == "" {
			problem("principals holds an invalid pattern %q", pattern)
		}
		if !validRole(loaded.Principals[pattern]) {
			problem("principals maps %q to %q, which is not one of %s", pattern, loaded.Principals[pattern], strings.Join(Roles, ", "))
		}
	}
	if loaded.OIDC.Issuer != "" {
		if !secureURL(loaded.OIDC.Issuer) {
			problem("oidc.issuer must be an https URL, or http on a loopback address")
		}
		if loaded.OIDC.ClientID == "" {
			problem("oidc.client_id is required with oidc.issuer")
		}
		if !secureURL(loaded.OIDC.RedirectURL) {
			problem("oidc.redirect_url must be an https URL, or http on a loopback address, ending in /api/v1/auth/callback")
		} else if parsed, _ := url.Parse(loaded.OIDC.RedirectURL); parsed.Path != "/api/v1/auth/callback" {
			problem("oidc.redirect_url must end in /api/v1/auth/callback")
		}
		if strings.TrimSpace(loaded.OIDC.RoleClaim) == "" {
			problem("oidc.role_claim is required with oidc.issuer")
		}
		if len(loaded.OIDC.RoleMap) == 0 {
			problem("oidc.role_map must map at least one %s value to a role", loaded.OIDC.RoleClaim)
		}
		for _, value := range sortedKeys(loaded.OIDC.RoleMap) {
			if !validRole(loaded.OIDC.RoleMap[value]) {
				problem("oidc.role_map maps %q to %q, which is not one of %s", value, loaded.OIDC.RoleMap[value], strings.Join(Roles, ", "))
			}
		}
	}
	if loaded.Sessions.LifetimeSeconds < 60 || loaded.Sessions.IdleSeconds < 60 || loaded.Sessions.IdleSeconds > loaded.Sessions.LifetimeSeconds {
		problem("sessions.lifetime_seconds and sessions.idle_seconds must be at least 60, and idle_seconds at most lifetime_seconds")
	}
	loaded.Alerts.validate(problem)
	positive := map[string]int{
		"drain_seconds":                            loaded.DrainSeconds,
		"max_streams":                              loaded.MaxStreams,
		"max_requests":                             loaded.MaxRequests,
		"request_timeout_seconds":                  loaded.RequestTimeoutSeconds,
		"dawn.port":                                loaded.Dawn.Port,
		"dawn.request_timeout_seconds":             loaded.Dawn.RequestTimeoutSeconds,
		"dawn.resolve_interval_seconds":            loaded.Dawn.ResolveIntervalSeconds,
		"engine.sweep_interval_seconds":            loaded.Engine.SweepIntervalSeconds,
		"engine.gate_interval_seconds":             loaded.Engine.GateIntervalSeconds,
		"engine.evaluation_queue":                  loaded.Engine.EvaluationQueue,
		"engine.dispatch_workers":                  loaded.Engine.DispatchWorkers,
		"engine.facts_per_second":                  loaded.Engine.FactsPerSecond,
		"engine.facts_workers":                     loaded.Engine.FactsWorkers,
		"engine.facts_max_age_seconds":             loaded.Engine.FactsMaxAgeSeconds,
		"engine.census_interval_seconds":           loaded.Engine.CensusIntervalSeconds,
		"engine.skip_lag_records":                  loaded.Engine.SkipLagRecords,
		"engine.skip_age_seconds":                  loaded.Engine.SkipAgeSeconds,
		"engine.counters_flush_seconds":            loaded.Engine.CountersFlushSeconds,
		"engine.process_lifetime_seconds":          loaded.Engine.ProcessLifetimeSeconds,
		"engine.intended_process_retention_days":   loaded.Engine.IntendedProcessRetentionDays,
		"engine.reaps_per_second":                  loaded.Engine.ReapsPerSecond,
		"engine.reap_interval_seconds":             loaded.Engine.ReapIntervalSeconds,
		"engine.reap_retry_seconds":                loaded.Engine.ReapRetrySeconds,
		"engine.presence_flush_millis":             loaded.Engine.PresenceFlushMillis,
		"engine.last_seen_bucket_seconds":          loaded.Engine.LastSeenBucketSeconds,
		"engine.dispatch_attempts":                 loaded.Engine.DispatchAttempts,
		"alerts.enrollment_rate_per_minute":        loaded.Alerts.EnrollmentRatePerMinute,
		"alerts.delivery_horizon_seconds":          loaded.Alerts.DeliveryHorizonSeconds,
		"reconcile.checkpoint_interval_ms":         loaded.Reconcile.CheckpointIntervalMS,
		"reconcile.commands_per_session":           loaded.Reconcile.CommandsPerSession,
		"reconcile.intended_process_cache_entries": loaded.Reconcile.IntendedProcessCacheEntries,
	}
	names := make([]string, 0, len(positive))
	for name := range positive {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		if positive[name] < 1 {
			problem("%s must be at least 1", name)
		}
	}
	shell := loaded.Reconcile.DefaultShell
	if min(shell.Ps, shell.StateReads, shell.LogsDump, shell.Kill, shell.ReapRounds, shell.ReapedPid) < 0 {
		problem("reconcile.default_shell counts must not be negative")
	}
	if loaded.Reconcile.Enabled && loaded.Reconcile.LedgerKeys == "" {
		problem("reconcile.ledger_keys is required while reconcile.enabled is true")
	}
	for _, group := range []struct{ name, value string }{
		{"kafka.results_group", loaded.Kafka.ResultsGroup},
		{"kafka.inventory_group", loaded.Kafka.InventoryGroup},
		{"kafka.reconcile_group", loaded.Kafka.ReconcileGroup},
		{"kafka.reconcile_results_group", loaded.Kafka.ReconcileResultsGroup},
	} {
		if group.value == "" {
			problem("%s is required", group.name)
		}
	}
	switch loaded.LogLevel {
	case "debug", "info", "warn", "error":
	default:
		problem("log_level must be debug, info, warn or error")
	}
	if len(problems) > 0 {
		return errors.New("invalid configuration: " + strings.Join(problems, "; "))
	}
	return nil
}

func sortedKeys(entries map[string]string) []string {
	keys := make([]string, 0, len(entries))
	for key := range entries {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

func secureURL(text string) bool {
	parsed, failure := url.Parse(text)
	if failure != nil || parsed.Host == "" {
		return false
	}
	if parsed.Scheme == "https" {
		return true
	}
	if parsed.Scheme != "http" {
		return false
	}
	host := parsed.Hostname()
	if host == "localhost" {
		return true
	}
	address := net.ParseIP(host)
	return address != nil && address.IsLoopback()
}
