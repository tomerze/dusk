package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/selector"
)

type node map[string]string

func (columns node) Column(name string) (string, bool) {
	value, present := columns[name]
	return value, present
}

func (columns node) Fact(string) (any, bool) { return nil, false }

func TestDeviceIDNumbersNodesAcrossSimulators(test *testing.T) {
	for _, current := range []struct {
		node, links uint64
		want        string
	}{
		{0, 1000, "00000000000000000000000000000000"},
		{999, 1000, "000000000000000000000000000003e7"},
		{1000, 1000, "00000000000000010000000000000000"},
		{123456, 50000, "00000000000000020000000000005ba0"},
	} {
		if got := deviceID(current.node, current.links); got != current.want {
			test.Errorf("deviceID(%d, %d) = %s, want %s", current.node, current.links, got, current.want)
		}
	}
}

func TestSelectorSelectsExactlyItsRangeOfSimulatedNodes(test *testing.T) {
	const links = 1000
	for _, realNodes := range []bool{false, true} {
		parsed, failure := selector.Parse(campaignSelector(1500, 2500, links, realNodes))
		if failure != nil {
			test.Fatalf("twilight refuses the selector: %v", failure)
		}
		for simulated := uint64(1400); simulated < 2600; simulated++ {
			matched := parsed.Matches(node{"device_id": deviceID(simulated, links), "impl": "simulated"})
			if want := simulated >= 1500 && simulated < 2500; matched != want {
				test.Errorf("real nodes %t: node %d matched %t, want %t", realNodes, simulated, matched, want)
			}
		}
		if matched := parsed.Matches(node{"device_id": "9f86d081884c7d659a2feaa0c55ad015", "impl": "nix"}); matched != realNodes {
			test.Errorf("real nodes %t: a real node matched %t", realNodes, matched)
		}
		if parsed.Matches(node{"device_id": "9f86d081884c7d659a2feaa0c55ad015"}) {
			test.Errorf("real nodes %t: a node whose facts were never read matched", realNodes)
		}
	}
}

func TestEveryCampaignDefinitionIsOneTwilightAccepts(test *testing.T) {
	settings := loadSettings{run: "scale-test", campaigns: 3, nodesPerCampaign: 5000, linksPerSimulator: 20000, firstNode: 100,
		scripts: scriptList{"echo one", "echo two"}, rate: 250, nodeTimeout: 60, realNodes: true}
	wantScripts := []string{"echo one", "echo two", "echo one"}
	for index := range settings.campaigns {
		encoded, failure := json.Marshal(campaignDefinition(settings, index))
		if failure != nil {
			test.Fatal(failure)
		}
		var definition campaign.Definition
		if failure := json.Unmarshal(encoded, &definition); failure != nil {
			test.Fatalf("campaign %d: %v", index, failure)
		}
		definition.Normalize()
		if failure := definition.Validate(time.Now()); failure != nil {
			test.Fatalf("campaign %d is refused: %v", index, failure)
		}
		if definition.Action.Script != wantScripts[index] {
			test.Errorf("campaign %d runs %q, want %q", index, definition.Action.Script, wantScripts[index])
		}
		if definition.Policy.Rate.PerSecond != 250 || definition.Policy.Rate.Burst != 250 || definition.Policy.Retry.MaxAttempts != 1 {
			test.Errorf("campaign %d has rate %+v and retry %+v", index, definition.Policy.Rate, definition.Policy.Retry)
		}
		if len(definition.Policy.Phases) != 1 || definition.Policy.Phases[0].BakeSeconds != 60 {
			test.Errorf("campaign %d has phases %+v", index, definition.Policy.Phases)
		}
	}
	parsed, failure := selector.Parse(campaignDefinition(settings, 1)["selector"].(string))
	if failure != nil {
		test.Fatal(failure)
	}
	if !parsed.Matches(node{"device_id": deviceID(5100, 20000)}) || parsed.Matches(node{"device_id": deviceID(10100, 20000)}) {
		test.Error("campaign 1 does not select nodes 5100 to 10099")
	}
}

type answer func(*http.Request) (*http.Response, error)

type scriptedTransport struct {
	answers []answer
	calls   int
}

