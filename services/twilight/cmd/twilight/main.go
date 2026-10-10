package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"os/user"
	"runtime"
	"runtime/debug"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/api"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/reconcile"
	"dusk/services/twilight/internal/tokens"
	"dusk/services/twilight/migrations"
)

var (
	version  = "dev"
	revision = ""
)

const usage = `twilight is the orchestration layer of the Dusk stack: it runs campaigns on the fleet.

Usage:
  twilight serve [--config FILE] [--migrate]
  twilight migrate [--config FILE]
  twilight token create --name NAME --role viewer|operator|admin [--config FILE]
  twilight token revoke ID [--config FILE]
  twilight grafana-alerting [--config FILE] [--output FILE]
  twilight version

Configuration comes from FILE (default /etc/twilight/twilight.yaml) and from
TWILIGHT__<SECTION>__<KEY> environment variables, which override it.

grafana-alerting writes Grafana's contact points, notification policy and
message templates for the receivers and routes in alerts, as a Grafana
alerting provisioning file.
`

type usageError struct{ message string }

func (failure usageError) Error() string { return failure.message }

func main() {
	os.Exit(run(context.Background(), os.Args[1:], os.Environ(), os.Stdout, os.Stderr))
}

func run(operation context.Context, arguments, environment []string, output, diagnostics io.Writer) int {
	if len(arguments) == 0 {
		fmt.Fprint(diagnostics, usage)
		return 2
	}
	var failure error
	switch arguments[0] {
	case "serve":
		failure = serve(operation, arguments[1:], environment, output, diagnostics)
	case "migrate":
		failure = migrate(operation, arguments[1:], environment, diagnostics)
	case "token":
		failure = token(operation, arguments[1:], environment, output, diagnostics)
	case "grafana-alerting":
		failure = grafanaAlerting(arguments[1:], environment, output, diagnostics)
	case "version":
		fmt.Fprintf(output, "twilight %s (%s, %s)\n", version, buildRevision(), runtime.Version())
	case "help", "-h", "--help":
		fmt.Fprint(output, usage)
	default:
		failure = usageError{fmt.Sprintf("unknown command %q", arguments[0])}
	}
	var wrongUsage usageError
	switch {
	case failure == nil:
		return 0
	case errors.As(failure, &wrongUsage), errors.Is(failure, flag.ErrHelp):
		if !errors.Is(failure, flag.ErrHelp) {
			fmt.Fprintf(diagnostics, "twilight: %v\n\n", failure)
		}
		fmt.Fprint(diagnostics, usage)
		return 2
	default:
		fmt.Fprintf(diagnostics, "twilight: %v\n", failure)
		return 1
	}
}

func buildRevision() string {
	if revision != "" {
		return revision
	}
	if information, available := debug.ReadBuildInfo(); available {
		for _, setting := range information.Settings {
			if setting.Key == "vcs.revision" {
				return setting.Value
			}
		}
	}
	return "unknown revision"
}

type parsed struct {
	flags      *flag.FlagSet
	path       *string
	positional []string
}

func parse(name string, arguments []string, diagnostics io.Writer, define func(*flag.FlagSet)) (parsed, error) {
	flags := flag.NewFlagSet("twilight "+name, flag.ContinueOnError)
	flags.SetOutput(diagnostics)
	result := parsed{flags: flags, path: flags.String("config", config.DefaultPath, "the configuration file")}
	if define != nil {
		define(flags)
	}
	for remaining := arguments; ; remaining = flags.Args()[1:] {
		if failure := flags.Parse(remaining); failure != nil {
			if errors.Is(failure, flag.ErrHelp) {
				return result, failure
			}
			return result, usageError{failure.Error()}
		}
		if flags.NArg() == 0 {
			return result, nil
		}
		result.positional = append(result.positional, flags.Arg(0))
	}
}

func (arguments parsed) load(environment []string) (config.Config, error) {
	explicit := false
	arguments.flags.Visit(func(set *flag.Flag) { explicit = explicit || set.Name == "config" })
	return config.Load(*arguments.path, explicit, environment)
}

