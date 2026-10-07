package dawn

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"sort"
	"strconv"
	"sync"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/config"
	"dusk/services/twilight/internal/database"
)

type NodeRef struct {
	DeviceID       string  `json:"device_id"`
	InstallationID string  `json:"installation_id"`
	NamespaceID    string  `json:"namespace_id"`
	Nightfall      *string `json:"nightfall"`
}

func (node NodeRef) Key() string {
	return node.DeviceID + "/" + node.InstallationID
}

type StreamLogs struct {
	Level           string `json:"level"`
	DurationSeconds int    `json:"duration_seconds"`
}

type Work struct {
	Pid            campaign.Pid `json:"pid"`
	CampaignID     *string      `json:"campaign_id"`
	Attempt        *int         `json:"attempt"`
	Kind           string       `json:"kind"`
	Script         *string      `json:"script"`
	TimeoutSeconds int          `json:"timeout_seconds"`
	VersionKey     *string      `json:"version_key"`
	DesiredVersion *string      `json:"desired_version"`
	ConfigHash     *string      `json:"config_hash"`
	CollectFacts   bool         `json:"collect_facts"`
	CollectFiles   []string     `json:"collect_files"`
	StreamLogs     *StreamLogs  `json:"stream_logs"`
}

type Reported struct {
	VersionKey *string                    `json:"version_key"`
	Version    *string                    `json:"version"`
	ConfigHash *string                    `json:"config_hash"`
	Services   []string                   `json:"services"`
	Facts      map[string]json.RawMessage `json:"facts"`
}

type Facts struct {
	Facts    map[string]json.RawMessage `json:"facts"`
	Reported *Reported                  `json:"reported"`
}

type Outcome int

const (
	Accepted Outcome = iota
	Busy
	NotDelivered
	Ambiguous
	Rejected
)

func (outcome Outcome) String() string {
	return [...]string{"accepted", "busy", "not_delivered", "ambiguous", "rejected"}[outcome]
}

type Error struct {
	Outcome  Outcome
	Endpoint string
	Status   int
	Message  string
}

func (failure *Error) Error() string {
	if failure.Status != 0 {
		return fmt.Sprintf("dawn %s answered %d (%s): %s", failure.Endpoint, failure.Status, failure.Outcome, failure.Message)
	}
	return fmt.Sprintf("dawn %s: %s (%s)", failure.Endpoint, failure.Message, failure.Outcome)
}

func OutcomeOf(failure error) Outcome {
	var dawnError *Error
	if failure == nil {
		return Accepted
	}
	if errors.As(failure, &dawnError) {
		return dawnError.Outcome
	}
	return Ambiguous
}

var (
	requests = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_dawn_requests_total",
		Help: "Calls twilight made to dawn, by route and outcome.",
	}, []string{"route", "outcome"})
	requestSeconds = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "twilight_dawn_request_seconds",
		Help:    "Duration of calls twilight made to dawn.",
		Buckets: prometheus.ExponentialBuckets(0.005, 2, 14),
	}, []string{"route"})
	endpointsGauge = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_dawn_endpoints",
		Help: "Healthy dawn endpoints twilight routes to.",
	})
)

type Resolver interface {
	LookupHost(operation context.Context, host string) ([]string, error)
}

type Client struct {
	settings  config.Dawn
	http      *http.Client
	scheme    string
	resolver  Resolver
	logger    *slog.Logger
	mutex     sync.Mutex
	endpoints []string
	unhealthy map[string]time.Time
	now       func() time.Time
}

func tlsConfig(settings config.Dawn) (*tls.Config, error) {
	authorities, failure := os.ReadFile(settings.CA)
	if failure != nil {
		return nil, fmt.Errorf("read dawn.ca: %w", failure)
	}
	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(authorities) {
		return nil, fmt.Errorf("dawn.ca %s holds no certificate", settings.CA)
	}
	if _, failure := tls.LoadX509KeyPair(settings.Certificate, settings.Key); failure != nil {
		return nil, fmt.Errorf("load the dawn client certificate: %w", failure)
	}
	certificatePath, keyPath := settings.Certificate, settings.Key
	return &tls.Config{
		RootCAs:    pool,
		ServerName: settings.ServerName,
		MinVersion: tls.VersionTLS13,
		GetClientCertificate: func(*tls.CertificateRequestInfo) (*tls.Certificate, error) {
			pair, failure := tls.LoadX509KeyPair(certificatePath, keyPath)
			if failure != nil {
				return nil, fmt.Errorf("load the dawn client certificate: %w", failure)
			}
			return &pair, nil
		},
	}, nil
}

