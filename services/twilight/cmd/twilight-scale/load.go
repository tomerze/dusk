package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"maps"
	"math/rand/v2"
	"net"
	"net/http"
	"net/url"
	"os"
	"slices"
	"strings"
	"time"
)

type loadSettings struct {
	twilight          string
	tokenFile         string
	run               string
	campaigns         int
	nodesPerCampaign  uint64
	linksPerSimulator uint64
	firstNode         uint64
	scripts           scriptList
	rate              float64
	burst             int
	nodeTimeout       int
	realNodes         bool
	wait              bool
	timeout           time.Duration
	poll              time.Duration
}

type scriptList []string

func (scripts *scriptList) String() string { return strings.Join(*scripts, "; ") }

func (scripts *scriptList) Set(script string) error {
	if strings.TrimSpace(script) == "" {
		return errors.New("a script cannot be empty")
	}
	*scripts = append(*scripts, script)
	return nil
}

var terminalStatuses = []string{"completed", "aborted", "failed", "archived"}

var rowStates = []string{"pending", "dispatching", "dispatched", "delivered", "verifying", "backoff", "succeeded", "failed", "unknown", "excluded", "conflict", "cancelled"}

func load(operation context.Context, arguments []string, output, diagnostics io.Writer) (int, error) {
	settings := loadSettings{}
	failure := parse("load", arguments, diagnostics, func(flags *flag.FlagSet) {
		flags.StringVar(&settings.twilight, "twilight", "http://twilight:8080", "twilight's address")
		flags.StringVar(&settings.tokenFile, "token-file", "", "a file holding a twilight API token with the operator role")
		flags.StringVar(&settings.run, "run", "scale-"+time.Now().UTC().Format("20060102T150405Z"), "the run's name; campaigns are named RUN/0, RUN/1, ...")
		flags.IntVar(&settings.campaigns, "campaigns", 1, "how many campaigns to create")
		flags.Uint64Var(&settings.nodesPerCampaign, "nodes-per-campaign", 1000, "how many simulated nodes each campaign selects")
		flags.Uint64Var(&settings.linksPerSimulator, "links-per-simulator", 1000, "the --links of every simulator, which numbers the simulated nodes")
		flags.Uint64Var(&settings.firstNode, "first-node", 0, "the simulated node the first campaign starts at, counted across simulators")
		flags.Var(&settings.scripts, "script", "a script the campaigns run; repeat it to give the campaigns different scripts in turn (default \"echo dusk-scale\")")
		flags.Float64Var(&settings.rate, "rate", 50, "processes each campaign sends per second")
		flags.IntVar(&settings.burst, "burst", 0, "processes each campaign may send at once (default: the rate, rounded up)")
		flags.IntVar(&settings.nodeTimeout, "node-timeout-seconds", 120, "how long one node's script may take")
		flags.BoolVar(&settings.realNodes, "real-nodes", false, "also select every node that is not simulated in every campaign")
		flags.BoolVar(&settings.wait, "wait", true, "wait for every campaign to finish")
		flags.DurationVar(&settings.timeout, "timeout", 2*time.Hour, "how long to wait for the campaigns to finish")
		flags.DurationVar(&settings.poll, "poll", 5*time.Second, "how often to print the campaigns' progress while waiting")
	})
	if failure != nil {
		return 0, failure
	}
	if settings.tokenFile == "" {
		return 0, usageError{"load needs --token-file"}
	}
	if settings.campaigns < 1 || settings.nodesPerCampaign < 1 || settings.linksPerSimulator < 1 {
		return 0, usageError{"--campaigns, --nodes-per-campaign and --links-per-simulator must each be at least 1"}
	}
	if len(settings.scripts) == 0 {
		settings.scripts = scriptList{"echo dusk-scale"}
	}
	token, failure := os.ReadFile(settings.tokenFile)
	if failure != nil {
		return 0, fmt.Errorf("read the token: %w", failure)
	}
	if strings.TrimSpace(string(token)) == "" {
		return 0, fmt.Errorf("the token file %s is empty", settings.tokenFile)
	}
	client := &twilightClient{base: strings.TrimRight(settings.twilight, "/"), token: strings.TrimSpace(string(token)), http: &http.Client{Timeout: 30 * time.Second}, backoff: time.Second}
	waiting, cancel := context.WithTimeout(operation, settings.timeout)
	defer cancel()
	taken, failure := campaignOfRun(waiting, client, settings.run)
	if failure != nil {
		return 0, fmt.Errorf("list the campaigns: %w", failure)
	}
	if taken != "" {
		return 0, fmt.Errorf("run %s already has the campaign %s; give this run another --run", settings.run, taken)
	}
	identifiers := make([]string, 0, settings.campaigns)
	for index := range settings.campaigns {
		definition := campaignDefinition(settings, index)
		var created campaignView
		if failure := client.do(waiting, http.MethodPost, "/api/v1/campaigns", definition, &created, false); failure != nil {
			return 0, fmt.Errorf("create campaign %s: %w", definition["name"], failure)
		}
		if failure := client.do(waiting, http.MethodPost, "/api/v1/campaigns/"+created.ID+"/start", map[string]any{}, nil, false); failure != nil {
			return 0, fmt.Errorf("start campaign %s (%s): %w", created.Name, created.ID, failure)
		}
		fmt.Fprintf(output, "started campaign %s (%s): %s\n", created.Name, created.ID, definition["selector"])
		identifiers = append(identifiers, created.ID)
	}
	if !settings.wait {
		return 0, nil
	}
	return waitForCampaigns(waiting, client, identifiers, settings.poll, output)
}