func newLogger(level string, output io.Writer) *slog.Logger {
	var threshold slog.Level
	_ = threshold.UnmarshalText([]byte(level))
	return slog.New(slog.NewJSONHandler(output, &slog.HandlerOptions{Level: threshold}))
}

func openDatabase(operation context.Context, settings config.Config) (*pgxpool.Pool, error) {
	opening, cancel := context.WithTimeout(operation, 30*time.Second)
	defer cancel()
	return database.Open(opening, settings.Database.URL, int32(settings.Database.MaxConnections))
}

func migrate(operation context.Context, arguments, environment []string, diagnostics io.Writer) error {
	parsed, failure := parse("migrate", arguments, diagnostics, nil)
	if failure != nil {
		return failure
	}
	if len(parsed.positional) > 0 {
		return usageError{"migrate takes no arguments"}
	}
	settings, failure := parsed.load(environment)
	if failure != nil {
		return failure
	}
	logger := newLogger(settings.LogLevel, diagnostics).With("instance", settings.Instance)
	pool, failure := openDatabase(operation, settings)
	if failure != nil {
		return failure
	}
	defer pool.Close()
	loaded, failure := database.LoadMigrations(migrations.Files)
	if failure != nil {
		return failure
	}
	applied, failure := database.Migrate(operation, pool, loaded, logger)
	if failure != nil {
		return failure
	}
	logger.Info("the database schema is current", "applied", len(applied), "known", len(loaded))
	return nil
}

func grafanaAlerting(arguments, environment []string, output, diagnostics io.Writer) error {
	var target string
	parsed, failure := parse("grafana-alerting", arguments, diagnostics, func(flags *flag.FlagSet) {
		flags.StringVar(&target, "output", "", "the file to write; standard output when not given")
	})
	if failure != nil {
		return failure
	}
	if len(parsed.positional) > 0 {
		return usageError{"grafana-alerting takes no arguments besides its flags"}
	}
	settings, failure := parsed.load(environment)
	if failure != nil {
		return failure
	}
	content, failure := alerts.GrafanaProvisioning(settings.Alerts)
	if failure != nil {
		return failure
	}
	if target == "" {
		_, failure = output.Write(content)
		return failure
	}
	staging := target + ".writing"
	if failure := os.WriteFile(staging, content, 0o644); failure != nil {
		return failure
	}
	if failure := os.Rename(staging, target); failure != nil {
		return failure
	}
	fmt.Fprintf(diagnostics, "wrote Grafana's alerting provisioning for %d receivers and %d routes to %s\n", len(settings.Alerts.Receivers), len(settings.Alerts.Routes), target)
	return nil
}

func token(operation context.Context, arguments, environment []string, output, diagnostics io.Writer) error {
	if len(arguments) == 0 {
		return usageError{"token needs create or revoke"}
	}
	var name, role string
	parsed, failure := parse("token "+arguments[0], arguments[1:], diagnostics, func(flags *flag.FlagSet) {
		if arguments[0] == "create" {
			flags.StringVar(&name, "name", "", "what the token is for")
			flags.StringVar(&role, "role", "", "viewer, operator or admin")
		}
	})
	if failure != nil {
		return failure
	}
	var identifier uuid.UUID
	switch arguments[0] {
	case "create":
		if len(parsed.positional) > 0 {
			return usageError{"token create takes no arguments besides its flags"}
		}
		if failure := tokens.Validate(name, role); failure != nil {
			return usageError{failure.Error()}
		}
	case "revoke":
		if len(parsed.positional) != 1 {
			return usageError{"token revoke takes the token's id"}
		}
		if identifier, failure = uuid.Parse(parsed.positional[0]); failure != nil {
			return usageError{fmt.Sprintf("%q is not a token id", parsed.positional[0])}
		}
	default:
		return usageError{fmt.Sprintf("unknown token command %q", arguments[0])}
	}
	settings, failure := parsed.load(environment)
	if failure != nil {
		return failure
	}
	pool, failure := openDatabase(operation, settings)
	if failure != nil {
		return failure
	}
	defer pool.Close()
	store := tokens.NewStore(pool)
	if arguments[0] == "revoke" {
		revoked, failure := store.Revoke(operation, identifier, time.Now())
		if failure != nil {
			return failure
		}
		fmt.Fprintf(diagnostics, "token %s (%s) is revoked since %s\n", revoked.ID, revoked.Name, revoked.RevokedAt.UTC().Format(time.RFC3339))
		return nil
	}
	creator := "cli"
	if current, failure := user.Current(); failure == nil && current.Username != "" {
		creator = "cli:" + current.Username
	}
	created, secret, failure := store.Create(operation, name, role, creator, time.Now())
	if failure != nil {
		return failure
	}
	fmt.Fprintf(diagnostics, "token %s created for %q with role %s; this is the only time it is shown\n", created.ID, created.Name, created.Role)
	fmt.Fprintln(output, secret)
	return nil
}

