//go:build integration

package engine

import (
	"context"
	"math"
	"slices"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/testsupport"
)

type spent struct {
	mutex sync.Mutex
	times []time.Time
}

func (record *spent) add(at time.Time) {
	record.mutex.Lock()
	defer record.mutex.Unlock()
	record.times = append(record.times, at)
}

func (record *spent) count() int {
	record.mutex.Lock()
	defer record.mutex.Unlock()
	return len(record.times)
}

func (record *spent) sorted() []time.Time {
	record.mutex.Lock()
	defer record.mutex.Unlock()
	sorted := slices.Clone(record.times)
	slices.SortFunc(sorted, func(left, right time.Time) int { return left.Compare(right) })
	return sorted
}

func budgetInstance(pool *pgxpool.Pool, instance string) *Engine {
	settings := config.Default()
	settings.Instance = instance
	return New(Dependencies{Config: settings, Pool: pool, Campaigns: campaign.NewStore(pool), Logger: testsupport.Logger().With("instance", instance)})
}

func runLessee(operation context.Context, owner *Engine, demand budgetDemand, queued func() int, spend bool, record *spent) {
	for sleep(operation, 5*time.Millisecond) {
		now := time.Now()
		owner.budgets.returnExpired(operation, now)
		if demand.queued = queued(); demand.queued > 0 {
			owner.budgets.lease(operation, demand, now)
		}
		for spend && owner.budgets.take(demand.key, time.Now()) {
			record.add(time.Now())
		}
	}
}

func runningCampaign(test *testing.T, store *campaign.Store, rate float64, burst int) *campaign.Campaign {
	test.Helper()
	operation := context.Background()
	policy := campaign.Policy{Rate: campaign.Rate{PerSecond: rate, Burst: burst}, Phases: []campaign.Phase{{Name: "all", Percent: 100, BakeSeconds: 900}},
		NodeTimeoutSeconds: 600, Gates: campaign.Gates{MinSample: 1}}
	created, failure := store.Create(operation, campaign.Definition{Name: "budgeted", Selector: "has(device_id)", Action: campaign.Action{Kind: campaign.KindRunScript, Script: "ps"}, Policy: policy},
		"operator@example.org", time.Now())
	if failure != nil {
		test.Fatal(failure)
	}
	started, failure := store.Transition(operation, created.ID, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: "operator@example.org", Now: time.Now()})
	if failure != nil {
		test.Fatal(failure)
	}
	if failure := store.EnsureBudget(operation, started.ID, rate, burst); failure != nil {
		test.Fatal(failure)
	}
	return started
}

func TestInstancesTogetherKeepACampaignsRateAndBurst(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	const rate, burst = 20.0, 6
	target := runningCampaign(test, store, rate, burst)
	demand := budgetDemand{key: target.ID, rate: rate, burst: burst, running: true}
	record := &spent{}
	operation, stop := context.WithCancel(context.Background())
	defer stop()
	killed, kill := context.WithCancel(operation)
	var moved atomic.Bool
	heavy := func() int { return 1000 }
	rebalanced := func() int {
		if moved.Load() {
			return 0
		}
		return 1000
	}
	instances := []*Engine{budgetInstance(pool, "twilight-first"), budgetInstance(pool, "twilight-second"), budgetInstance(pool, "twilight-killed")}
	var running sync.WaitGroup
	running.Add(3)
	start := time.Now()
	go func() { defer running.Done(); runLessee(operation, instances[0], demand, heavy, true, record) }()
	go func() { defer running.Done(); runLessee(operation, instances[1], demand, rebalanced, true, record) }()
	go func() { defer running.Done(); runLessee(killed, instances[2], demand, heavy, true, record) }()
	time.Sleep(1500 * time.Millisecond)
	moved.Store(true)
	time.Sleep(time.Second)
	instances[2].budgets.mutex.Lock()
	lost := 0
	if held := instances[2].budgets.held[target.ID]; held != nil {
		lost = held.tokens
	}
	instances[2].budgets.mutex.Unlock()
	kill()
	beforeKill := record.count()
	time.Sleep(time.Second)
	moved.Store(false)
	time.Sleep(1500 * time.Millisecond)
	stop()
	running.Wait()
	elapsed := time.Since(start)
	times := record.sorted()
	if failure := testsupport.WithinBudget(times, start, rate, burst, len(instances)*max(1, int(math.Ceil(rate*budgetLeaseSpan.Seconds())))); failure != nil {
		test.Fatal(failure)
	}
	if afterKill := len(times) - beforeKill; afterKill < int(rate) {
		test.Fatalf("only %d tokens were spent in the 2.5 s after an instance was killed holding %d", afterKill, lost)
	}
	if total := len(times); float64(total) < 0.5*rate*elapsed.Seconds() {
		test.Fatalf("%d tokens were spent in %s at %v a second; the instances left the budget idle", total, elapsed, rate)
	}
}

