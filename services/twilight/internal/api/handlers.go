package api

import (
	"errors"
	"net/http"
	"regexp"
	"strconv"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
)

const streamPath = "/api/v1/stream"

func routeTable() []route {
	return []route{
		{http.MethodGet, "/api/openapi.json", RolePublic, (*Server).openAPI},
		{http.MethodGet, LoginPath, RolePublic, (*Server).login},
		{http.MethodGet, CallbackPath, RolePublic, (*Server).callback},
		{http.MethodPost, "/api/v1/auth/logout", RoleViewer, (*Server).logout},
		{http.MethodGet, "/api/v1/me", RoleViewer, (*Server).me},
		{http.MethodGet, "/api/v1/overview", RoleViewer, (*Server).overview},
		{http.MethodGet, "/api/v1/nodes", RoleViewer, (*Server).nodes},
		{http.MethodGet, "/api/v1/nodes/{device}/{installation}", RoleViewer, (*Server).node},
		{http.MethodPost, "/api/v1/nodes/{device}/{installation}/lifecycle", RoleOperator, (*Server).lifecycle},
		{http.MethodPost, "/api/v1/nodes/{device}/{installation}/sessions", RoleOperator, (*Server).openSession},
		{http.MethodPost, "/api/v1/nodes/{device}/{installation}/logs", RoleOperator, (*Server).streamLogs},
		{http.MethodPost, "/api/v1/nodes/{device}/{installation}/files", RoleOperator, (*Server).collectFile},
		{http.MethodPost, "/api/v1/devices/{device}/lifecycle", RoleAdmin, (*Server).deviceLifecycle},
		{http.MethodPost, "/api/v1/selectors/validate", RoleViewer, (*Server).validateSelector},
		{http.MethodGet, "/api/v1/campaigns", RoleViewer, (*Server).campaigns},
		{http.MethodPost, "/api/v1/campaigns", RoleOperator, (*Server).createCampaign},
		{http.MethodGet, "/api/v1/campaigns/{id}", RoleViewer, (*Server).campaign},
		{http.MethodPut, "/api/v1/campaigns/{id}", RoleOperator, (*Server).updateCampaign},
		{http.MethodGet, "/api/v1/campaigns/{id}/overlap", RoleViewer, (*Server).campaignOverlap},
		{http.MethodPost, "/api/v1/campaigns/{id}/start", RoleOperator, transition(campaign.TransitionStart)},
		{http.MethodPost, "/api/v1/campaigns/{id}/pause", RoleOperator, transition(campaign.TransitionPause)},
		{http.MethodPost, "/api/v1/campaigns/{id}/resume", RoleOperator, transition(campaign.TransitionResume)},
		{http.MethodPost, "/api/v1/campaigns/{id}/abort", RoleOperator, transition(campaign.TransitionAbort)},
		{http.MethodPost, "/api/v1/campaigns/{id}/complete", RoleOperator, transition(campaign.TransitionComplete)},
		{http.MethodPost, "/api/v1/campaigns/{id}/archive", RoleOperator, transition(campaign.TransitionArchive)},
		{http.MethodGet, "/api/v1/campaigns/{id}/nodes", RoleViewer, (*Server).campaignNodes},
		{http.MethodPost, "/api/v1/campaigns/{id}/nodes/retry", RoleOperator, (*Server).retryNodes},
		{http.MethodPost, "/api/v1/campaigns/{id}/nodes/resolve", RoleOperator, (*Server).resolveNodes},
		{http.MethodGet, "/api/v1/campaigns/{id}/events", RoleViewer, (*Server).campaignEvents},
		{http.MethodGet, "/api/v1/campaigns/{id}/gates", RoleViewer, (*Server).campaignGates},
		{http.MethodGet, "/api/v1/alerts", RoleViewer, (*Server).alerts},
		{http.MethodPost, "/api/v1/alerts/{id}/acknowledge", RoleOperator, (*Server).acknowledgeAlert},
		{http.MethodPost, "/api/v1/alerts/{id}/resolve", RoleOperator, (*Server).resolveAlert},
		{http.MethodGet, streamPath, RoleViewer, (*Server).stream},
	}
}

var identifierPattern = regexp.MustCompile(`^[0-9a-f]{32}$`)

func nodeKey(call *exchange) (inventory.Key, error) {
	key := inventory.Key{DeviceID: call.request.PathValue("device"), InstallationID: call.request.PathValue("installation")}
	if !identifierPattern.MatchString(key.DeviceID) || !identifierPattern.MatchString(key.InstallationID) {
		return key, invalid("a device id and an installation id are 32 lowercase hexadecimal digits")
	}
	return key, nil
}

