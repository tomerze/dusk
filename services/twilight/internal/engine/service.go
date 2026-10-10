package engine

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
	"dusk/services/twilight/internal/selector"
)

var (
	ErrNodeOffline     = errors.New("the node is not online")
	ErrInvalidArgument = errors.New("invalid argument")
)

type OverlapError struct {
	Count     int64       `json:"count"`
	Campaigns []uuid.UUID `json:"campaigns"`
}

func (failure *OverlapError) Error() string {
	return fmt.Sprintf("%d nodes are also targeted by running campaigns of the same kind; start with policy.allow_overlap to accept it", failure.Count)
}

type Service struct {
	engine *Engine
}

func (engine *Engine) Service() *Service {
	return &Service{engine: engine}
}

func invalidArgument(format string, arguments ...any) error {
	return fmt.Errorf("%w: %s", ErrInvalidArgument, fmt.Sprintf(format, arguments...))
}

func (service *Service) CreateCampaign(operation context.Context, definition campaign.Definition, actor string) (*campaign.Campaign, error) {
	created, failure := service.engine.Campaigns.Create(operation, definition, actor, time.Now())
	if failure == nil {
		service.engine.Logger.Info("campaign created", "campaign_id", created.ID, "principal", actor, "kind", created.Kind)
	}
	return created, failure
}

type CampaignSummary struct {
	*campaign.Campaign
	Counters []campaign.Counter `json:"counters"`
}

type CampaignPage struct {
	Campaigns []CampaignSummary `json:"campaigns"`
	Next      string            `json:"next"`
}

func (service *Service) withCounters(operation context.Context, campaigns []*campaign.Campaign) ([]CampaignSummary, error) {
	identifiers := make([]uuid.UUID, len(campaigns))
	for index, current := range campaigns {
		identifiers[index] = current.ID
	}
	counters, failure := service.engine.Campaigns.Counters(operation, identifiers)
	if failure != nil {
		return nil, failure
	}
	summaries := make([]CampaignSummary, len(campaigns))
	for index, current := range campaigns {
		summaries[index] = CampaignSummary{Campaign: current, Counters: counters[current.ID]}
		if summaries[index].Counters == nil {
			summaries[index].Counters = []campaign.Counter{}
		}
	}
	return summaries, nil
}

func (service *Service) Campaign(operation context.Context, identifier uuid.UUID) (CampaignSummary, error) {
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return CampaignSummary{}, failure
	}
	summaries, failure := service.withCounters(operation, []*campaign.Campaign{current})
	if failure != nil {
		return CampaignSummary{}, failure
	}
	return summaries[0], nil
}

func (service *Service) Campaigns(operation context.Context, statuses []campaign.Status, cursor string, limit int) (CampaignPage, error) {
	page, failure := service.engine.Campaigns.List(operation, statuses, cursor, limit)
	if errors.Is(failure, campaign.ErrMalformedCursor) {
		return CampaignPage{}, invalidArgument("malformed cursor")
	}
	if failure != nil {
		return CampaignPage{}, failure
	}
	summaries, failure := service.withCounters(operation, page.Campaigns)
	if failure != nil {
		return CampaignPage{}, failure
	}
	return CampaignPage{Campaigns: summaries, Next: page.Next}, nil
}

func (service *Service) UpdateCampaign(operation context.Context, identifier uuid.UUID, definition campaign.Definition, version int, actor string) (*campaign.Campaign, error) {
	updated, failure := service.engine.Campaigns.UpdateDefinition(operation, identifier, definition, version, actor, time.Now())
	if failure == nil {
		service.engine.Logger.Info("campaign updated", "campaign_id", identifier, "principal", actor, "version", updated.Version)
	}
	return updated, failure
}

func (service *Service) Overlap(operation context.Context, identifier uuid.UUID) (*OverlapError, error) {
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return nil, failure
	}
	count, overlapping, failure := service.engine.Campaigns.Overlap(operation, current)
	if failure != nil {
		return nil, failure
	}
	return &OverlapError{Count: count, Campaigns: overlapping}, nil
}

func (service *Service) transition(operation context.Context, identifier uuid.UUID, request campaign.TransitionRequest) (*campaign.Campaign, error) {
	request.Now = time.Now()
	updated, failure := service.engine.Campaigns.Transition(operation, identifier, request)
	if failure != nil {
		return nil, failure
	}
	service.engine.registry.setStatus(identifier, updated)
	service.engine.Logger.Info("campaign transition", "campaign_id", identifier, "transition", request.Transition, "status", updated.Status, "principal", request.Actor, "reason", request.Reason)
	return updated, nil
}