func TestUnspentTokensGoBackToTheSharedBudget(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	const rate, burst = 20.0, 4
	target := runningCampaign(test, store, rate, burst)
	demand := budgetDemand{key: target.ID, rate: rate, burst: burst, running: true}
	idle, busy := budgetInstance(pool, "twilight-idle"), budgetInstance(pool, "twilight-busy")
	idleSpent, busySpent := &spent{}, &spent{}
	returnedBefore := metricValue(test, "twilight_budget_tokens_returned_total", map[string]string{"budget": "campaign"})
	leasedBefore := metricValue(test, "twilight_budget_tokens_leased_total", map[string]string{"budget": "campaign"})
	operation, stop := context.WithCancel(context.Background())
	var running sync.WaitGroup
	running.Add(2)
	start := time.Now()
	go func() {
		defer running.Done()
		runLessee(operation, idle, demand, func() int { return 50 }, false, idleSpent)
	}()
	go func() {
		defer running.Done()
		runLessee(operation, busy, demand, func() int { return 1000 }, true, busySpent)
	}()
	time.Sleep(4 * time.Second)
	stop()
	running.Wait()
	idle.budgets.returnAll()
	busy.budgets.returnAll()
	elapsed := time.Since(start)
	leased := metricValue(test, "twilight_budget_tokens_leased_total", map[string]string{"budget": "campaign"}) - leasedBefore
	returned := metricValue(test, "twilight_budget_tokens_returned_total", map[string]string{"budget": "campaign"}) - returnedBefore
	if spentTotal := float64(busySpent.count()); leased-returned != spentTotal {
		test.Fatalf("%v tokens leased, %v returned and %v spent: tokens leased and not spent were not all given back", leased, returned, spentTotal)
	}
	if returned == 0 {
		test.Fatal("the instance that spent nothing gave nothing back")
	}
	if busyCount := float64(busySpent.count()); busyCount < 0.5*rate*elapsed.Seconds() {
		test.Fatalf("the busy instance spent %v tokens in %s at %v a second; the idle one stranded the budget", busyCount, elapsed, rate)
	}
	var tokens, available float64
	if failure := pool.QueryRow(context.Background(), `select tokens, least(burst, tokens + extract(epoch from clock_timestamp() - refilled_at) * rate) from campaign_budgets where campaign_id = $1`,
		target.ID).Scan(&tokens, &available); failure != nil || available > burst {
		test.Fatalf("the budget holds %v tokens, %v available (%v); returns must never fill it past its burst of %d", tokens, available, failure, burst)
	}
}

func TestARateBelowTheInstanceCountStillDispatches(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	const rate, burst = 0.5, 1
	target := runningCampaign(test, store, rate, burst)
	demand := budgetDemand{key: target.ID, rate: rate, burst: burst, running: true}
	record := &spent{}
	operation, stop := context.WithCancel(context.Background())
	var running sync.WaitGroup
	instances := []*Engine{budgetInstance(pool, "twilight-a"), budgetInstance(pool, "twilight-b"), budgetInstance(pool, "twilight-c")}
	start := time.Now()
	for _, instance := range instances {
		running.Add(1)
		go func() {
			defer running.Done()
			runLessee(operation, instance, demand, func() int { return 5 }, true, record)
		}()
	}
	time.Sleep(6500 * time.Millisecond)
	stop()
	running.Wait()
	times := record.sorted()
	if failure := testsupport.WithinBudget(times, start, rate, burst, len(instances)*max(1, int(math.Ceil(rate*budgetLeaseSpan.Seconds())))); failure != nil {
		test.Fatal(failure)
	}
	if len(times) < 3 {
		test.Fatalf("three instances spent %d tokens of a campaign at half a token a second in 6.5 s", len(times))
	}
}

func TestPausingACampaignStopsItsLeases(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := campaign.NewStore(pool)
	operation := context.Background()
	target := runningCampaign(test, store, 50, 50)
	time.Sleep(200 * time.Millisecond)
	owner := budgetInstance(pool, "twilight-pausing")
	demand := budgetDemand{key: target.ID, rate: 50, burst: 50, running: true, queued: 100}
	owner.budgets.lease(operation, demand, time.Now())
	if !owner.budgets.take(target.ID, time.Now()) {
		test.Fatalf("a running campaign leased nothing: %+v", owner.budgets.held[target.ID])
	}
	if _, failure := store.Transition(operation, target.ID, campaign.TransitionRequest{Transition: campaign.TransitionPause, Actor: "operator@example.org", Now: time.Now()}); failure != nil {
		test.Fatal(failure)
	}
	owner.budgets.mutex.Lock()
	owner.budgets.held[target.ID] = &leasedTokens{}
	owner.budgets.mutex.Unlock()
	time.Sleep(200 * time.Millisecond)
	owner.budgets.lease(operation, demand, time.Now())
	if owner.budgets.take(target.ID, time.Now()) {
		test.Fatal("a paused campaign leased tokens")
	}
	facts := budgetDemand{key: campaign.FactsBudget, rate: 5, burst: 5, queued: 3}
	owner.budgets.lease(operation, facts, time.Now())
	owner.budgets.mutex.Lock()
	ensured := owner.budgets.ensured[campaign.FactsBudget]
	owner.budgets.mutex.Unlock()
	if ensured.rate != 5 {
		test.Fatalf("the facts budget was not created: %+v", ensured)
	}
	time.Sleep(700 * time.Millisecond)
	owner.budgets.mutex.Lock()
	owner.budgets.held[campaign.FactsBudget].attempted = time.Time{}
	owner.budgets.mutex.Unlock()
	owner.budgets.lease(operation, facts, time.Now())
	if !owner.budgets.take(campaign.FactsBudget, time.Now()) {
		test.Fatal("the facts budget, which belongs to no campaign, leased nothing")
	}
	if failure := store.EnsureBudget(operation, campaign.FactsBudget, 9, 9); failure != nil {
		test.Fatal(failure)
	}
	var rate, burst float64
	if failure := pool.QueryRow(operation, `select rate, burst from campaign_budgets where campaign_id = $1`, campaign.FactsBudget).Scan(&rate, &burst); failure != nil || rate != 9 || burst != 9 {
		test.Fatalf("the facts budget kept rate %v and burst %v (%v) after its setting changed", rate, burst, failure)
	}
	if _, failure := store.LeaseTokens(operation, uuid.New(), 1, true); failure != nil {
		test.Fatalf("leasing from a budget nobody created: %v", failure)
	}
}
