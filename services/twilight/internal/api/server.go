package api

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"mime"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/tokens"
)

type Backend interface {
	Overview(operation context.Context) (engine.Overview, error)
	Nodes(operation context.Context, source string, online *bool, sort, cursor string, limit int) (engine.NodePage, error)
	Node(operation context.Context, key inventory.Key) (engine.NodeDetail, error)
	SetLifecycle(operation context.Context, key inventory.Key, lifecycle, reason, actor string, administrator bool) error
	SetDeviceLifecycle(operation context.Context, device, lifecycle, reason, actor string) (inventory.DeviceLifecycle, error)
	OpenSession(operation context.Context, key inventory.Key, reason string, lifetimeSeconds int, actor string) (engine.OperatorProcess, error)
	StreamLogs(operation context.Context, key inventory.Key, level string, durationSeconds int, actor string) (string, error)
	CollectFile(operation context.Context, key inventory.Key, path, actor string) (string, error)
	ValidateSelector(operation context.Context, source string) (engine.SelectorValidation, error)
	Campaigns(operation context.Context, statuses []campaign.Status, cursor string, limit int) (engine.CampaignPage, error)
	Campaign(operation context.Context, identifier uuid.UUID) (engine.CampaignSummary, error)
	CreateCampaign(operation context.Context, definition campaign.Definition, actor string) (*campaign.Campaign, error)
	UpdateCampaign(operation context.Context, identifier uuid.UUID, definition campaign.Definition, version int, actor string) (*campaign.Campaign, error)
	Overlap(operation context.Context, identifier uuid.UUID) (*engine.OverlapError, error)
	StartCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error)
	PauseCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error)
	ResumeCampaign(operation context.Context, identifier uuid.UUID, actor string, overrideGate bool, reason string) (*campaign.Campaign, error)
	AbortCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error)
	CompleteCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error)
	ArchiveCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error)
	CampaignNodes(operation context.Context, identifier uuid.UUID, query campaign.RowQuery) (campaign.RowPage, error)
	RetryNodes(operation context.Context, identifier uuid.UUID, selection engine.NodeSelection, actor string) (int, error)
	ResolveNodes(operation context.Context, identifier uuid.UUID, selection engine.NodeSelection, outcome string, actor string) (int, error)
	CampaignEvents(operation context.Context, identifier uuid.UUID, after int64, limit int) ([]campaign.Event, error)
	CampaignGates(operation context.Context, identifier uuid.UUID) (engine.GateStatus, error)
	Alerts(operation context.Context, open bool, cursor string, limit int) (alerts.Page, error)
	AcknowledgeAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error)
	ResolveAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error)
	Subscribe(operation context.Context) <-chan engine.FeedEvent
}

type TokenAuthenticator interface {
	Authenticate(operation context.Context, secret string) (tokens.Token, error)
}

type Options struct {
	Backend         Backend
	Tokens          TokenAuthenticator
	Sessions        SessionStore
	OIDC            *OIDCSettings
	Principals      PrincipalRoles
	Development     bool
	SessionLifetime time.Duration
	SessionIdle     time.Duration
	MaxStreams      int
	StreamLifetime  time.Duration
	MaxRequests     int
	RequestTimeout  time.Duration
	UI              http.Handler
	Logger          *slog.Logger
	Now             func() time.Time
}

type Server struct {
	Options
	mux       *http.ServeMux
	routes    []route
	streams   atomic.Int64
	requests  chan struct{}
	draining  chan struct{}
	drainOnce sync.Once
	oidc      *oidcClient
}

type route struct {
	Method string
	Path   string
	Role   Role
	handle func(*Server, *exchange) error
}

func (route route) pattern() string {
	return route.Method + " " + route.Path
}

type exchange struct {
	writer    http.ResponseWriter
	request   *http.Request
	client    context.Context
	principal *Principal
	requestID string
}

func (call *exchange) actor() string {
	return call.principal.Subject
}

const (
	maximumBodyBytes = 1 << 20
	defaultLimit     = 100
	maximumLimit     = 500
)

var (
	requestsTotal = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_api_requests_total",
		Help: "API requests answered, by route and status code.",
	}, []string{"route", "code"})
	requestSeconds = promauto.NewHistogramVec(prometheus.HistogramOpts{
		Name:    "twilight_api_request_seconds",
		Help:    "Time to answer an API request, by route; a server-sent event stream counts until it ends.",
		Buckets: prometheus.ExponentialBuckets(0.002, 2, 14),
	}, []string{"route"})
	streamsGauge = promauto.NewGauge(prometheus.GaugeOpts{
		Name: "twilight_api_streams",
		Help: "Server-sent event streams open on this instance.",
	})
	requestsRefused = promauto.NewCounter(prometheus.CounterOpts{
		Name: "twilight_api_requests_refused_total",
		Help: "API requests answered 503 because every request slot of the instance stayed taken until their deadline.",
	})
	authenticationFailures = promauto.NewCounterVec(prometheus.CounterOpts{
		Name: "twilight_api_authentication_failures_total",
		Help: "API requests refused because the caller could not be authenticated or authorized, by reason.",
	}, []string{"reason"})
)