func (service *Service) StartCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error) {
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return nil, failure
	}
	if current.Status == campaign.StatusDraft && current.Kind.Converging() && !current.Policy.AllowOverlap {
		count, overlapping, failure := service.engine.Campaigns.Overlap(operation, current)
		if failure != nil {
			return nil, failure
		}
		if count > 0 {
			return nil, &OverlapError{Count: count, Campaigns: overlapping}
		}
	}
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionStart, Actor: actor})
}

func (service *Service) PauseCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionPause, PauseKind: campaign.PauseOperator, Actor: actor, Reason: reason})
}

func (service *Service) ResumeCampaign(operation context.Context, identifier uuid.UUID, actor string, overrideGate bool, reason string) (*campaign.Campaign, error) {
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionResume, OverrideGate: overrideGate, Actor: actor, Reason: reason})
}

func (service *Service) AbortCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionAbort, Actor: actor, Reason: reason})
}

func (service *Service) CompleteCampaign(operation context.Context, identifier uuid.UUID, actor, reason string) (*campaign.Campaign, error) {
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionComplete, Actor: actor, Reason: reason})
}

func (service *Service) ArchiveCampaign(operation context.Context, identifier uuid.UUID, actor string) (*campaign.Campaign, error) {
	return service.transition(operation, identifier, campaign.TransitionRequest{Transition: campaign.TransitionArchive, Actor: actor})
}

func (service *Service) CampaignNodes(operation context.Context, identifier uuid.UUID, query campaign.RowQuery) (campaign.RowPage, error) {
	if _, failure := service.engine.Campaigns.Get(operation, identifier); failure != nil {
		return campaign.RowPage{}, failure
	}
	page, failure := service.engine.Campaigns.Rows(operation, identifier, query)
	if errors.Is(failure, campaign.ErrMalformedCursor) {
		return page, invalidArgument("malformed cursor")
	}
	if page.Rows == nil {
		page.Rows = []campaign.Row{}
	}
	return page, failure
}

func (service *Service) CampaignEvents(operation context.Context, identifier uuid.UUID, after int64, limit int) ([]campaign.Event, error) {
	if _, failure := service.engine.Campaigns.Get(operation, identifier); failure != nil {
		return nil, failure
	}
	events, failure := service.engine.Campaigns.Events(operation, identifier, after, limit)
	if events == nil && failure == nil {
		events = []campaign.Event{}
	}
	return events, failure
}

type GateStatus struct {
	GateReport
	Phase              int     `json:"phase"`
	PhaseName          string  `json:"phase_name"`
	BakeSeconds        int     `json:"bake_seconds"`
	BakeAccruedSeconds float64 `json:"bake_accrued_seconds"`
	Converging         bool    `json:"converging"`
}

func (service *Service) CampaignGates(operation context.Context, identifier uuid.UUID) (GateStatus, error) {
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return GateStatus{}, failure
	}
	report, failure := service.engine.Report(operation, current)
	if failure != nil {
		return GateStatus{}, failure
	}
	phase := current.Policy.Phases[min(current.CurrentPhase, len(current.Policy.Phases)-1)]
	return GateStatus{
		GateReport: report, Phase: current.CurrentPhase, PhaseName: phase.Name, BakeSeconds: phase.BakeSeconds,
		BakeAccruedSeconds: current.BakeAccrued(time.Now()).Seconds(),
		Converging:         current.Kind.Converging() && current.Status == campaign.StatusRunning && current.LastPhase() && report.Verdict == VerdictPass && current.BakeComplete(time.Now()),
	}, nil
}

type NodeSelection struct {
	States []campaign.NodeState
	Nodes  []inventory.Key
	Reason string
}

const maximumRowsPerRequest = 10000

func (service *Service) rowsFor(operation context.Context, identifier uuid.UUID, selection NodeSelection, defaultStates []campaign.NodeState) ([]campaign.Row, error) {
	if strings.TrimSpace(selection.Reason) == "" {
		return nil, invalidArgument("a reason is required")
	}
	if len(selection.Nodes) > maximumRowsPerRequest {
		return nil, invalidArgument("at most %d nodes per request", maximumRowsPerRequest)
	}
	states := selection.States
	if len(states) == 0 {
		states = defaultStates
	}
	for _, state := range states {
		if !state.Valid() {
			return nil, invalidArgument("%q is not a node state", state)
		}
	}
	devices := make([]string, len(selection.Nodes))
	installations := make([]string, len(selection.Nodes))
	for index, key := range selection.Nodes {
		devices[index], installations[index] = key.DeviceID, key.InstallationID
	}
	return service.engine.Campaigns.RowsByKeys(operation, identifier, states, devices, installations, maximumRowsPerRequest)
}

