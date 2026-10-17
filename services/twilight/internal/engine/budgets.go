package engine

import (
	"context"
	"math"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
)

const (
	budgetLeaseInterval = 100 * time.Millisecond
	budgetLeaseSpan     = 200 * time.Millisecond
	budgetTokenLifetime = time.Second
	budgetCallTimeout   = 5 * time.Second
)

var (
	budgetTokensLeased = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_budget_tokens_leased_total",
		Help: "Tokens this instance leased from the shared budgets, by budget: campaign, facts or reaps.",
	}, []string{"budget"})
	budgetTokensReturned = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_budget_tokens_returned_total",
		Help: "Leased tokens this instance did not spend within their lifetime and gave back to the shared budgets, by budget.",
	}, []string{"budget"})
)

type budgetDemand struct {
	key     uuid.UUID
	rate    float64
	burst   int
	running bool
	queued  int
}

type leasedTokens struct {
	tokens    int
	expires   time.Time
	attempted time.Time
	failing   bool
}

type budgets struct {
	engine  *Engine
	mutex   sync.Mutex
	held    map[uuid.UUID]*leasedTokens
	ensured map[uuid.UUID]budgetDemand
}

func newBudgets(engine *Engine) *budgets {
	return &budgets{engine: engine, held: map[uuid.UUID]*leasedTokens{}, ensured: map[uuid.UUID]budgetDemand{}}
}

func budgetName(key uuid.UUID) string {
	switch key {
	case campaign.FactsBudget:
		return "facts"
	case campaign.ReapsBudget:
		return "reaps"
	}
	return "campaign"
}

func (current *budgets) take(key uuid.UUID, now time.Time) bool {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	held := current.held[key]
	if held == nil || held.tokens == 0 || !now.Before(held.expires) {
		return false
	}
	held.tokens--
	return true
}

func (current *budgets) available(key uuid.UUID, now time.Time) (bool, time.Duration) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	held := current.held[key]
	if held != nil && held.tokens > 0 && now.Before(held.expires) {
		return true, 0
	}
	return false, budgetLeaseInterval
}

func (current *budgets) give(key uuid.UUID, count int) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	if held := current.held[key]; held != nil {
		held.tokens += count
	}
}

func (current *budgets) grant(key uuid.UUID, count int, now time.Time) {
	current.mutex.Lock()
	defer current.mutex.Unlock()
	held := current.held[key]
	if held == nil {
		held = &leasedTokens{}
		current.held[key] = held
	}
	held.tokens += count
	held.expires = now.Add(budgetTokenLifetime)
}

func (current *budgets) demands() []budgetDemand {
	settings := current.engine.Config.Engine
	queued := current.engine.dispatcher.queued()
	demands := []budgetDemand{
		{key: campaign.FactsBudget, rate: float64(settings.FactsPerSecond), burst: max(1, settings.FactsPerSecond), queued: current.engine.factsDemand()},
		{key: campaign.ReapsBudget, rate: float64(settings.ReapsPerSecond), burst: max(1, settings.ReapsPerSecond), queued: current.engine.dispatcher.reaps.size()},
	}
	for _, target := range current.engine.registry.targets() {
		if target.Campaign.Status != campaign.StatusRunning {
			continue
		}
		rate := target.Campaign.Policy.Rate
		demands = append(demands, budgetDemand{key: target.Campaign.ID, rate: rate.PerSecond, burst: rate.Burst, running: true, queued: queued[target.Campaign.ID]})
	}
	return demands
}

func (current *budgets) run(operation context.Context) {
	defer current.returnAll()
	for sleep(operation, budgetLeaseInterval) {
		now := time.Now()
		current.returnExpired(operation, now)
		for _, demand := range current.demands() {
			if demand.queued > 0 && demand.rate > 0 {
				current.lease(operation, demand, now)
			}
		}
	}
}