func New(options Options) (*Server, error) {
	if options.Now == nil {
		options.Now = time.Now
	}
	if options.StreamLifetime == 0 {
		options.StreamLifetime = 5 * time.Minute
	}
	if options.UI == nil {
		options.UI = http.NotFoundHandler()
	}
	if options.RequestTimeout == 0 {
		options.RequestTimeout = 15 * time.Second
	}
	server := &Server{Options: options, mux: http.NewServeMux(), routes: routeTable(), draining: make(chan struct{})}
	if options.MaxRequests > 0 {
		server.requests = make(chan struct{}, options.MaxRequests)
	}
	if options.OIDC != nil {
		server.oidc = &oidcClient{settings: *options.OIDC}
	}
	for _, current := range server.routes {
		server.mux.Handle(current.pattern(), server.wrap(current))
	}
	server.mux.Handle("/api/", server.wrap(route{Path: "/api/", Role: RolePublic, handle: (*Server).unmatched}))
	server.mux.Handle("/", options.UI)
	return server, nil
}

func (server *Server) ServeHTTP(writer http.ResponseWriter, request *http.Request) {
	server.mux.ServeHTTP(writer, request)
}

func (server *Server) Drain() {
	server.drainOnce.Do(func() { close(server.draining) })
}

type recorder struct {
	http.ResponseWriter
	status int
}

func (recorded *recorder) WriteHeader(status int) {
	if recorded.status == 0 {
		recorded.status = status
	}
	recorded.ResponseWriter.WriteHeader(status)
}

func (recorded *recorder) Write(content []byte) (int, error) {
	if recorded.status == 0 {
		recorded.status = http.StatusOK
	}
	return recorded.ResponseWriter.Write(content)
}

func (recorded *recorder) Unwrap() http.ResponseWriter {
	return recorded.ResponseWriter
}

func newRequestID() string {
	material := make([]byte, 12)
	_, _ = rand.Read(material)
	return base64.RawURLEncoding.EncodeToString(material)
}

func (server *Server) wrap(current route) http.Handler {
	label := current.pattern()
	if current.Method == "" {
		label = "unmatched"
	}
	return http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		started := time.Now()
		recorded := &recorder{ResponseWriter: writer}
		call := &exchange{writer: recorded, request: request, client: request.Context(), requestID: newRequestID()}
		header := recorded.Header()
		header.Set("X-Request-Id", call.requestID)
		header.Set("Cache-Control", "no-store")
		header.Set("X-Content-Type-Options", "nosniff")
		if failure := server.bounded(current, call); failure != nil {
			server.fail(call, failure)
		}
		requestsTotal.WithLabelValues(label, strconv.Itoa(recorded.status)).Inc()
		requestSeconds.WithLabelValues(label).Observe(time.Since(started).Seconds())
		attributes := []any{"route", label, "status", recorded.status, "request_id", call.requestID, "seconds", time.Since(started).Seconds()}
		if call.principal != nil {
			attributes = append(attributes, "principal", call.principal.Subject)
		}
		server.Logger.Debug("api request", attributes...)
	})
}

func (server *Server) bounded(current route, call *exchange) error {
	if current.Path == streamPath {
		return server.serve(current, call)
	}
	operation, cancel := context.WithTimeout(call.request.Context(), server.RequestTimeout)
	defer cancel()
	call.request = call.request.WithContext(operation)
	if server.requests != nil {
		select {
		case server.requests <- struct{}{}:
			defer func() { <-server.requests }()
		case <-operation.Done():
			if call.client.Err() == nil {
				requestsRefused.Inc()
				server.Logger.Warn("an api request waited out its deadline for a slot", "max_requests", server.MaxRequests, "request_id", call.requestID)
				return newProblem(http.StatusServiceUnavailable, "busy", "this instance is answering as many requests as it may; try again shortly").with("retry_after_seconds", 1)
			}
			return operation.Err()
		}
	}
	return server.serve(current, call)
}

func (server *Server) serve(current route, call *exchange) error {
	if current.Role == RolePublic {
		return current.handle(server, call)
	}
	principal, failure := server.authenticate(call)
	if failure != nil {
		return failure
	}
	call.principal = principal
	if principal.role < current.Role {
		authenticationFailures.WithLabelValues("role").Inc()
		server.Logger.Warn("api request refused for its role", "principal", principal.Subject, "role", principal.Role, "required", current.Role.String(), "route", current.pattern(), "request_id", call.requestID)
		return newProblem(http.StatusForbidden, "forbidden", "this needs the "+current.Role.String()+" role; you have "+principal.Role)
	}
	if principal.session != nil && current.Method != http.MethodGet {
		if failure := server.checkCSRF(call, principal.session); failure != nil {
			return failure
		}
	}
	return current.handle(server, call)
}