func (script *scriptedTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	next := script.answers[min(script.calls, len(script.answers)-1)]
	script.calls++
	return next(request)
}

func status(code int, body string) answer {
	return func(request *http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: code, Body: io.NopCloser(strings.NewReader(body)), Header: http.Header{}, Request: request}, nil
	}
}

func broken(failure error) answer {
	return func(*http.Request) (*http.Response, error) { return nil, failure }
}

func TestARequestIsSentAgainOnlyWhenItCannotHaveBeenApplied(test *testing.T) {
	refused := &net.OpError{Op: "dial", Net: "tcp", Err: errors.New("connection refused")}
	reset := &net.OpError{Op: "read", Net: "tcp", Err: errors.New("connection reset by peer")}
	busy := status(http.StatusServiceUnavailable, `{"error":{"code":"busy","message":"this instance is answering as many requests as it may; try again shortly","details":{"retry_after_seconds":1}}}`)
	timeout := status(http.StatusServiceUnavailable, `{"error":{"code":"timeout","message":"the request did not finish in time; try again shortly","details":{"retry_after_seconds":5}}}`)
	internal := status(http.StatusInternalServerError, `{"error":{"code":"internal","message":"internal error","details":{}}}`)
	created := status(http.StatusCreated, `{"id":"0192f3a4-5b6c-7d8e-9f01-23456789abcd"}`)
	for _, current := range []struct {
		name       string
		method     string
		idempotent bool
		first      answer
		sent       int
		succeeds   bool
	}{
		{"a create whose connection was refused", http.MethodPost, false, broken(refused), 2, true},
		{"a create twilight turned away as busy", http.MethodPost, false, busy, 2, true},
		{"a create twilight timed out", http.MethodPost, false, timeout, 1, false},
		{"a create twilight failed", http.MethodPost, false, internal, 1, false},
		{"a create whose connection broke after it was written", http.MethodPost, false, broken(reset), 1, false},
		{"a create the client timed out", http.MethodPost, false, broken(context.DeadlineExceeded), 1, false},
		{"a read twilight failed", http.MethodGet, true, internal, 2, true},
		{"a read whose connection broke", http.MethodGet, true, broken(reset), 2, true},
	} {
		script := &scriptedTransport{answers: []answer{current.first, created}}
		client := &twilightClient{base: "http://twilight.test", token: "token", http: &http.Client{Transport: script}, backoff: time.Millisecond}
		operation, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		failure := client.do(operation, current.method, "/api/v1/campaigns", map[string]any{}, nil, current.idempotent)
		cancel()
		if script.calls != current.sent || (failure == nil) != current.succeeds {
			test.Errorf("%s: sent %d times, error %v; want %d times, success %t", current.name, script.calls, failure, current.sent, current.succeeds)
		}
	}
}

func TestARunOwnsOnlyItsNumberedCampaigns(test *testing.T) {
	for _, current := range []struct {
		name string
		want bool
	}{
		{"scale-1/0", true},
		{"scale-1/12", true},
		{"scale-1/", false},
		{"scale-1/a", false},
		{"scale-1/0/1", false},
		{"scale-10/0", false},
		{"other/0", false},
	} {
		if got := inRun(current.name, "scale-1"); got != current.want {
			test.Errorf("inRun(%q, scale-1) = %t, want %t", current.name, got, current.want)
		}
	}
}

func TestARefusalSaysTwilightsReason(test *testing.T) {
	for _, current := range []struct {
		body string
		want string
	}{
		{`{"error":{"code":"invalid_selector","message":"unknown field \"os\"","details":{"position":0}}}`, `answered 400: invalid_selector: unknown field "os"`},
		{`upstream connect error`, `answered 400: upstream connect error`},
	} {
		script := &scriptedTransport{answers: []answer{status(http.StatusBadRequest, current.body)}}
		client := &twilightClient{base: "http://twilight.test", token: "token", http: &http.Client{Transport: script}, backoff: time.Millisecond}
		failure := client.do(context.Background(), http.MethodPost, "/api/v1/campaigns", map[string]any{}, nil, false)
		if failure == nil || !strings.Contains(failure.Error(), current.want) {
			test.Errorf("refusal %q reads %v, want it to contain %q", current.body, failure, current.want)
		}
	}
}