func (service *Service) RetryNodes(operation context.Context, identifier uuid.UUID, selection NodeSelection, actor string) (int, error) {
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return 0, failure
	}
	if !current.Status.Active() {
		return 0, fmt.Errorf("%w: only a running or paused campaign retries nodes", campaign.ErrInvalidTransition)
	}
	rows, failure := service.rowsFor(operation, identifier, selection, []campaign.NodeState{campaign.StateFailed, campaign.StateUnknown})
	if failure != nil {
		return 0, failure
	}
	term, failure := service.engine.Campaigns.Term(operation)
	if failure != nil {
		return 0, failure
	}
	retried := 0
	for _, row := range rows {
		next, changed := row.Retry(time.Now())
		if !changed {
			continue
		}
		if written, applied, _ := service.engine.write(operation, term, &row, next, "operator retry"); applied {
			retried++
			service.engine.hint(inventory.Key{DeviceID: written.DeviceID, InstallationID: written.InstallationID})
		}
	}
	if failure := service.engine.Campaigns.AddEvent(operation, identifier, "nodes_retried", actor, map[string]any{"count": retried, "reason": selection.Reason}); failure != nil {
		service.engine.Logger.Warn("the retry event was not written", "campaign_id", identifier, "error", failure)
	}
	service.engine.Logger.Info("campaign nodes retried", "campaign_id", identifier, "principal", actor, "count", retried)
	return retried, nil
}

func (service *Service) ResolveNodes(operation context.Context, identifier uuid.UUID, selection NodeSelection, outcome string, actor string) (int, error) {
	if outcome != "succeeded" && outcome != "failed" {
		return 0, invalidArgument("outcome must be succeeded or failed")
	}
	if len(selection.Nodes) == 0 {
		return 0, invalidArgument("name the nodes to resolve")
	}
	current, failure := service.engine.Campaigns.Get(operation, identifier)
	if failure != nil {
		return 0, failure
	}
	selection.States = []campaign.NodeState{campaign.StateUnknown}
	rows, failure := service.rowsFor(operation, identifier, selection, nil)
	if failure != nil {
		return 0, failure
	}
	term, failure := service.engine.Campaigns.Term(operation)
	if failure != nil {
		return 0, failure
	}
	resolved := 0
	for _, row := range rows {
		next, changed := row.Resolve(current, outcome == "succeeded", selection.Reason, time.Now())
		if !changed {
			continue
		}
		if _, applied, _ := service.engine.write(operation, term, &row, next, "operator resolve"); applied {
			resolved++
		}
	}
	if failure := service.engine.Campaigns.AddEvent(operation, identifier, "nodes_resolved", actor, map[string]any{"count": resolved, "outcome": outcome, "reason": selection.Reason}); failure != nil {
		service.engine.Logger.Warn("the resolve event was not written", "campaign_id", identifier, "error", failure)
	}
	return resolved, nil
}

type SelectorValidation struct {
	OK      bool            `json:"ok"`
	Error   *selector.Error `json:"error"`
	Matched int64           `json:"matched"`
	Sample  []NodeSummary   `json:"sample"`
}

func (service *Service) ValidateSelector(operation context.Context, source string) (SelectorValidation, error) {
	parsed, failure := selector.Parse(source)
	if failure != nil {
		var selectorError *selector.Error
		if errors.As(failure, &selectorError) {
			return SelectorValidation{Error: selectorError, Sample: []NodeSummary{}}, nil
		}
		return SelectorValidation{}, failure
	}
	matched, failure := service.engine.Inventory.Count(operation, parsed)
	if failure != nil {
		return SelectorValidation{}, failure
	}
	page, failure := service.engine.Inventory.List(operation, parsed, inventory.Order{Column: "device_id"}, "", 10)
	if failure != nil {
		return SelectorValidation{}, failure
	}
	sample := make([]NodeSummary, len(page.Nodes))
	for index, node := range page.Nodes {
		sample[index] = service.summarize(node)
	}
	return SelectorValidation{OK: true, Matched: matched, Sample: sample}, nil
}

