//go:build integration

package testsupport

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"net/netip"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/moby/moby/api/types/container"
	"github.com/moby/moby/api/types/network"
	"github.com/testcontainers/testcontainers-go"
	"github.com/testcontainers/testcontainers-go/modules/postgres"
	"github.com/testcontainers/testcontainers-go/modules/redpanda"
	"github.com/testcontainers/testcontainers-go/wait"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
	"dusk/services/twilight/migrations"
)

const (
	PostgresImage = "postgres:17-alpine"
	RedpandaImage = "docker.redpanda.com/redpandadata/redpanda:v25.2.4"
	MailpitImage  = "axllent/mailpit:v1.31.2"
)

var (
	mutex       sync.Mutex
	containers  []testcontainers.Container
	postgresURL string
	brokers     string
)

func containerName(kind string) string {
	prefix := os.Getenv("TWILIGHT_TEST_CONTAINER_PREFIX")
	if prefix == "" {
		prefix = "stack-twilight-"
	}
	return prefix + kind + "-" + randomSuffix()
}

func randomSuffix() string {
	bytes := make([]byte, 4)
	_, _ = rand.Read(bytes)
	return hex.EncodeToString(bytes)
}

func localPorts(ports ...string) testcontainers.CustomizeRequestOption {
	return testcontainers.WithHostConfigModifier(func(hostConfig *container.HostConfig) {
		if hostConfig.PortBindings == nil {
			hostConfig.PortBindings = network.PortMap{}
		}
		for _, port := range ports {
			hostConfig.PortBindings[network.MustParsePort(port)] = []network.PortBinding{{HostIP: netip.MustParseAddr("127.0.0.1")}}
		}
	})
}

func Main(suite *testing.M) {
	os.Exit(Run(suite))
}

func Run(suite *testing.M) int {
	code := suite.Run()
	mutex.Lock()
	for _, started := range containers {
		stopping, cancel := context.WithTimeout(context.Background(), time.Minute)
		if failure := started.Terminate(stopping); failure != nil {
			fmt.Fprintf(os.Stderr, "terminate container: %v\n", failure)
		}
		cancel()
	}
	mutex.Unlock()
	return code
}

func Logger() *slog.Logger {
	if os.Getenv("TWILIGHT_TEST_LOGS") != "" {
		return slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelDebug}))
	}
	return slog.New(slog.NewJSONHandler(io.Discard, nil))
}

func PostgresURL(test testing.TB) string {
	test.Helper()
	mutex.Lock()
	defer mutex.Unlock()
	if postgresURL != "" {
		return postgresURL
	}
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	started, failure := postgres.Run(operation, PostgresImage,
		postgres.WithDatabase("inventory"),
		postgres.WithUsername("twilight"),
		postgres.WithPassword("twilight"),
		postgres.BasicWaitStrategies(),
		testcontainers.WithName(containerName("postgres")),
		localPorts("5432/tcp"),
	)
	if started != nil {
		containers = append(containers, started)
	}
	if failure != nil {
		test.Fatalf("start postgres: %v", failure)
	}
	url, failure := started.ConnectionString(operation, "sslmode=disable")
	if failure != nil {
		test.Fatalf("postgres connection string: %v", failure)
	}
	postgresURL = url
	return postgresURL
}

func EmptyDatabase(test testing.TB) (*pgxpool.Pool, string) {
	test.Helper()
	administrator := PostgresURL(test)
	operation, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	connection, failure := pgx.Connect(operation, administrator)
	if failure != nil {
		test.Fatalf("connect to postgres: %v", failure)
	}
	name := "test_" + randomSuffix()
	if _, failure := connection.Exec(operation, "create database "+pgx.Identifier{name}.Sanitize()); failure != nil {
		test.Fatalf("create database: %v", failure)
	}
	connection.Close(operation)
	configuration, failure := pgx.ParseConfig(administrator)
	if failure != nil {
		test.Fatalf("parse postgres url: %v", failure)
	}
	url := fmt.Sprintf("postgres://twilight:twilight@%s:%d/%s?sslmode=disable", configuration.Host, configuration.Port, name)
	pool, failure := database.Open(operation, url, 16)
	if failure != nil {
		test.Fatalf("open database: %v", failure)
	}
	test.Cleanup(pool.Close)
	return pool, url
}

func Database(test testing.TB) (*pgxpool.Pool, string) {
	test.Helper()
	pool, url := EmptyDatabase(test)
	operation, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	loaded, failure := database.LoadMigrations(migrations.Files)
	if failure != nil {
		test.Fatalf("load migrations: %v", failure)
	}
	if _, failure := database.Migrate(operation, pool, loaded, Logger()); failure != nil {
		test.Fatalf("migrate: %v", failure)
	}
	if _, failure := database.MaintainIntendedProcessPartitions(operation, pool, time.Now(), 3, 31*24*time.Hour); failure != nil {
		test.Fatalf("intended_processes partitions: %v", failure)
	}
	return pool, url
}

func Brokers(test testing.TB) string {
	test.Helper()
	mutex.Lock()
	defer mutex.Unlock()
	if brokers != "" {
		return brokers
	}
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	started, failure := redpanda.Run(operation, RedpandaImage,
		testcontainers.WithName(containerName("redpanda")),
		localPorts("9092/tcp", "9644/tcp", "8081/tcp", "8082/tcp"),
	)
	if started != nil {
		containers = append(containers, started)
	}
	if failure != nil {
		test.Fatalf("start redpanda: %v", failure)
	}
	seed, failure := started.KafkaSeedBroker(operation)
	if failure != nil {
		test.Fatalf("redpanda seed broker: %v", failure)
	}
	brokers = seed
	return brokers
}

func AlertStore(test testing.TB, pool *pgxpool.Pool) *alerts.Store {
	store, failure := alerts.NewStore(pool, config.Default().Alerts, "test", Logger())
	if failure != nil {
		test.Fatal(failure)
	}
	return store
}

func Mailpit(test testing.TB, environment map[string]string, files ...testcontainers.ContainerFile) (string, string) {
	test.Helper()
	operation, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	started, failure := testcontainers.Run(operation, MailpitImage,
		testcontainers.WithName(containerName("mailpit")),
		testcontainers.WithExposedPorts("1025/tcp", "8025/tcp"),
		testcontainers.WithEnv(environment),
		testcontainers.WithFiles(files...),
		testcontainers.WithWaitStrategy(wait.ForHTTP("/livez").WithPort("8025/tcp")),
		localPorts("1025/tcp", "8025/tcp"),
	)
	if started != nil {
		test.Cleanup(func() {
			stopping, cancel := context.WithTimeout(context.Background(), time.Minute)
			defer cancel()
			if failure := started.Terminate(stopping); failure != nil {
				test.Logf("terminate mailpit: %v", failure)
			}
		})
	}
	if failure != nil {
		test.Fatalf("start mailpit: %v", failure)
	}
	smtpEndpoint, failure := started.PortEndpoint(operation, "1025/tcp", "")
	if failure != nil {
		test.Fatalf("mailpit smtp port: %v", failure)
	}
	apiEndpoint, failure := started.PortEndpoint(operation, "8025/tcp", "http")
	if failure != nil {
		test.Fatalf("mailpit api port: %v", failure)
	}
	return smtpEndpoint, apiEndpoint
}