func New(settings config.Dawn, resolver Resolver, logger *slog.Logger) (*Client, error) {
	transport := &http.Transport{
		Proxy:                 nil,
		DialContext:           (&net.Dialer{Timeout: 5 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		MaxIdleConns:          512,
		MaxIdleConnsPerHost:   128,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ResponseHeaderTimeout: time.Duration(settings.RequestTimeoutSeconds) * time.Second,
		ForceAttemptHTTP2:     true,
	}
	scheme := "http"
	if !settings.AllowPlaintext {
		configured, failure := tlsConfig(settings)
		if failure != nil {
			return nil, failure
		}
		transport.TLSClientConfig = configured
		scheme = "https"
	}
	if resolver == nil {
		resolver = net.DefaultResolver
	}
	client := &Client{
		settings:  settings,
		http:      &http.Client{Transport: transport, Timeout: time.Duration(settings.RequestTimeoutSeconds) * time.Second},
		scheme:    scheme,
		resolver:  resolver,
		logger:    logger,
		unhealthy: map[string]time.Time{},
		now:       time.Now,
	}
	if len(settings.Endpoints) > 0 {
		client.endpoints = append([]string(nil), settings.Endpoints...)
		sort.Strings(client.endpoints)
		endpointsGauge.Set(float64(len(client.endpoints)))
	}
	return client, nil
}

func (client *Client) Resolve(operation context.Context) error {
	if len(client.settings.Endpoints) > 0 {
		return nil
	}
	resolving, cancel := context.WithTimeout(operation, 5*time.Second)
	defer cancel()
	addresses, failure := client.resolver.LookupHost(resolving, client.settings.Service)
	if failure != nil {
		return fmt.Errorf("resolve dawn service %s: %w", client.settings.Service, failure)
	}
	endpoints := make([]string, 0, len(addresses))
	for _, address := range addresses {
		endpoints = append(endpoints, net.JoinHostPort(address, strconv.Itoa(client.settings.Port)))
	}
	sort.Strings(endpoints)
	client.mutex.Lock()
	changed := len(endpoints) != len(client.endpoints)
	for index := 0; !changed && index < len(endpoints); index++ {
		changed = endpoints[index] != client.endpoints[index]
	}
	client.endpoints = endpoints
	client.mutex.Unlock()
	endpointsGauge.Set(float64(len(endpoints)))
	if changed {
		client.logger.Info("dawn endpoints changed", "service", client.settings.Service, "endpoints", endpoints)
	}
	return nil
}

func (client *Client) RunResolver(operation context.Context) {
	interval := time.Duration(client.settings.ResolveIntervalSeconds) * time.Second
	for {
		if failure := client.Resolve(operation); failure != nil {
			client.logger.Warn("dawn endpoints were not refreshed", "error", failure)
		}
		select {
		case <-operation.Done():
			return
		case <-time.After(interval):
		}
	}
}

func score(key, endpoint string) uint64 {
	digest := sha256.Sum256([]byte(key + "\x00" + endpoint))
	return binary.BigEndian.Uint64(digest[:8])
}

func Rank(key string, endpoints []string) []string {
	ranked := append([]string(nil), endpoints...)
	sort.Slice(ranked, func(left, right int) bool {
		leftScore, rightScore := score(key, ranked[left]), score(key, ranked[right])
		if leftScore != rightScore {
			return leftScore > rightScore
		}
		return ranked[left] < ranked[right]
	})
	return ranked
}

func (client *Client) route(key string) (string, error) {
	client.mutex.Lock()
	defer client.mutex.Unlock()
	if len(client.endpoints) == 0 {
		return "", &Error{Outcome: NotDelivered, Endpoint: client.settings.Service, Message: "no dawn endpoint is known"}
	}
	now := client.now()
	ranked := Rank(key, client.endpoints)
	for _, endpoint := range ranked {
		if until, marked := client.unhealthy[endpoint]; !marked || now.After(until) {
			delete(client.unhealthy, endpoint)
			return endpoint, nil
		}
	}
	return ranked[0], nil
}

func (client *Client) markUnhealthy(endpoint string) {
	client.mutex.Lock()
	client.unhealthy[endpoint] = client.now().Add(30 * time.Second)
	client.mutex.Unlock()
}

const maximumResponseBytes = 16 << 20

func (client *Client) post(operation context.Context, route, key string, body any, response any, expected int) error {
	endpoint, failure := client.route(key)
	if failure != nil {
		requests.WithLabelValues(route, NotDelivered.String()).Inc()
		return failure
	}
	encoded, failure := json.Marshal(body)
	if failure != nil {
		return failure
	}
	started := time.Now()
	defer func() { requestSeconds.WithLabelValues(route).Observe(time.Since(started).Seconds()) }()
	request, failure := http.NewRequestWithContext(operation, http.MethodPost, client.scheme+"://"+endpoint+route, bytes.NewReader(encoded))
	if failure != nil {
		return failure
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("Accept", "application/json")
	answer, failure := client.http.Do(request)
	if failure != nil {
		outcome := Ambiguous
		var operationError *net.OpError
		if errors.As(failure, &operationError) && operationError.Op == "dial" {
			outcome = NotDelivered
			client.markUnhealthy(endpoint)
		}
		requests.WithLabelValues(route, outcome.String()).Inc()
		return &Error{Outcome: outcome, Endpoint: endpoint, Message: failure.Error()}
	}
	defer answer.Body.Close()
	content, failure := io.ReadAll(io.LimitReader(answer.Body, maximumResponseBytes+1))
	if failure != nil {
		requests.WithLabelValues(route, Ambiguous.String()).Inc()
		return &Error{Outcome: Ambiguous, Endpoint: endpoint, Status: answer.StatusCode, Message: "reading the response: " + failure.Error()}
	}
	if answer.StatusCode != expected {
		outcome := Rejected
		switch {
		case answer.StatusCode == http.StatusTooManyRequests:
			outcome = Busy
		case answer.StatusCode == http.StatusServiceUnavailable:
			outcome = NotDelivered
		case answer.StatusCode >= 500:
			outcome = Ambiguous
		}
		requests.WithLabelValues(route, outcome.String()).Inc()
		message := string(content)
		if len(message) > 1024 {
			message = message[:1024]
		}
		return &Error{Outcome: outcome, Endpoint: endpoint, Status: answer.StatusCode, Message: message}
	}
	if len(content) > maximumResponseBytes {
		requests.WithLabelValues(route, Rejected.String()).Inc()
		return &Error{Outcome: Rejected, Endpoint: endpoint, Status: answer.StatusCode, Message: "the response is larger than 16 MiB"}
	}
	requests.WithLabelValues(route, Accepted.String()).Inc()
	if response == nil {
		return nil
	}
	if failure := json.Unmarshal(database.Storable(content), response); failure != nil {
		return &Error{Outcome: Rejected, Endpoint: endpoint, Status: answer.StatusCode, Message: "undecodable response: " + failure.Error()}
	}
	return nil
}

type dispatchRequest struct {
	Node NodeRef `json:"node"`
	Work []Work  `json:"work"`
}

type dispatchResponse struct {
	Accepted []campaign.Pid `json:"accepted"`
}

func (client *Client) Dispatch(operation context.Context, node NodeRef, work []Work) ([]campaign.Pid, error) {
	if work == nil {
		work = []Work{}
	}
	for index := range work {
		if work[index].CollectFiles == nil {
			work[index].CollectFiles = []string{}
		}
	}
	var response dispatchResponse
	if failure := client.post(operation, "/v1/dispatch", node.Key(), dispatchRequest{Node: node, Work: work}, &response, http.StatusAccepted); failure != nil {
		return nil, failure
	}
	return response.Accepted, nil
}

type factsRequest struct {
	Node        NodeRef      `json:"node"`
	Pid         campaign.Pid `json:"pid"`
	VersionKeys []string     `json:"version_keys"`
}

func (client *Client) Facts(operation context.Context, node NodeRef, pid campaign.Pid, versionKeys []string) (Facts, error) {
	if versionKeys == nil {
		versionKeys = []string{}
	}
	var response Facts
	failure := client.post(operation, "/v1/facts", node.Key(), factsRequest{Node: node, Pid: pid, VersionKeys: versionKeys}, &response, http.StatusOK)
	return response, failure
}

type logsRequest struct {
	Node            NodeRef      `json:"node"`
	Pid             campaign.Pid `json:"pid"`
	Level           string       `json:"level"`
	DurationSeconds int          `json:"duration_seconds"`
	Endpoint        *string      `json:"endpoint"`
}

type logsResponse struct {
	StreamID string `json:"stream_id"`
}

func (client *Client) Logs(operation context.Context, node NodeRef, pid campaign.Pid, level string, durationSeconds int, endpoint *string) (string, error) {
	var response logsResponse
	failure := client.post(operation, "/v1/logs", node.Key(), logsRequest{Node: node, Pid: pid, Level: level, DurationSeconds: durationSeconds, Endpoint: endpoint}, &response, http.StatusAccepted)
	return response.StreamID, failure
}

type filesRequest struct {
	Node       NodeRef      `json:"node"`
	Pid        campaign.Pid `json:"pid"`
	Path       string       `json:"path"`
	CampaignID *string      `json:"campaign_id"`
}

type filesResponse struct {
	UploadID string `json:"upload_id"`
}

func (client *Client) Files(operation context.Context, node NodeRef, pid campaign.Pid, path string, campaignID *string) (string, error) {
	var response filesResponse
	failure := client.post(operation, "/v1/files", node.Key(), filesRequest{Node: node, Pid: pid, Path: path, CampaignID: campaignID}, &response, http.StatusAccepted)
	return response.UploadID, failure
}