type NodeSummary struct {
	inventory.Node
	Online     bool       `json:"online"`
	LastSeenAt *time.Time `json:"last_seen_at"`
}

type NodePage struct {
	Nodes []NodeSummary `json:"nodes"`
	Next  string        `json:"next"`
}

func (service *Service) summarize(node inventory.Node) NodeSummary {
	summary := NodeSummary{Node: node}
	sessions := service.engine.view.Sessions(node.Key())
	summary.Online = len(sessions) > 0
	for _, session := range sessions {
		lastSeen := session.LastSeen
		if summary.LastSeenAt == nil || lastSeen.After(*summary.LastSeenAt) {
			summary.LastSeenAt = &lastSeen
		}
	}
	return summary
}

func (service *Service) Nodes(operation context.Context, source string, online *bool, sort, cursor string, limit int) (NodePage, error) {
	parsed, failure := selector.Parse(source)
	if failure != nil {
		return NodePage{}, failure
	}
	order, failure := inventory.ParseOrder(sort)
	if failure != nil {
		return NodePage{}, invalidArgument("%v", failure)
	}
	result := NodePage{Nodes: []NodeSummary{}}
	after := cursor
	for range 20 {
		page, failure := service.engine.Inventory.List(operation, parsed, order, after, limit)
		if errors.Is(failure, inventory.ErrMalformedCursor) {
			return NodePage{}, invalidArgument("malformed cursor")
		}
		if failure != nil {
			return NodePage{}, failure
		}
		for _, node := range page.Nodes {
			after = order.CursorAfter(node)
			summary := service.summarize(node)
			if online != nil && summary.Online != *online {
				continue
			}
			result.Nodes = append(result.Nodes, summary)
			if len(result.Nodes) == limit {
				result.Next = after
				return result, nil
			}
		}
		if page.Next == "" {
			return result, nil
		}
	}
	result.Next = after
	return result, nil
}

type SessionView struct {
	NamespaceID  string    `json:"namespace_id"`
	Epoch        int64     `json:"epoch"`
	Instance     string    `json:"instance"`
	InnerAddress string    `json:"inner_address"`
	ConnectedAt  time.Time `json:"connected_at"`
	LastSeen     time.Time `json:"last_seen"`
}

type NodeDetail struct {
	NodeSummary
	Device     *inventory.DeviceLifecycle `json:"device"`
	Sessions   []SessionView              `json:"sessions"`
	Executions []campaign.Row             `json:"executions"`
}

func (service *Service) Node(operation context.Context, key inventory.Key) (NodeDetail, error) {
	node, failure := service.engine.Inventory.Get(operation, key)
	if failure != nil {
		return NodeDetail{}, failure
	}
	detail := NodeDetail{NodeSummary: service.summarize(node), Sessions: []SessionView{}}
	stored, failure := service.engine.Inventory.GetPresence(operation, key)
	switch {
	case failure == nil && !detail.Online && stored.LastSeenAt != nil:
		detail.LastSeenAt = stored.LastSeenAt
	case failure != nil && !errors.Is(failure, inventory.ErrNotFound) && operation.Err() == nil:
		service.engine.Logger.Warn("the node's stored presence was not read; its last-seen time comes from the online view only", "device_id", key.DeviceID, "installation_id", key.InstallationID, "error", failure)
	}
	for _, session := range service.engine.view.Sessions(key) {
		detail.Sessions = append(detail.Sessions, SessionView(session))
	}
	if detail.Device, failure = service.engine.Inventory.GetDeviceLifecycle(operation, key.DeviceID); failure != nil {
		return NodeDetail{}, failure
	}
	rows, failure := service.engine.Campaigns.RowsForNode(operation, key.DeviceID, key.InstallationID, 100)
	if failure != nil {
		return NodeDetail{}, failure
	}
	detail.Executions = rows
	if detail.Executions == nil {
		detail.Executions = []campaign.Row{}
	}
	return detail, nil
}

var operatorLifecycles = map[string]bool{"active": true, "quarantined": true, "retired": true, "revoked": true}

func Withdrawn(lifecycle string) bool {
	return lifecycle == "retired" || lifecycle == "revoked"
}

type LifecycleRefused struct {
	From string
	To   string
}

func (refused *LifecycleRefused) Error() string {
	if Withdrawn(refused.From) {
		return "the node is " + refused.From + ", and only the admin role changes the lifecycle of a " + refused.From + " node"
	}
	return "setting a node " + refused.To + " needs the admin role"
}

