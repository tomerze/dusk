package engine

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/database"
)

var (
	leaderGauge = promauto.NewGauge(prometheus.GaugeOpts{Name: "twilight_leader", Help: "1 while this instance holds the leadership lock."})
	termGauge   = promauto.NewGauge(prometheus.GaugeOpts{Name: "twilight_leader_term", Help: "The leadership term this instance holds, 0 when it does not lead."})
)

type Leadership struct {
	url           string
	instance      string
	logger        *slog.Logger
	elected       func(operation context.Context, term int64)
	checkInterval time.Duration
	leading       atomic.Bool
	term          atomic.Int64
}

func NewLeadership(url, instance string, logger *slog.Logger, elected func(operation context.Context, term int64)) *Leadership {
	return &Leadership{url: url, instance: instance, logger: logger, elected: elected, checkInterval: 2 * time.Second}
}

func (leadership *Leadership) Leading() bool {
	return leadership.leading.Load()
}

func (leadership *Leadership) Term() int64 {
	return leadership.term.Load()
}

var keepaliveSettings = []string{
	"set tcp_keepalives_idle = 10",
	"set tcp_keepalives_interval = 5",
	"set tcp_keepalives_count = 3",
	"set tcp_user_timeout = 15000",
}

func (leadership *Leadership) connect(operation context.Context) (*pgx.Conn, error) {
	config, failure := pgx.ParseConfig(leadership.url)
	if failure != nil {
		return nil, fmt.Errorf("parse database.leader_url: %w", failure)
	}
	config.RuntimeParams["application_name"] = "twilight-leader-" + leadership.instance
	config.ConnectTimeout = 10 * time.Second
	dialer := &net.Dialer{Timeout: 10 * time.Second, KeepAlive: 10 * time.Second}
	config.DialFunc = dialer.DialContext
	connection, failure := pgx.ConnectConfig(operation, config)
	if failure != nil {
		return nil, fmt.Errorf("connect for leadership: %w", failure)
	}
	for _, setting := range keepaliveSettings {
		if _, failure := connection.Exec(operation, setting); failure != nil {
			leadership.logger.Warn("a keepalive setting was refused on the leadership connection", "setting", setting, "error", failure)
		}
	}
	var replica bool
	if failure := connection.QueryRow(operation, `select pg_is_in_recovery()`).Scan(&replica); failure != nil || replica {
		connection.Close(operation)
		if failure == nil {
			failure = errors.New("database.leader_url reaches a replica; it must reach the primary directly")
		}
		return nil, failure
	}
	return connection, nil
}

func (leadership *Leadership) holds(operation context.Context, connection *pgx.Conn) error {
	checking, cancel := context.WithTimeout(operation, 2*time.Second)
	defer cancel()
	var held bool
	failure := connection.QueryRow(checking, `select exists (select 1 from pg_locks where locktype = 'advisory' and pid = pg_backend_pid() and granted
		and classid = ($1::bigint >> 32)::oid and objid = ($1::bigint & 4294967295)::oid and objsubid = 1)`, database.LeaderLockKey).Scan(&held)
	if failure != nil {
		return failure
	}
	if !held {
		return errors.New("the leadership lock is no longer held")
	}
	return nil
}

func sleep(operation context.Context, duration time.Duration) bool {
	select {
	case <-operation.Done():
		return false
	case <-time.After(duration):
		return true
	}
}

func jitter(base time.Duration) time.Duration {
	return base/2 + time.Duration(rand.Int64N(int64(base)/2+1))
}

func (leadership *Leadership) Run(operation context.Context) {
	backoff := time.Second
	for operation.Err() == nil {
		connection, failure := leadership.connect(operation)
		if failure != nil {
			leadership.logger.Warn("leadership connection failed", "error", failure)
			sleep(operation, time.Duration(rand.Int64N(int64(backoff))+1))
			backoff = min(2*backoff, 30*time.Second)
			continue
		}
		backoff = time.Second
		leadership.campaign(operation, connection)
		closing, cancel := context.WithTimeout(context.WithoutCancel(operation), 5*time.Second)
		if failure := connection.Close(closing); failure != nil {
			leadership.logger.Warn("closing the leadership connection", "error", failure)
		}
		cancel()
		sleep(operation, jitter(2*time.Second))
	}
}

func (leadership *Leadership) campaign(operation context.Context, connection *pgx.Conn) {
	for operation.Err() == nil {
		trying, cancel := context.WithTimeout(operation, 5*time.Second)
		var acquired bool
		failure := connection.QueryRow(trying, `select pg_try_advisory_lock($1)`, database.LeaderLockKey).Scan(&acquired)
		cancel()
		if failure != nil {
			leadership.logger.Warn("trying the leadership lock failed", "error", failure)
			return
		}
		if acquired {
			leadership.lead(operation, connection)
			return
		}
		if !sleep(operation, jitter(2*time.Second)) {
			return
		}
	}
}

func (leadership *Leadership) lead(operation context.Context, connection *pgx.Conn) {
	var term int64
	updating, cancel := context.WithTimeout(operation, 5*time.Second)
	failure := connection.QueryRow(updating, `update leadership set term = term + 1, holder = $1, acquired_at = now() where id = 1 returning term`, leadership.instance).Scan(&term)
	cancel()
	if failure != nil {
		leadership.logger.Error("the leadership lock was taken but the term was not advanced; giving it up", "error", failure)
		return
	}
	leaderContext, stop := context.WithCancel(operation)
	var finished sync.WaitGroup
	finished.Add(1)
	leadership.term.Store(term)
	leadership.leading.Store(true)
	leaderGauge.Set(1)
	termGauge.Set(float64(term))
	leadership.logger.Info("leadership acquired", "term", term, "instance", leadership.instance)
	go func() {
		defer finished.Done()
		leadership.elected(leaderContext, term)
	}()
	misses := 0
	for {
		if !sleep(leaderContext, leadership.checkInterval) {
			break
		}
		if failure := leadership.holds(leaderContext, connection); failure != nil {
			misses++
			leadership.logger.Warn("leadership check failed", "term", term, "consecutive", misses, "error", failure)
			if misses >= 2 {
				break
			}
			continue
		}
		misses = 0
	}
	leadership.leading.Store(false)
	stop()
	finished.Wait()
	leadership.term.Store(0)
	leaderGauge.Set(0)
	termGauge.Set(0)
	leadership.logger.Info("leadership released", "term", term, "instance", leadership.instance)
}