func campaignID(call *exchange) (uuid.UUID, error) {
	identifier, failure := uuid.Parse(call.request.PathValue("id"))
	if failure != nil {
		return uuid.Nil, invalid("a campaign id is a UUID")
	}
	return identifier, nil
}

func alertID(call *exchange) (int64, error) {
	identifier, failure := strconv.ParseInt(call.request.PathValue("id"), 10, 64)
	if failure != nil || identifier < 1 {
		return 0, invalid("an alert id is a positive integer")
	}
	return identifier, nil
}

func (server *Server) me(call *exchange) error {
	writeJSON(call, http.StatusOK, call.principal)
	return nil
}

func (server *Server) overview(call *exchange) error {
	overview, failure := server.Backend.Overview(call.request.Context())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, overview)
	return nil
}

func (server *Server) nodes(call *exchange) error {
	query := call.request.URL.Query()
	limit, failure := queryLimit(call)
	if failure != nil {
		return failure
	}
	var online *bool
	if text := query.Get("online"); text != "" {
		parsed, failure := strconv.ParseBool(text)
		if failure != nil {
			return invalid("online must be true or false")
		}
		online = &parsed
	}
	listed, failure := server.Backend.Nodes(call.request.Context(), query.Get("selector"), online, query.Get("sort"), query.Get("cursor"), limit)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, newPage(listed.Nodes, listed.Next))
	return nil
}

func (server *Server) node(call *exchange) error {
	key, failure := nodeKey(call)
	if failure != nil {
		return failure
	}
	detail, failure := server.Backend.Node(call.request.Context(), key)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, detail)
	return nil
}