func CheckLifecycleChange(from, to string, administrator bool) error {
	if administrator {
		return nil
	}
	if Withdrawn(from) || Withdrawn(to) {
		return &LifecycleRefused{From: from, To: to}
	}
	return nil
}

func (service *Service) SetLifecycle(operation context.Context, key inventory.Key, lifecycle, reason, actor string, administrator bool) error {
	if !operatorLifecycles[lifecycle] {
		return invalidArgument("lifecycle must be active, quarantined, retired or revoked")
	}
	if strings.TrimSpace(reason) == "" {
		return invalidArgument("a reason is required")
	}
	if failure := CheckLifecycleChange("", lifecycle, administrator); failure != nil {
		return failure
	}
	now := time.Now()
	previous, failure := service.engine.Inventory.SetLifecycle(operation, key, lifecycle, reason, now, func(previous string) error {
		if failure := CheckLifecycleChange(previous, lifecycle, administrator); failure != nil {
			return failure
		}
		producing, cancel := context.WithTimeout(operation, 10*time.Second)
		defer cancel()
		if failure := service.engine.NodeState.ProduceNodeState(producing, kafka.NewInstallationState(key.DeviceID, key.InstallationID, lifecycle, &reason, actor, now)); failure != nil {
			return fmt.Errorf("the node-state record was not acknowledged: %w", failure)
		}
		return nil
	})
	if failure != nil {
		return failure
	}
	service.engine.Logger.Info("node lifecycle changed", "device_id", key.DeviceID, "installation_id", key.InstallationID, "from", previous, "lifecycle", lifecycle, "principal", actor, "reason", reason)
	return nil
}

var deviceLifecycles = map[string]bool{"active": true, "retired": true, "revoked": true}

func (service *Service) SetDeviceLifecycle(operation context.Context, device, lifecycle, reason, actor string) (inventory.DeviceLifecycle, error) {
	if !deviceLifecycles[lifecycle] {
		return inventory.DeviceLifecycle{}, invalidArgument("lifecycle must be active, retired or revoked")
	}
	if strings.TrimSpace(reason) == "" {
		return inventory.DeviceLifecycle{}, invalidArgument("a reason is required")
	}
	changed := inventory.DeviceLifecycle{DeviceID: device, Lifecycle: lifecycle, Reason: reason, ChangedAt: time.Now(), Actor: actor}
	previous, failure := service.engine.Inventory.SetDeviceLifecycle(operation, changed, func(string) error {
		producing, cancel := context.WithTimeout(operation, 10*time.Second)
		defer cancel()
		var failure error
		if lifecycle == "active" {
			failure = service.engine.NodeState.ClearNodeState(producing, "device/"+device)
		} else {
			failure = service.engine.NodeState.ProduceNodeState(producing, kafka.NewDeviceState(device, lifecycle, &reason, actor, changed.ChangedAt))
		}
		if failure != nil {
			return fmt.Errorf("the node-state record was not acknowledged: %w", failure)
		}
		return nil
	})
	if failure != nil {
		return inventory.DeviceLifecycle{}, failure
	}
	service.engine.Logger.Info("device lifecycle changed", "device_id", device, "from", previous, "lifecycle", lifecycle, "principal", actor, "reason", reason)
	return changed, nil
}

type OperatorProcess struct {
	Pid  campaign.Pid `json:"pid"`
	Node dawn.NodeRef `json:"node"`
}

func (service *Service) operatorProcess(operation context.Context, key inventory.Key, actionKind, actor string, lifetime time.Duration, maxCommands int) (OperatorProcess, error) {
	session, online := service.engine.view.Target(key)
	if !online {
		return OperatorProcess{}, ErrNodeOffline
	}
	now := time.Now()
	pid, failure := service.engine.recordRandomProcess(operation, key, actionKind, actor, actor, now.Add(lifetime), maxCommands, 0, now)
	if failure != nil {
		return OperatorProcess{}, failure
	}
	service.engine.Logger.Info("operator process intended", "pid", pid.String(), "action_kind", actionKind, "principal", actor, "device_id", key.DeviceID, "installation_id", key.InstallationID,
		"expires_at", now.Add(lifetime))
	return OperatorProcess{Pid: pid, Node: nodeReference(key, session)}, nil
}