func serve(operation context.Context, arguments, environment []string, output, diagnostics io.Writer) error {
	var applyMigrations bool
	parsed, failure := parse("serve", arguments, diagnostics, func(flags *flag.FlagSet) {
		flags.BoolVar(&applyMigrations, "migrate", false, "apply pending database migrations before serving")
	})
	if failure != nil {
		return failure
	}
	if len(parsed.positional) > 0 {
		return usageError{"serve takes no arguments"}
	}
	settings, failure := parsed.load(environment)
	if failure != nil {
		return failure
	}
	development := developmentMode(environment)
	if failure := checkDevelopment(settings, development); failure != nil {
		return failure
	}
	logger := newLogger(settings.LogLevel, output).With("instance", settings.Instance)
	signals, stopSignals := signal.NotifyContext(operation, syscall.SIGTERM, os.Interrupt)
	defer stopSignals()
	raiseFileLimit(logger)

	pool, failure := openDatabase(signals, settings)
	if failure != nil {
		return failure
	}
	defer pool.Close()
	loaded, failure := database.LoadMigrations(migrations.Files)
	if failure != nil {
		return failure
	}
	if applyMigrations {
		if _, failure := database.Migrate(signals, pool, loaded, logger); failure != nil {
			return failure
		}
	}
	status, failure := database.Status(signals, pool, loaded)
	if failure != nil {
		return failure
	}
	if len(status.Pending) > 0 {
		return fmt.Errorf("%w (%d pending)", database.ErrPendingMigrations, len(status.Pending))
	}
	var ledgerKeys *reconcile.VerifyingKeys
	if settings.Reconcile.Enabled {
		if ledgerKeys, failure = reconcile.LoadVerifyingKeys(settings.Reconcile.LedgerKeys); failure != nil {
			return failure
		}
	}
	validator, failure := kafka.NewValidator()
	if failure != nil {
		return failure
	}
	options, failure := kafka.Options(settings.Kafka, settings.Instance)
	if failure != nil {
		return failure
	}
	producer, failure := kgo.NewClient(options...)
	if failure != nil {
		return failure
	}
	defer producer.Close()
	dawnClient, failure := dawn.New(settings.Dawn, nil, logger)
	if failure != nil {
		return failure
	}
	alertStore, failure := alerts.NewStore(pool, settings.Alerts, settings.Instance, logger)
	if failure != nil {
		return failure
	}
	core := engine.New(engine.Dependencies{
		Config: settings, Pool: pool, Campaigns: campaign.NewStore(pool), Inventory: inventory.NewStore(pool), Alerts: alertStore,
		Dawn: dawnClient, KafkaOptions: options, Validator: validator,
		NodeState: kafka.NewNodeStateProducer(producer, validator, settings.Kafka.Topics.NodeState),
		Intended:  kafka.NewIntendedProcessProducer(producer, validator, settings.Kafka.Topics.IntendedProcesses), Logger: logger,
	})
	apiConfiguration, failure := apiOptions(settings, development, pool, core.Service(), logger)
	if failure != nil {
		return failure
	}
	handler, failure := api.New(apiConfiguration)
	if failure != nil {
		return failure
	}
	tasks := []func(context.Context){core.Run, dawnClient.RunResolver}
	if settings.Reconcile.Enabled {
		reconciler := reconcile.New(reconcile.Dependencies{Config: settings, Pool: pool, Alerts: alertStore, Validator: validator, KafkaOptions: options, Keys: ledgerKeys, Logger: logger})
		tasks = append(tasks, reconciler.Run)
	} else {
		logger.Warn("reconcile is disabled; ledger calls are not compared with intended processes on this instance")
	}

	var draining atomic.Bool
	health := http.NewServeMux()
	health.HandleFunc("GET /healthz", func(writer http.ResponseWriter, _ *http.Request) {
		writer.WriteHeader(http.StatusOK)
		fmt.Fprintln(writer, "ok")
	})
	health.HandleFunc("GET /readyz", func(writer http.ResponseWriter, request *http.Request) {
		pinging, cancel := context.WithTimeout(request.Context(), 2*time.Second)
		defer cancel()
		switch {
		case draining.Load():
			http.Error(writer, "draining", http.StatusServiceUnavailable)
		case !core.Ready():
			http.Error(writer, "the online view is loading or Kafka topics are not as required", http.StatusServiceUnavailable)
		case pool.Ping(pinging) != nil:
			http.Error(writer, "the database is unreachable", http.StatusServiceUnavailable)
		default:
			fmt.Fprintln(writer, "ready")
		}
	})
	health.Handle("GET /metrics", promhttp.Handler())
	listener, failure := net.Listen("tcp", settings.HealthListen)
	if failure != nil {
		return fmt.Errorf("listen on health_listen %s: %w", settings.HealthListen, failure)
	}
	healthServer := &http.Server{Handler: health, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 30 * time.Second, IdleTimeout: time.Minute}
	serving := make(chan error, 2)
	go func() { serving <- fmt.Errorf("the health server stopped: %w", healthServer.Serve(listener)) }()
	publicListener, failure := apiListener(settings, logger)
	if failure != nil {
		healthServer.Close()
		return failure
	}
	publicServer := apiServer(handler, logger)
	go func() { serving <- fmt.Errorf("the API server stopped: %w", publicServer.Serve(publicListener)) }()

	work, stopWork := context.WithCancel(context.WithoutCancel(signals))
	defer stopWork()
	var running sync.WaitGroup
	for _, task := range tasks {
		running.Add(1)
		go func() {
			defer running.Done()
			task(work)
		}()
	}
	logger.Info("twilight started", "version", version, "revision", buildRevision(), "listen", publicListener.Addr().String(), "tls", settings.TLS.Certificate != "",
		"oidc", settings.OIDC.Issuer != "", "development", development, "health_listen", listener.Addr().String(), "reconcile", settings.Reconcile.Enabled)

	var result error
	select {
	case <-signals.Done():
		logger.Info("shutting down", "drain_seconds", settings.DrainSeconds)
	case failure := <-serving:
		result = failure
		logger.Error("a server stopped; shutting down", "error", failure)
	}
	draining.Store(true)
	handler.Drain()
	deadline := time.Now().Add(time.Duration(settings.DrainSeconds) * time.Second)
	closing, cancelClosing := context.WithDeadline(context.Background(), deadline)
	defer cancelClosing()
	answered := make(chan error, 1)
	go func() { answered <- publicServer.Shutdown(closing) }()
	stopWork()
	stopped := make(chan struct{})
	go func() {
		running.Wait()
		close(stopped)
	}()
	select {
	case <-stopped:
		logger.Info("the engine and reconcile stopped")
	case <-closing.Done():
		logger.Error("the engine did not stop within drain_seconds; exiting anyway", "drain_seconds", settings.DrainSeconds)
		if result == nil {
			result = errors.New("shutdown did not finish within drain_seconds")
		}
	}
	if failure := <-answered; failure != nil {
		logger.Error("API requests were still running at the end of drain_seconds; closing them", "error", failure)
		publicServer.Close()
		if result == nil {
			result = errors.New("API requests did not finish within drain_seconds")
		}
	} else {
		logger.Info("the API server stopped")
	}
	healthClosing, cancelHealth := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancelHealth()
	if failure := healthServer.Shutdown(healthClosing); failure != nil {
		logger.Warn("the health server did not close cleanly", "error", failure)
	}
	return result
}
