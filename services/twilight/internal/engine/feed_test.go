package engine

import (
	"context"
	"log/slog"
	"testing"
)

func TestPublishingEndsSubscribersThatFellBehind(test *testing.T) {
	feed := newFeed(&Engine{Logger: slog.New(slog.DiscardHandler)})
	operation, cancel := context.WithCancel(context.Background())
	defer cancel()
	slow := feed.Subscribe(operation)
	fast := feed.Subscribe(operation)
	for online := range 65 {
		feed.publish(FeedPresence, PresenceCounts{Online: online})
		<-fast
	}
	received := 0
	for range slow {
		received++
	}
	if received != 64 {
		test.Fatalf("the slow subscriber received %d events before it was ended, want 64", received)
	}
	feed.publish(FeedPresence, PresenceCounts{Online: 100})
	if event, open := <-fast; !open || event.Kind != FeedPresence {
		test.Fatalf("the subscriber that kept up was ended or got %+v", event)
	}
	late := feed.Subscribe(operation)
	if event := <-late; string(event.Data) != `{"online":100,"degraded":false}` {
		test.Fatalf("a new subscriber starts from %s, want the latest presence", event.Data)
	}
}