func waitForCampaigns(operation context.Context, client *twilightClient, identifiers []string, poll time.Duration, output io.Writer) (int, error) {
	began := time.Now()
	for {
		statuses := map[string]int{}
		states := map[string]int64{}
		var paused []string
		completed := 0
		for _, identifier := range identifiers {
			var current campaignView
			if failure := client.do(operation, http.MethodGet, "/api/v1/campaigns/"+identifier, nil, &current, true); failure != nil {
				return 1, fmt.Errorf("read campaign %s: %w", identifier, failure)
			}
			statuses[current.Status]++
			if current.Status == "completed" {
				completed++
			}
			if current.Status == "paused" && current.PauseReason != nil {
				paused = append(paused, current.Name+": "+*current.PauseReason)
			}
			for _, counter := range current.Counters {
				states[counter.State] += counter.Count
			}
		}
		fmt.Fprintf(output, "%6.0f s  campaigns: %s  rows: %s\n", time.Since(began).Seconds(), summarize(statuses, append([]string{"draft", "running", "paused"}, terminalStatuses...)), summarize(states, rowStates))
		for _, reason := range paused {
			fmt.Fprintf(output, "         paused %s\n", reason)
		}
		finished := 0
		for _, status := range terminalStatuses {
			finished += statuses[status]
		}
		if finished == len(identifiers) {
			if completed == len(identifiers) {
				return 0, nil
			}
			fmt.Fprintf(output, "%d of %d campaigns did not complete\n", len(identifiers)-completed, len(identifiers))
			return 1, nil
		}
		select {
		case <-operation.Done():
			fmt.Fprintf(output, "%d of %d campaigns had not finished when the wait ended\n", len(identifiers)-finished, len(identifiers))
			return 1, nil
		case <-time.After(poll):
		}
	}
}

func campaignOfRun(operation context.Context, client *twilightClient, run string) (string, error) {
	cursor := ""
	for {
		path := "/api/v1/campaigns?limit=500"
		if cursor != "" {
			path += "&cursor=" + url.QueryEscape(cursor)
		}
		var page struct {
			Items      []campaignView `json:"items"`
			NextCursor *string        `json:"next_cursor"`
		}
		if failure := client.do(operation, http.MethodGet, path, nil, &page, true); failure != nil {
			return "", failure
		}
		for _, item := range page.Items {
			if inRun(item.Name, run) {
				return item.Name, nil
			}
		}
		if page.NextCursor == nil {
			return "", nil
		}
		cursor = *page.NextCursor
	}
}

func inRun(name, run string) bool {
	index, found := strings.CutPrefix(name, run+"/")
	if !found || index == "" {
		return false
	}
	for _, character := range index {
		if character < '0' || character > '9' {
			return false
		}
	}
	return true
}

func summarize[Count int | int64](counts map[string]Count, order []string) string {
	names := append([]string(nil), order...)
	for _, name := range slices.Sorted(maps.Keys(counts)) {
		if !slices.Contains(order, name) {
			names = append(names, name)
		}
	}
	parts := []string{}
	for _, name := range names {
		if counts[name] > 0 {
			parts = append(parts, fmt.Sprintf("%s %d", name, counts[name]))
		}
	}
	if len(parts) == 0 {
		return "none"
	}
	return strings.Join(parts, ", ")
}