func (service *Service) OpenSession(operation context.Context, key inventory.Key, reason string, lifetimeSeconds int, actor string) (OperatorProcess, error) {
	if strings.TrimSpace(reason) == "" {
		return OperatorProcess{}, invalidArgument("a reason is required")
	}
	if lifetimeSeconds < 60 || lifetimeSeconds > 28800 {
		return OperatorProcess{}, invalidArgument("ttl_seconds must be between 60 and 28800")
	}
	opened, failure := service.operatorProcess(operation, key, "interactive", actor, time.Duration(lifetimeSeconds)*time.Second, service.engine.Config.Reconcile.CommandsPerSession)
	if failure == nil {
		service.engine.Logger.Info("interactive session opened", "pid", opened.Pid.String(), "principal", actor, "reason", reason, "device_id", key.DeviceID, "installation_id", key.InstallationID)
	}
	return opened, failure
}

func (service *Service) StreamLogs(operation context.Context, key inventory.Key, level string, durationSeconds int, actor string) (string, error) {
	switch level {
	case "trace", "debug", "info", "warn", "error":
	default:
		return "", invalidArgument("level must be trace, debug, info, warn or error")
	}
	if durationSeconds < 1 || durationSeconds > 86400 {
		return "", invalidArgument("duration_seconds must be between 1 and 86400")
	}
	granted, failure := service.operatorProcess(operation, key, "stream_logs", actor, time.Duration(durationSeconds)*time.Second+5*time.Minute, 1)
	if failure != nil {
		return "", failure
	}
	return service.engine.Dawn.Logs(operation, granted.Node, granted.Pid, level, durationSeconds, nil)
}

func (service *Service) CollectFile(operation context.Context, key inventory.Key, path, actor string) (string, error) {
	if path == "" || len(path) > 4096 || strings.ContainsRune(path, 0) {
		return "", invalidArgument("path must be a non-empty absolute path")
	}
	granted, failure := service.operatorProcess(operation, key, "collect_file", actor, time.Duration(service.engine.Config.Engine.ProcessLifetimeSeconds)*time.Second, 1)
	if failure != nil {
		return "", failure
	}
	return service.engine.Dawn.Files(operation, granted.Node, granted.Pid, path, nil)
}

type Overview struct {
	Nodes struct {
		Total       int64            `json:"total"`
		Online      int              `json:"online"`
		ByLifecycle map[string]int64 `json:"by_lifecycle"`
	} `json:"nodes"`
	Campaigns            []CampaignSummary         `json:"campaigns"`
	Alerts               map[alerts.Severity]int64 `json:"alerts"`
	AlertsUnacknowledged map[alerts.Severity]int64 `json:"alerts_unacknowledged"`
	Degraded             bool                      `json:"degraded"`
	Leader               bool                      `json:"leader"`
}

func (service *Service) Overview(operation context.Context) (Overview, error) {
	var overview Overview
	counts, failure := service.engine.Inventory.LifecycleCounts(operation)
	if failure != nil {
		return overview, failure
	}
	overview.Nodes.ByLifecycle = counts
	for _, count := range counts {
		overview.Nodes.Total += count
	}
	overview.Nodes.Online = service.engine.view.OnlineCount()
	active, failure := service.engine.Campaigns.Active(operation)
	if failure != nil {
		return overview, failure
	}
	if overview.Campaigns, failure = service.withCounters(operation, active); failure != nil {
		return overview, failure
	}
	alertCounts, failure := service.engine.Alerts.Counts(operation)
	if failure != nil {
		return overview, failure
	}
	overview.Alerts, overview.AlertsUnacknowledged = alertCounts.Open, alertCounts.Unacknowledged
	overview.Degraded = service.engine.view.Degraded()
	overview.Leader = service.engine.leadership.Leading()
	return overview, nil
}

func (service *Service) Alerts(operation context.Context, open bool, cursor string, limit int) (alerts.Page, error) {
	page, failure := service.engine.Alerts.List(operation, open, cursor, limit)
	if errors.Is(failure, alerts.ErrMalformedCursor) {
		return page, invalidArgument("malformed cursor")
	}
	return page, failure
}

func (service *Service) Alert(operation context.Context, identifier int64) (alerts.Alert, error) {
	return service.engine.Alerts.Get(operation, identifier)
}

func (service *Service) AcknowledgeAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error) {
	return service.engine.Alerts.Acknowledge(operation, identifier, actor, time.Now())
}

func (service *Service) ResolveAlert(operation context.Context, identifier int64, actor string) (alerts.Alert, error) {
	return service.engine.Alerts.Resolve(operation, identifier, actor, time.Now())
}

func (service *Service) Subscribe(operation context.Context) <-chan FeedEvent {
	return service.engine.feed.Subscribe(operation)
}