func (server *Server) lifecycle(call *exchange) error {
	key, failure := nodeKey(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Lifecycle string `json:"lifecycle"`
		Reason    string `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	failure = server.Backend.SetLifecycle(call.request.Context(), key, body.Lifecycle, body.Reason, call.actor(), call.principal.role >= RoleAdmin)
	var refused *engine.LifecycleRefused
	if errors.As(failure, &refused) {
		authenticationFailures.WithLabelValues("role").Inc()
		server.Logger.Warn("api request refused for its role", "principal", call.principal.Subject, "role", call.principal.Role, "required", RoleAdmin.String(),
			"from", refused.From, "lifecycle", body.Lifecycle, "request_id", call.requestID)
		return newProblem(http.StatusForbidden, "forbidden", refused.Error()+"; you have "+call.principal.Role)
	}
	if failure != nil {
		return failure
	}
	return server.node(call)
}

func (server *Server) deviceLifecycle(call *exchange) error {
	device := call.request.PathValue("device")
	if !identifierPattern.MatchString(device) {
		return invalid("a device id is 32 lowercase hexadecimal digits")
	}
	var body struct {
		Lifecycle string `json:"lifecycle"`
		Reason    string `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	changed, failure := server.Backend.SetDeviceLifecycle(call.request.Context(), device, body.Lifecycle, body.Reason, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, changed)
	return nil
}

func (server *Server) openSession(call *exchange) error {
	key, failure := nodeKey(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Reason          string `json:"reason"`
		LifetimeSeconds int    `json:"ttl_seconds"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	granted, failure := server.Backend.OpenSession(call.request.Context(), key, body.Reason, body.LifetimeSeconds, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusCreated, granted)
	return nil
}

func (server *Server) streamLogs(call *exchange) error {
	key, failure := nodeKey(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Level           string `json:"level"`
		DurationSeconds int    `json:"duration_seconds"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	stream, failure := server.Backend.StreamLogs(call.request.Context(), key, body.Level, body.DurationSeconds, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusAccepted, map[string]string{"stream_id": stream})
	return nil
}

func (server *Server) collectFile(call *exchange) error {
	key, failure := nodeKey(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Path string `json:"path"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	upload, failure := server.Backend.CollectFile(call.request.Context(), key, body.Path, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusAccepted, map[string]string{"upload_id": upload})
	return nil
}

func (server *Server) validateSelector(call *exchange) error {
	var body struct {
		Selector *string `json:"selector"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	if body.Selector == nil {
		return invalid("selector is required")
	}
	validation, failure := server.Backend.ValidateSelector(call.request.Context(), *body.Selector)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, validation)
	return nil
}

func (server *Server) campaigns(call *exchange) error {
	limit, failure := queryLimit(call)
	if failure != nil {
		return failure
	}
	var statuses []campaign.Status
	for _, text := range queryList(call, "status") {
		status := campaign.Status(text)
		if !status.Valid() {
			return invalid(strconv.Quote(text) + " is not a campaign status")
		}
		statuses = append(statuses, status)
	}
	listed, failure := server.Backend.Campaigns(call.request.Context(), statuses, call.request.URL.Query().Get("cursor"), limit)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, newPage(listed.Campaigns, listed.Next))
	return nil
}

func (server *Server) respondCampaign(call *exchange, identifier uuid.UUID, status int) error {
	summary, failure := server.Backend.Campaign(call.request.Context(), identifier)
	if failure != nil {
		return failure
	}
	writeJSON(call, status, summary)
	return nil
}

func (server *Server) createCampaign(call *exchange) error {
	var definition campaign.Definition
	if failure := decodeBody(call, &definition, true); failure != nil {
		return failure
	}
	created, failure := server.Backend.CreateCampaign(call.request.Context(), definition, call.actor())
	if failure != nil {
		return failure
	}
	call.writer.Header().Set("Location", "/api/v1/campaigns/"+created.ID.String())
	return server.respondCampaign(call, created.ID, http.StatusCreated)
}

func (server *Server) campaign(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	return server.respondCampaign(call, identifier, http.StatusOK)
}

func (server *Server) updateCampaign(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Version *int `json:"version"`
		campaign.Definition
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	if body.Version == nil {
		return invalid("version is required: the version of the campaign the edit was made to")
	}
	if _, failure := server.Backend.UpdateCampaign(call.request.Context(), identifier, body.Definition, *body.Version, call.actor()); failure != nil {
		return failure
	}
	return server.respondCampaign(call, identifier, http.StatusOK)
}

func (server *Server) campaignOverlap(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	overlap, failure := server.Backend.Overlap(call.request.Context(), identifier)
	if failure != nil {
		return failure
	}
	if overlap.Campaigns == nil {
		overlap.Campaigns = []uuid.UUID{}
	}
	writeJSON(call, http.StatusOK, overlap)
	return nil
}

func transition(kind campaign.Transition) func(*Server, *exchange) error {
	return func(server *Server, call *exchange) error {
		identifier, failure := campaignID(call)
		if failure != nil {
			return failure
		}
		operation, actor := call.request.Context(), call.actor()
		switch kind {
		case campaign.TransitionStart, campaign.TransitionArchive:
			var body struct{}
			if failure := decodeBody(call, &body, false); failure != nil {
				return failure
			}
			if kind == campaign.TransitionStart {
				_, failure = server.Backend.StartCampaign(operation, identifier, actor)
			} else {
				_, failure = server.Backend.ArchiveCampaign(operation, identifier, actor)
			}
		case campaign.TransitionResume:
			var body struct {
				Reason       string `json:"reason"`
				OverrideGate bool   `json:"override_gate"`
			}
			if failure := decodeBody(call, &body, false); failure != nil {
				return failure
			}
			_, failure = server.Backend.ResumeCampaign(operation, identifier, actor, body.OverrideGate, body.Reason)
		default:
			var body struct {
				Reason string `json:"reason"`
			}
			if failure := decodeBody(call, &body, false); failure != nil {
				return failure
			}
			switch kind {
			case campaign.TransitionPause:
				_, failure = server.Backend.PauseCampaign(operation, identifier, actor, body.Reason)
			case campaign.TransitionAbort:
				_, failure = server.Backend.AbortCampaign(operation, identifier, actor, body.Reason)
			default:
				_, failure = server.Backend.CompleteCampaign(operation, identifier, actor, body.Reason)
			}
		}
		if failure != nil {
			return failure
		}
		return server.respondCampaign(call, identifier, http.StatusOK)
	}
}

func (server *Server) campaignNodes(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	limit, failure := queryLimit(call)
	if failure != nil {
		return failure
	}
	query := campaign.RowQuery{After: call.request.URL.Query().Get("cursor"), Limit: limit}
	for _, text := range queryList(call, "state") {
		state := campaign.NodeState(text)
		if !state.Valid() {
			return invalid(strconv.Quote(text) + " is not a node state")
		}
		query.States = append(query.States, state)
	}
	if text := call.request.URL.Query().Get("phase"); text != "" {
		phase, failure := strconv.Atoi(text)
		if failure != nil || phase < 0 {
			return invalid("phase must be a phase index, 0 or more")
		}
		query.Phase = &phase
	}
	rows, failure := server.Backend.CampaignNodes(call.request.Context(), identifier, query)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, newPage(rows.Rows, rows.Next))
	return nil
}

