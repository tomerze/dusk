package engine

import (
	"context"
	"log/slog"
	"math/rand/v2"
	"sync"
	"sync/atomic"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"github.com/twmb/franz-go/pkg/kgo"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

type Dawn interface {
	Dispatch(operation context.Context, node dawn.NodeRef, work []dawn.Work) ([]campaign.Pid, error)
	Reap(operation context.Context, node dawn.NodeRef, pids []campaign.Pid) error
	Facts(operation context.Context, node dawn.NodeRef, pid campaign.Pid, versionKeys []string) (dawn.Facts, error)
	Logs(operation context.Context, node dawn.NodeRef, pid campaign.Pid, level string, durationSeconds int, endpoint *string) (string, error)
	Files(operation context.Context, node dawn.NodeRef, pid campaign.Pid, path string, campaignID *string) (string, error)
}

type NodeStateProducer interface {
	ProduceNodeState(operation context.Context, state kafka.NodeState) error
	ProduceNodeStates(operation context.Context, states []kafka.NodeState) error
	ClearNodeState(operation context.Context, key string) error
}

type IntendedProcessProducer interface {
	ProduceIntendedProcess(operation context.Context, intended kafka.IntendedProcess) error
	ClearIntendedProcess(operation context.Context, key string) error
}

type Dependencies struct {
	Config       config.Config
	Pool         *pgxpool.Pool
	Campaigns    *campaign.Store
	Inventory    *inventory.Store
	Alerts       *alerts.Store
	Dawn         Dawn
	KafkaOptions []kgo.Opt
	Validator    *kafka.Validator
	NodeState    NodeStateProducer
	Intended     IntendedProcessProducer
	Logger       *slog.Logger
}

var (
	onlineGauge   = promauto.NewGauge(prometheus.GaugeOpts{Name: "twilight_online_nodes", Help: "Nodes the online view holds as online."})
	degradedGauge = promauto.NewGauge(prometheus.GaugeOpts{Name: "twilight_view_degraded", Help: "1 while the online view is degraded."})
)

type Engine struct {
	Dependencies
	view        *View
	leadership  *Leadership
	registry    *registry
	evaluations *evaluationQueue
	dispatcher  *dispatcher
	facts       *factsQueue
	presence    *presenceWriter
	feed        *Feed
	sweeps      chan sweepRequest
	presenceUp  atomic.Bool
	topicsOK    atomic.Bool
	random      *rand.Rand
	randomMutex sync.Mutex
	factsTried  *attemptLog
}

func New(dependencies Dependencies) *Engine {
	engine := &Engine{
		Dependencies: dependencies,
		view:         NewView(),
		evaluations:  newEvaluationQueue(dependencies.Config.Engine.EvaluationQueue),
		facts:        newFactsQueue(dependencies.Config.Engine.EvaluationQueue),
		sweeps:       make(chan sweepRequest, 64),
		random:       rand.New(rand.NewPCG(uint64(time.Now().UnixNano()), 0x7477696c69676874)),
		factsTried:   newAttemptLog(1 << 20),
	}
	engine.registry = newRegistry(engine)
	engine.dispatcher = newDispatcher(engine)
	engine.presence = newPresenceWriter(engine)
	engine.feed = newFeed(engine)
	engine.leadership = NewLeadership(dependencies.Config.Database.LeaderURL, dependencies.Config.Instance, dependencies.Logger, engine.lead)
	return engine
}

func (engine *Engine) View() *View {
	return engine.view
}

func (engine *Engine) Leadership() *Leadership {
	return engine.leadership
}

func (engine *Engine) Feed() *Feed {
	return engine.feed
}

func (engine *Engine) randomFloat() float64 {
	engine.randomMutex.Lock()
	defer engine.randomMutex.Unlock()
	return engine.random.Float64()
}

func (engine *Engine) Ready() bool {
	return engine.presenceUp.Load() && engine.topicsOK.Load()
}

func (engine *Engine) Run(operation context.Context) {
	var running sync.WaitGroup
	start := func(task func(context.Context)) {
		running.Add(1)
		go func() {
			defer running.Done()
			task(operation)
		}()
	}
	start(engine.runTopicCheck)
	start(engine.runPresence)
	start(engine.runExpiry)
	start(engine.leadership.Run)
	start(engine.feed.run)
	start(engine.Alerts.RunWebhook)
	start(func(operation context.Context) { engine.Alerts.RunMetrics(operation, 30*time.Second) })
	running.Wait()
}

func (engine *Engine) runTopicCheck(operation context.Context) {
	client, failure := kgo.NewClient(engine.KafkaOptions...)
	if failure != nil {
		engine.Logger.Error("kafka client for the topic check", "error", failure)
		return
	}
	defer client.Close()
	required := kafka.RequiredTopics(engine.Config.Kafka.Topics)
	for {
		checking, cancel := context.WithTimeout(operation, 15*time.Second)
		failure := kafka.CheckTopics(checking, client, required)
		cancel()
		if failure != nil && operation.Err() == nil {
			if engine.topicsOK.Swap(false) {
				engine.Logger.Error("kafka topics are not as required; not ready", "error", failure)
			} else {
				engine.Logger.Warn("kafka topics are not as required", "error", failure)
			}
		} else if failure == nil && !engine.topicsOK.Swap(true) {
			engine.Logger.Info("kafka topics are present")
		}
		if !sleep(operation, 30*time.Second) {
			return
		}
	}
}

func (engine *Engine) runExpiry(operation context.Context) {
	census := time.Duration(engine.Config.Engine.CensusIntervalSeconds) * time.Second
	for sleep(operation, 5*time.Second) {
		now := time.Now()
		engine.onChanges(engine.view.Expire(now, census))
		if evicted := engine.view.Evict(now, census+time.Minute); evicted > 0 {
			engine.Logger.Debug("nodes offline past the replay window left the online view", "nodes", evicted)
		}
		onlineGauge.Set(float64(engine.view.OnlineCount()))
		degraded := engine.view.Degraded()
		if degraded {
			degradedGauge.Set(1)
		} else {
			degradedGauge.Set(0)
		}
		if engine.view.TakeCaughtUp() {
			engine.Logger.Info("the online view caught up after a skip; sweeping every campaign")
			engine.requestSweep(sweepRequest{all: true})
		}
	}
}

func (engine *Engine) onChanges(changes []Change) {
	if len(changes) == 0 || !engine.leadership.Leading() {
		return
	}
	engine.presence.add(changes)
}

func (engine *Engine) lead(operation context.Context, term int64) {
	for !engine.presenceUp.Load() {
		if !sleep(operation, 200*time.Millisecond) {
			return
		}
	}
	engine.Logger.Info("leader work starting", "term", term)
	if failure := engine.registry.load(operation, term); failure != nil {
		engine.Logger.Error("loading the campaigns at election failed", "term", term, "error", failure)
	}
	var running sync.WaitGroup
	start := func(name string, task func(context.Context, int64)) {
		running.Add(1)
		go func() {
			defer running.Done()
			task(operation, term)
			engine.Logger.Debug("leader task stopped", "task", name, "term", term)
		}()
	}
	start("presence", engine.presence.run)
	start("last_seen", engine.presence.runLastSeen)
	start("registry", engine.registry.run)
	start("counters", engine.registry.runFlush)
	start("evaluations", engine.runEvaluations)
	start("dispatcher", engine.dispatcher.run)
	start("facts", engine.runFacts)
	start("results", engine.runResults)
	start("deadlines", engine.runDeadlines)
	start("reaps", engine.runReapSweep)
	start("gates", engine.runGates)
	start("sweeps", engine.runSweeps)
	start("due", engine.runDueScan)
	start("quarantine", engine.runQuarantineFlips)
	start("enrollments", engine.runEnrollments)
	start("revocations", engine.runRevocationCheck)
	start("bulk revocations", engine.runRevocations)
	start("partitions", engine.runPartitionMaintenance)
	start("intents", engine.runIntentPublication)
	engine.requestSweep(sweepRequest{all: true})
	running.Wait()
	engine.dispatcher.reset()
	engine.registry.reset()
	engine.Logger.Info("leader work stopped", "term", term)
}