func (server *Server) fail(call *exchange, failure error) {
	if call.client.Err() != nil {
		server.Logger.Debug("api request ended by the client", "request_id", call.requestID, "error", failure)
		return
	}
	problem := classify(failure)
	if problem.Status >= http.StatusInternalServerError {
		problem.with("request_id", call.requestID)
		server.Logger.Error("api request failed", "request_id", call.requestID, "method", call.request.Method, "path", call.request.URL.Path, "status", problem.Status, "code", problem.Code, "error", failure)
	}
	if seconds := retryAfter(problem); seconds != "" {
		call.writer.Header().Set("Retry-After", seconds)
	}
	if problem.Status == http.StatusUnauthorized {
		call.writer.Header().Set("WWW-Authenticate", `Bearer realm="twilight"`)
	}
	writeJSON(call, problem.Status, map[string]any{"error": problem})
}

func writeJSON(call *exchange, status int, value any) {
	encoded, failure := json.Marshal(value)
	if failure != nil {
		encoded = []byte(`{"error":{"code":"internal","message":"internal error","details":{}}}`)
		status = http.StatusInternalServerError
	}
	call.writer.Header().Set("Content-Type", "application/json")
	call.writer.WriteHeader(status)
	_, _ = call.writer.Write(append(encoded, '\n'))
}

func (server *Server) unmatched(call *exchange) error {
	var allowed []string
	for _, method := range []string{http.MethodGet, http.MethodPost, http.MethodPut, http.MethodDelete} {
		probe := call.request.Clone(call.request.Context())
		probe.Method = method
		if _, pattern := server.mux.Handler(probe); pattern != "" && pattern != "/api/" && pattern != "/" {
			allowed = append(allowed, method)
		}
	}
	if len(allowed) > 0 {
		call.writer.Header().Set("Allow", strings.Join(allowed, ", "))
		return newProblem(http.StatusMethodNotAllowed, "method_not_allowed", call.request.Method+" is not allowed here; use "+strings.Join(allowed, " or "))
	}
	return newProblem(http.StatusNotFound, "not_found", "no such API route")
}

func decodeBody(call *exchange, target any, required bool) error {
	content, failure := io.ReadAll(http.MaxBytesReader(call.writer, call.request.Body, maximumBodyBytes))
	var tooLarge *http.MaxBytesError
	if errors.As(failure, &tooLarge) {
		return newProblem(http.StatusRequestEntityTooLarge, "payload_too_large", "the request body is larger than 1 MiB")
	}
	if failure != nil {
		return invalid("the request body could not be read")
	}
	if len(strings.TrimSpace(string(content))) == 0 {
		if required {
			return invalid("a JSON request body is required")
		}
		return nil
	}
	if mediaType, _, failure := mime.ParseMediaType(call.request.Header.Get("Content-Type")); failure != nil || mediaType != "application/json" {
		return newProblem(http.StatusUnsupportedMediaType, "unsupported_media_type", "the request body must be application/json")
	}
	decoder := json.NewDecoder(bytes.NewReader(content))
	decoder.DisallowUnknownFields()
	if failure := decoder.Decode(target); failure != nil {
		return invalid("the request body is not valid: " + failure.Error())
	}
	if decoder.More() {
		return invalid("the request body holds more than one JSON value")
	}
	return nil
}

type page[Item any] struct {
	Items      []Item  `json:"items"`
	NextCursor *string `json:"next_cursor"`
}

func newPage[Item any](items []Item, next string) page[Item] {
	if items == nil {
		items = []Item{}
	}
	result := page[Item]{Items: items}
	if next != "" {
		result.NextCursor = &next
	}
	return result
}

func queryLimit(call *exchange) (int, error) {
	text := call.request.URL.Query().Get("limit")
	if text == "" {
		return defaultLimit, nil
	}
	limit, failure := strconv.Atoi(text)
	if failure != nil || limit < 1 || limit > maximumLimit {
		return 0, invalid("limit must be an integer between 1 and " + strconv.Itoa(maximumLimit))
	}
	return limit, nil
}

func queryList(call *exchange, name string) []string {
	var values []string
	for _, entry := range call.request.URL.Query()[name] {
		for item := range strings.SplitSeq(entry, ",") {
			if trimmed := strings.TrimSpace(item); trimmed != "" {
				values = append(values, trimmed)
			}
		}
	}
	return values
}
