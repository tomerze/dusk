package engine

import (
	"bytes"
	"context"
	"encoding/json"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
)

const (
	FeedPresence = "presence"
	FeedCounters = "counters"
	FeedAlerts   = "alerts"
)

type FeedEvent struct {
	Kind string          `json:"kind"`
	Time time.Time       `json:"time"`
	Data json.RawMessage `json:"data"`
}

type PresenceCounts struct {
	Online   int  `json:"online"`
	Degraded bool `json:"degraded"`
}

type CampaignCounters struct {
	CampaignID uuid.UUID          `json:"campaign_id"`
	Status     campaign.Status    `json:"status"`
	Phase      int                `json:"phase"`
	Counters   []campaign.Counter `json:"counters"`
}

type AlertCounts struct {
	Open           map[alerts.Severity]int64 `json:"open"`
	Unacknowledged map[alerts.Severity]int64 `json:"unacknowledged"`
	Latest         *alerts.Alert             `json:"latest"`
}

var droppedSubscribers = promauto.NewCounter(prometheus.CounterOpts{
	Name: "twilight_feed_subscribers_dropped_total",
	Help: "Event stream subscribers ended because they did not take the feed's events as fast as it published them.",
})

type Feed struct {
	engine      *Engine
	mutex       sync.Mutex
	subscribers map[chan FeedEvent]struct{}
	latest      map[string]FeedEvent
	interval    time.Duration
}

func newFeed(engine *Engine) *Feed {
	return &Feed{engine: engine, subscribers: map[chan FeedEvent]struct{}{}, latest: map[string]FeedEvent{}, interval: 5 * time.Second}
}

func (feed *Feed) Subscribe(operation context.Context) <-chan FeedEvent {
	channel := make(chan FeedEvent, 64)
	feed.mutex.Lock()
	for _, event := range feed.latest {
		channel <- event
	}
	feed.subscribers[channel] = struct{}{}
	feed.mutex.Unlock()
	go func() {
		<-operation.Done()
		feed.mutex.Lock()
		defer feed.mutex.Unlock()
		if _, subscribed := feed.subscribers[channel]; subscribed {
			delete(feed.subscribers, channel)
			close(channel)
		}
	}()
	return channel
}

func (feed *Feed) publish(kind string, data any) {
	encoded, failure := json.Marshal(data)
	if failure != nil {
		feed.engine.Logger.Warn("a feed event could not be encoded", "kind", kind, "error", failure)
		return
	}
	feed.mutex.Lock()
	defer feed.mutex.Unlock()
	if previous, found := feed.latest[kind]; found && bytes.Equal(previous.Data, encoded) {
		return
	}
	event := FeedEvent{Kind: kind, Time: time.Now().UTC(), Data: encoded}
	feed.latest[kind] = event
	dropped := 0
	for channel := range feed.subscribers {
		select {
		case channel <- event:
		default:
			delete(feed.subscribers, channel)
			close(channel)
			dropped++
		}
	}
	if dropped > 0 {
		droppedSubscribers.Add(float64(dropped))
		feed.engine.Logger.Warn("event streams that fell behind were ended so their clients reconnect to a fresh snapshot", "kind", kind, "streams", dropped)
	}
}

func (feed *Feed) run(operation context.Context) {
	for sleep(operation, feed.interval) {
		feed.publish(FeedPresence, PresenceCounts{Online: feed.engine.view.OnlineCount(), Degraded: feed.engine.view.Degraded()})
		if counters, failure := feed.engine.campaignCounters(operation); failure == nil {
			feed.publish(FeedCounters, counters)
		} else if operation.Err() == nil {
			feed.engine.Logger.Warn("feed counters were not read", "error", failure)
		}
		counts, failure := feed.engine.Alerts.Counts(operation)
		if failure != nil {
			if operation.Err() == nil {
				feed.engine.Logger.Warn("feed alert counts were not read", "error", failure)
			}
			continue
		}
		latest, failure := feed.engine.Alerts.List(operation, true, "", 1)
		if failure != nil {
			if operation.Err() == nil {
				feed.engine.Logger.Warn("the feed's latest alert was not read", "error", failure)
			}
			continue
		}
		summary := AlertCounts{Open: counts.Open, Unacknowledged: counts.Unacknowledged}
		if len(latest.Alerts) > 0 {
			summary.Latest = &latest.Alerts[0]
		}
		feed.publish(FeedAlerts, summary)
	}
}

func (engine *Engine) campaignCounters(operation context.Context) ([]CampaignCounters, error) {
	active, failure := engine.Campaigns.Active(operation)
	if failure != nil {
		return nil, failure
	}
	identifiers := make([]uuid.UUID, len(active))
	for index, current := range active {
		identifiers[index] = current.ID
	}
	counters, failure := engine.Campaigns.Counters(operation, identifiers)
	if failure != nil {
		return nil, failure
	}
	summaries := make([]CampaignCounters, len(active))
	for index, current := range active {
		summaries[index] = CampaignCounters{CampaignID: current.ID, Status: current.Status, Phase: current.CurrentPhase, Counters: counters[current.ID]}
		if summaries[index].Counters == nil {
			summaries[index].Counters = []campaign.Counter{}
		}
	}
	return summaries, nil
}