type nodeSelectionBody struct {
	States []campaign.NodeState `json:"states"`
	Nodes  []inventory.Key      `json:"nodes"`
	Reason string               `json:"reason"`
}

func (body nodeSelectionBody) selection() (engine.NodeSelection, error) {
	for _, key := range body.Nodes {
		if !identifierPattern.MatchString(key.DeviceID) || !identifierPattern.MatchString(key.InstallationID) {
			return engine.NodeSelection{}, invalid("a device id and an installation id are 32 lowercase hexadecimal digits")
		}
	}
	return engine.NodeSelection{States: body.States, Nodes: body.Nodes, Reason: body.Reason}, nil
}

func (server *Server) retryNodes(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	var body nodeSelectionBody
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	selection, failure := body.selection()
	if failure != nil {
		return failure
	}
	count, failure := server.Backend.RetryNodes(call.request.Context(), identifier, selection, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, map[string]int{"count": count})
	return nil
}

func (server *Server) resolveNodes(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	var body struct {
		Nodes   []inventory.Key `json:"nodes"`
		Outcome string          `json:"outcome"`
		Reason  string          `json:"reason"`
	}
	if failure := decodeBody(call, &body, true); failure != nil {
		return failure
	}
	selection, failure := nodeSelectionBody{Nodes: body.Nodes, Reason: body.Reason}.selection()
	if failure != nil {
		return failure
	}
	count, failure := server.Backend.ResolveNodes(call.request.Context(), identifier, selection, body.Outcome, call.actor())
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, map[string]int{"count": count})
	return nil
}

func (server *Server) campaignEvents(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	limit, failure := queryLimit(call)
	if failure != nil {
		return failure
	}
	var after int64
	if text := call.request.URL.Query().Get("after"); text != "" {
		if after, failure = strconv.ParseInt(text, 10, 64); failure != nil || after < 0 {
			return invalid("after must be an event id")
		}
	}
	events, failure := server.Backend.CampaignEvents(call.request.Context(), identifier, after, limit)
	if failure != nil {
		return failure
	}
	next := ""
	if len(events) == limit {
		next = strconv.FormatInt(events[len(events)-1].ID, 10)
	}
	writeJSON(call, http.StatusOK, newPage(events, next))
	return nil
}

func (server *Server) campaignGates(call *exchange) error {
	identifier, failure := campaignID(call)
	if failure != nil {
		return failure
	}
	gates, failure := server.Backend.CampaignGates(call.request.Context(), identifier)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, gates)
	return nil
}

func (server *Server) alerts(call *exchange) error {
	limit, failure := queryLimit(call)
	if failure != nil {
		return failure
	}
	open := true
	switch call.request.URL.Query().Get("state") {
	case "", "open":
	case "all":
		open = false
	default:
		return invalid("state must be open or all")
	}
	listed, failure := server.Backend.Alerts(call.request.Context(), open, call.request.URL.Query().Get("cursor"), limit)
	if failure != nil {
		return failure
	}
	writeJSON(call, http.StatusOK, newPage(listed.Alerts, listed.Next))
	return nil
}

func (server *Server) acknowledgeAlert(call *exchange) error {
	identifier, failure := alertID(call)
	if failure != nil {
		return failure
	}
	if failure := decodeBody(call, &struct{}{}, false); failure != nil {
		return failure
	}
	alert, failure := server.Backend.AcknowledgeAlert(call.request.Context(), identifier, call.actor())
	if failure != nil {
		return failure
	}
	server.Logger.Info("alert acknowledged", "alert_id", identifier, "principal", call.actor())
	writeJSON(call, http.StatusOK, alert)
	return nil
}

func (server *Server) resolveAlert(call *exchange) error {
	identifier, failure := alertID(call)
	if failure != nil {
		return failure
	}
	if failure := decodeBody(call, &struct{}{}, false); failure != nil {
		return failure
	}
	alert, failure := server.Backend.ResolveAlert(call.request.Context(), identifier, call.actor())
	if failure != nil {
		return failure
	}
	server.Logger.Info("alert resolved", "alert_id", identifier, "principal", call.actor())
	writeJSON(call, http.StatusOK, alert)
	return nil
}