func deviceID(node, linksPerSimulator uint64) string {
	return fmt.Sprintf("%016x%016x", node/linksPerSimulator, node%linksPerSimulator)
}

func campaignSelector(first, end, linksPerSimulator uint64, realNodes bool) string {
	selected := fmt.Sprintf("device_id >= %q and device_id < %q", deviceID(first, linksPerSimulator), deviceID(end, linksPerSimulator))
	if realNodes {
		return "(" + selected + ") or impl != \"simulated\""
	}
	return selected
}

func campaignDefinition(settings loadSettings, index int) map[string]any {
	first := settings.firstNode + uint64(index)*settings.nodesPerCampaign
	end := first + settings.nodesPerCampaign
	return map[string]any{
		"name":        fmt.Sprintf("%s/%d", settings.run, index),
		"description": fmt.Sprintf("Scale run %s: simulated nodes %d to %d.", settings.run, first, end-1),
		"selector":    campaignSelector(first, end, settings.linksPerSimulator, settings.realNodes),
		"action":      map[string]any{"kind": "run_script", "script": settings.scripts[index%len(settings.scripts)]},
		"policy": map[string]any{
			"rate":                 map[string]any{"per_second": settings.rate, "burst": settings.burst},
			"gates":                map[string]any{"min_sample": 1, "max_failure_rate": 1, "max_silent_rate": 1, "silent_window_seconds": 0, "breakdown": []string{}},
			"node_timeout_seconds": settings.nodeTimeout,
			"retry":                map[string]any{"max_attempts": 1},
		},
	}
}

type campaignView struct {
	ID          string  `json:"id"`
	Name        string  `json:"name"`
	Status      string  `json:"status"`
	PauseReason *string `json:"pause_reason"`
	Counters    []struct {
		State string `json:"state"`
		Count int64  `json:"count"`
	} `json:"counters"`
}

type problem struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type twilightClient struct {
	base    string
	token   string
	http    *http.Client
	backoff time.Duration
}

func (client *twilightClient) do(operation context.Context, method, path string, body, into any, idempotent bool) error {
	backoff := client.backoff
	for {
		status, refusal, failure := client.once(operation, method, path, body, into)
		if failure == nil && status < 300 {
			return nil
		}
		var operationError *net.OpError
		unsent := errors.As(failure, &operationError) && operationError.Op == "dial"
		retry := failure != nil && (unsent || idempotent) ||
			status == http.StatusServiceUnavailable && (refusal.Code == "busy" || idempotent) ||
			status >= 500 && idempotent
		if failure == nil && refusal.Code == "" {
			failure = fmt.Errorf("%s %s answered %d: %s", method, path, status, refusal.Message)
		} else if failure == nil {
			failure = fmt.Errorf("%s %s answered %d: %s: %s", method, path, status, refusal.Code, refusal.Message)
		}
		if !retry {
			return failure
		}
		select {
		case <-operation.Done():
			return fmt.Errorf("%w (last failure: %w)", operation.Err(), failure)
		case <-time.After(rand.N(backoff) + time.Millisecond):
		}
		backoff = min(2*backoff, 30*time.Second)
	}
}

func (client *twilightClient) once(operation context.Context, method, path string, body, into any) (int, problem, error) {
	var content io.Reader
	if body != nil {
		encoded, failure := json.Marshal(body)
		if failure != nil {
			return 0, problem{}, failure
		}
		content = bytes.NewReader(encoded)
	}
	request, failure := http.NewRequestWithContext(operation, method, client.base+path, content)
	if failure != nil {
		return 0, problem{}, failure
	}
	request.Header.Set("Authorization", "Bearer "+client.token)
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	response, failure := client.http.Do(request)
	if failure != nil {
		return 0, problem{}, failure
	}
	defer response.Body.Close()
	answer, failure := io.ReadAll(io.LimitReader(response.Body, 16<<20))
	if failure != nil {
		return response.StatusCode, problem{}, failure
	}
	if response.StatusCode >= 300 {
		var envelope struct {
			Error problem `json:"error"`
		}
		if json.Unmarshal(answer, &envelope) != nil || envelope.Error.Code == "" {
			envelope.Error = problem{Message: strings.TrimSpace(string(answer))}
		}
		return response.StatusCode, envelope.Error, nil
	}
	if into != nil {
		if failure := json.Unmarshal(answer, into); failure != nil {
			return response.StatusCode, problem{}, fmt.Errorf("decode the answer to %s %s: %w", method, path, failure)
		}
	}
	return response.StatusCode, problem{}, nil
}
