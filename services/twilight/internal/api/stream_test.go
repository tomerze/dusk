package api

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"
	"github.com/santhosh-tekuri/jsonschema/v6"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/engine"
)

type sseEvent struct {
	name string
	data string
}

func openStream(test *testing.T, address string) (*http.Response, *bufio.Reader) {
	test.Helper()
	request, _ := http.NewRequest(http.MethodGet, address+"/api/v1/stream", nil)
	request.Header.Set("Authorization", "Bearer "+viewerToken)
	response, failure := http.DefaultClient.Do(request)
	if failure != nil {
		test.Fatal(failure)
	}
	test.Cleanup(func() { response.Body.Close() })
	return response, bufio.NewReader(response.Body)
}

func readEvent(test *testing.T, reader *bufio.Reader) (sseEvent, error) {
	test.Helper()
	var event sseEvent
	for {
		line, failure := reader.ReadString('\n')
		if failure != nil {
			return event, failure
		}
		line = strings.TrimRight(line, "\n")
		switch {
		case line == "" && (event.name != "" || event.data != ""):
			return event, nil
		case strings.HasPrefix(line, "event: "):
			event.name = strings.TrimPrefix(line, "event: ")
		case strings.HasPrefix(line, "data: "):
			event.data = strings.TrimPrefix(line, "data: ")
		}
	}
}

func feedEvent(test *testing.T, kind string, data any) engine.FeedEvent {
	encoded, failure := json.Marshal(data)
	if failure != nil {
		test.Fatal(failure)
	}
	return engine.FeedEvent{Kind: kind, Time: sampleTime(), Data: encoded}
}

func TestTheStreamDeliversFeedEvents(test *testing.T) {
	current := newHarness(test, nil)
	current.backend.events = make(chan engine.FeedEvent, 8)
	served := httptest.NewServer(current.server)
	defer served.Close()
	response, reader := openStream(test, served.URL)
	if response.StatusCode != http.StatusOK || response.Header.Get("Content-Type") != "text/event-stream" || response.Header.Get("Cache-Control") != "no-store" {
		test.Fatalf("%d %v", response.StatusCode, response.Header)
	}
	if line, _ := reader.ReadString('\n'); line != "retry: 5000\n" {
		test.Fatalf("first line %q", line)
	}
	alert := sampleAlert()
	sent := []engine.FeedEvent{
		feedEvent(test, engine.FeedPresence, engine.PresenceCounts{Online: 1204, Degraded: false}),
		feedEvent(test, engine.FeedCounters, []engine.CampaignCounters{{CampaignID: uuid.MustParse("0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e8f"), Status: campaign.StatusRunning, Phase: 1,
			Counters: []campaign.Counter{{Phase: 0, State: campaign.StateSucceeded, Count: 12}}}}),
		feedEvent(test, engine.FeedAlerts, engine.AlertCounts{Open: map[alerts.Severity]int64{alerts.Critical: 1}, Unacknowledged: map[alerts.Severity]int64{alerts.Critical: 1}, Latest: &alert}),
		feedEvent(test, engine.FeedAlerts, engine.AlertCounts{Open: map[alerts.Severity]int64{}, Unacknowledged: map[alerts.Severity]int64{}}),
	}
	schema, failure := current.schemas.Compile(documentLocation + "#/components/schemas/FeedEvent")
	if failure != nil {
		test.Fatal(failure)
	}
	for _, event := range sent {
		current.backend.events <- event
		received, failure := readEvent(test, reader)
		if failure != nil {
			test.Fatal(failure)
		}
		if received.name != event.Kind {
			test.Fatalf("event %q, want %q", received.name, event.Kind)
		}
		instance, failure := jsonschema.UnmarshalJSON(strings.NewReader(received.data))
		if failure != nil {
			test.Fatal(failure)
		}
		if failure := schema.Validate(instance); failure != nil {
			test.Fatalf("%s does not match FeedEvent: %v", received.data, failure)
		}
		var decoded engine.FeedEvent
		if failure := json.Unmarshal([]byte(received.data), &decoded); failure != nil || decoded.Kind != event.Kind || !bytes.Equal(decoded.Data, event.Data) {
			test.Fatalf("%s %v", received.data, failure)
		}
	}
	close(current.backend.events)
	if _, failure := readEvent(test, reader); failure != io.EOF {
		test.Fatalf("the stream did not end with the feed: %v", failure)
	}
}

func TestStreamsAreBoundedAndEnd(test *testing.T) {
	current := newHarness(test, func(options *Options) {
		options.MaxStreams = 1
		options.StreamLifetime = 300 * time.Millisecond
	})
	current.backend.events = make(chan engine.FeedEvent)
	served := httptest.NewServer(current.server)
	defer served.Close()
	started := time.Now()
	_, reader := openStream(test, served.URL)
	if line, _ := reader.ReadString('\n'); line != "retry: 5000\n" {
		test.Fatalf("first line %q", line)
	}
	second, _ := openStream(test, served.URL)
	if second.StatusCode != http.StatusServiceUnavailable || second.Header.Get("Retry-After") != "30" {
		test.Fatalf("a stream past max_streams answered %d", second.StatusCode)
	}
	if _, failure := io.ReadAll(reader); failure != nil {
		test.Fatal(failure)
	}
	if elapsed := time.Since(started); elapsed < 250*time.Millisecond || elapsed > 5*time.Second {
		test.Fatalf("the stream lived %s, not its lifetime", elapsed)
	}
	if current.server.streams.Load() != 0 {
		test.Fatalf("%d streams still counted", current.server.streams.Load())
	}
	drained := newHarness(test, nil)
	drained.backend.events = make(chan engine.FeedEvent)
	drainedServer := httptest.NewServer(drained.server)
	defer drainedServer.Close()
	_, drainedReader := openStream(test, drainedServer.URL)
	if line, _ := drainedReader.ReadString('\n'); line != "retry: 5000\n" {
		test.Fatalf("first line %q", line)
	}
	drained.server.Drain()
	finished := make(chan error, 1)
	go func() {
		_, failure := io.ReadAll(drainedReader)
		finished <- failure
	}()
	select {
	case failure := <-finished:
		if failure != nil {
			test.Fatal(failure)
		}
	case <-time.After(5 * time.Second):
		test.Fatal("draining did not end the stream")
	}
}

func TestTheStreamOutlivesItsWriteTimeoutOverHTTP2(test *testing.T) {
	current := newHarness(test, nil)
	current.backend.events = make(chan engine.FeedEvent, 8)
	served := httptest.NewUnstartedServer(current.server)
	served.EnableHTTP2 = true
	served.StartTLS()
	defer served.Close()
	request, _ := http.NewRequest(http.MethodGet, served.URL+"/api/v1/stream", nil)
	request.Header.Set("Authorization", "Bearer "+viewerToken)
	response, failure := served.Client().Do(request)
	if failure != nil {
		test.Fatal(failure)
	}
	defer response.Body.Close()
	if response.ProtoMajor != 2 {
		test.Fatalf("the stream was served over %s", response.Proto)
	}
	reader := bufio.NewReader(response.Body)
	if line, _ := reader.ReadString('\n'); line != "retry: 5000\n" {
		test.Fatalf("first line %q", line)
	}
	time.Sleep(streamWriteTimeout + time.Second)
	current.backend.events <- feedEvent(test, engine.FeedPresence, engine.PresenceCounts{Online: 3})
	event, failure := readEvent(test, reader)
	if failure != nil || event.name != engine.FeedPresence {
		test.Fatalf("an HTTP/2 stream quiet for longer than its write timeout was reset: %+v %v", event, failure)
	}
}