func (current *budgets) lease(operation context.Context, demand budgetDemand, now time.Time) {
	current.mutex.Lock()
	held := current.held[demand.key]
	if held == nil {
		held = &leasedTokens{}
		current.held[demand.key] = held
	}
	if (held.tokens > 0 && now.Before(held.expires)) || now.Sub(held.attempted) < budgetLeaseInterval {
		current.mutex.Unlock()
		return
	}
	held.attempted = now
	ensured, known := current.ensured[demand.key]
	current.mutex.Unlock()
	calling, cancel := context.WithTimeout(operation, budgetCallTimeout)
	defer cancel()
	if !known || ensured.rate != demand.rate || ensured.burst != demand.burst {
		if failure := current.engine.Campaigns.EnsureBudget(calling, demand.key, demand.rate, demand.burst); failure != nil {
			current.failed(operation, demand.key, "a shared budget was not created; its work waits for it", failure)
			return
		}
		current.mutex.Lock()
		current.ensured[demand.key] = demand
		current.mutex.Unlock()
	}
	wanted := min(demand.queued, max(1, int(math.Ceil(demand.rate*budgetLeaseSpan.Seconds()))))
	granted, failure := current.engine.Campaigns.LeaseTokens(calling, demand.key, wanted, demand.running)
	if failure != nil {
		current.failed(operation, demand.key, "tokens were not leased from a shared budget; its work waits for the next lease", failure)
		return
	}
	current.mutex.Lock()
	recovered := held.failing
	held.failing = false
	current.mutex.Unlock()
	if recovered {
		current.engine.Logger.Info("tokens are leased from a shared budget again", "budget", budgetName(demand.key), "budget_key", demand.key)
	}
	if granted == 0 {
		return
	}
	current.grant(demand.key, granted, now)
	budgetTokensLeased.WithLabelValues(budgetName(demand.key)).Add(float64(granted))
	current.engine.dispatcher.nudge()
}

func (current *budgets) failed(operation context.Context, key uuid.UUID, message string, failure error) {
	current.mutex.Lock()
	first := false
	if held := current.held[key]; held != nil {
		first = !held.failing
		held.failing = true
	}
	current.mutex.Unlock()
	if first && operation.Err() == nil {
		current.engine.Logger.Warn(message, "budget", budgetName(key), "budget_key", key, "error", failure)
	}
}

func (current *budgets) returnExpired(operation context.Context, now time.Time) {
	current.mutex.Lock()
	unspent := map[uuid.UUID]int{}
	for key, held := range current.held {
		if held.tokens > 0 && !now.Before(held.expires) {
			unspent[key] = held.tokens
			held.tokens = 0
		}
		if held.tokens == 0 && now.Sub(held.attempted) > time.Minute {
			delete(current.held, key)
			delete(current.ensured, key)
		}
	}
	current.mutex.Unlock()
	current.giveBack(operation, unspent)
}

func (current *budgets) returnAll() {
	current.mutex.Lock()
	unspent := map[uuid.UUID]int{}
	for key, held := range current.held {
		if held.tokens > 0 {
			unspent[key] = held.tokens
		}
	}
	current.held = map[uuid.UUID]*leasedTokens{}
	current.mutex.Unlock()
	returning, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	current.giveBack(returning, unspent)
}

func (current *budgets) giveBack(operation context.Context, unspent map[uuid.UUID]int) {
	for key, count := range unspent {
		returning, cancel := context.WithTimeout(operation, budgetCallTimeout)
		failure := current.engine.Campaigns.ReturnTokens(returning, key, count)
		cancel()
		if failure != nil {
			current.engine.Logger.Warn("unspent tokens were not given back to a shared budget; it runs below its rate until they refill", "budget", budgetName(key),
				"budget_key", key, "tokens", count, "error", failure)
			continue
		}
		budgetTokensReturned.WithLabelValues(budgetName(key)).Add(float64(count))
	}
}
