package campaign

import (
	"time"

	"github.com/google/uuid"
)

type NodeState string

const (
	StatePending     NodeState = "pending"
	StateDispatching NodeState = "dispatching"
	StateDispatched  NodeState = "dispatched"
	StateDelivered   NodeState = "delivered"
	StateVerifying   NodeState = "verifying"
	StateBackoff     NodeState = "backoff"
	StateSucceeded   NodeState = "succeeded"
	StateFailed      NodeState = "failed"
	StateUnknown     NodeState = "unknown"
	StateExcluded    NodeState = "excluded"
	StateConflict    NodeState = "conflict"
	StateCancelled   NodeState = "cancelled"
)

var NodeStates = []NodeState{
	StatePending, StateDispatching, StateDispatched, StateDelivered, StateVerifying, StateBackoff,
	StateSucceeded, StateFailed, StateUnknown, StateExcluded, StateConflict, StateCancelled,
}

func (state NodeState) Valid() bool {
	for _, known := range NodeStates {
		if state == known {
			return true
		}
	}
	return false
}

func (state NodeState) Terminal() bool {
	return state == StateSucceeded || state == StateFailed || state == StateUnknown || state == StateCancelled
}

func (state NodeState) InFlight() bool {
	return state == StateDispatching || state == StateDispatched || state == StateDelivered
}

func (state NodeState) Outstanding() bool {
	return state == StatePending || state == StateBackoff || state.InFlight() || state == StateVerifying
}

type ResultStatus string

const (
	ResultStarted          ResultStatus = "started"
	ResultSucceeded        ResultStatus = "succeeded"
	ResultFailed           ResultStatus = "failed"
	ResultDuplicate        ResultStatus = "duplicate"
	ResultRunning          ResultStatus = "running"
	ResultEnded            ResultStatus = "ended"
	ResultAlreadySatisfied ResultStatus = "already_satisfied"
	ResultUnreachable      ResultStatus = "unreachable"
	ResultTimedOut         ResultStatus = "timed_out"
	ResultDenied           ResultStatus = "denied"
	ResultReaped           ResultStatus = "reaped"
	ResultError            ResultStatus = "error"
)

func (status ResultStatus) Final() bool {
	return status != ResultStarted
}

type Row struct {
	CampaignID     uuid.UUID         `json:"campaign_id"`
	DeviceID       string            `json:"device_id"`
	InstallationID string            `json:"installation_id"`
	Phase          int               `json:"phase"`
	State          NodeState         `json:"state"`
	Attempt        int               `json:"attempt"`
	Failures       int               `json:"failures"`
	Unreached      int               `json:"unreached"`
	Pid            Pid               `json:"pid"`
	Epoch          int64             `json:"epoch"`
	NamespaceID    string            `json:"namespace_id"`
	DispatchedAt   *time.Time        `json:"dispatched_at"`
	DeliveredAt    *time.Time        `json:"delivered_at"`
	DeadlineAt     *time.Time        `json:"deadline_at"`
	FinishedAt     *time.Time        `json:"finished_at"`
	NextAttemptAt  *time.Time        `json:"next_attempt_at"`
	LastError      string            `json:"last_error"`
	LastStatus     string            `json:"last_status"`
	EventAt        *time.Time        `json:"event_at"`
	BackAt         *time.Time        `json:"back_at"`
	Silent         *bool             `json:"silent"`
	ReapedAt       *time.Time        `json:"reaped_at"`
	Breakdown      map[string]string `json:"breakdown"`
	Revision       int64             `json:"-"`
}

type Result struct {
	Pid       Pid
	Status    ResultStatus
	Delivered bool
	Error     string
	Satisfied *bool
}

type Effect int

const (
	EffectNone Effect = iota
	EffectPauseForDenied
)

type Session struct {
	NamespaceID string
	Epoch       int64
}

const verificationFailure = "the reported state still differs from the desired state after the process succeeded"

func NewRow(campaign uuid.UUID, device, installation string, phase int, state NodeState) Row {
	return Row{CampaignID: campaign, DeviceID: device, InstallationID: installation, Phase: phase, State: state, Attempt: 1, Breakdown: map[string]string{}}
}

func timestamp(at time.Time) *time.Time {
	return &at
}

func (row Row) Due(now time.Time) bool {
	switch row.State {
	case StatePending:
		return row.NextAttemptAt == nil || !row.NextAttemptAt.After(now)
	case StateBackoff:
		return row.NextAttemptAt != nil && !row.NextAttemptAt.After(now)
	}
	return false
}

func (row Row) DueOnNewSession(session Session, now time.Time) bool {
	if row.State == StatePending && row.Epoch != 0 && session.Epoch > row.Epoch {
		return true
	}
	return row.Due(now)
}

func (row Row) Dispatch(campaign uuid.UUID, session Session, now, deadline time.Time, breakdown map[string]string) (Row, bool) {
	if row.State != StatePending && row.State != StateBackoff {
		return row, false
	}
	resend := row.Pid != 0
	if !resend {
		row.Pid = DerivePid(campaign, row.DeviceID, row.InstallationID, row.Attempt)
	}
	row.State = StateDispatching
	row.Epoch = session.Epoch
	row.NamespaceID = session.NamespaceID
	row.DispatchedAt = timestamp(now)
	row.DeliveredAt = nil
	row.DeadlineAt = timestamp(deadline)
	row.NextAttemptAt = nil
	if len(row.Breakdown) == 0 && len(breakdown) > 0 {
		row.Breakdown = breakdown
	}
	return row, resend
}

func (row Row) Accepted(pid Pid) (Row, bool) {
	if row.State != StateDispatching || row.Pid != pid {
		return row, false
	}
	row.State = StateDispatched
	return row, true
}

func (row Row) Undelivered(status, reason string, now time.Time, delay time.Duration) (Row, bool) {
	if !row.State.InFlight() {
		return row, false
	}
	row.State = StatePending
	row.Unreached++
	row.NextAttemptAt = timestamp(now.Add(delay))
	row.LastStatus = status
	row.LastError = reason
	row.DeliveredAt = nil
	row.DeadlineAt = nil
	return row, true
}

func (row Row) succeed(now time.Time, status string) Row {
	row.State = StateSucceeded
	row.FinishedAt = timestamp(now)
	row.NextAttemptAt = nil
	row.DeadlineAt = nil
	row.LastStatus = status
	row.EventAt = nil
	row.BackAt = nil
	row.Silent = nil
	return row
}

func (row Row) fail(policy Policy, status, reason string, now time.Time, random float64) Row {
	row.Failures++
	row.Unreached = 0
	row.LastStatus = status
	row.LastError = reason
	row.DeadlineAt = nil
	if row.Failures < policy.Retry.MaxAttempts {
		row.State = StateBackoff
		row.Attempt++
		row.Pid = 0
		row.NextAttemptAt = timestamp(now.Add(policy.Backoff(row.Failures, random)))
		return row
	}
	row.State = StateFailed
	row.FinishedAt = timestamp(now)
	row.NextAttemptAt = nil
	return row
}

func (row Row) accepts(pid Pid) bool {
	if row.Pid == 0 || row.Pid != pid {
		return false
	}
	return row.State.InFlight() || row.State == StatePending || row.State == StateUnknown
}

func (row Row) Settle(campaign *Campaign, now time.Time) Row {
	if !campaign.Status.Finished() {
		return row
	}
	switch row.State {
	case StatePending, StateBackoff, StateVerifying:
		row.State = StateCancelled
		row.FinishedAt = timestamp(now)
		row.NextAttemptAt = nil
		row.DeadlineAt = nil
	}
	return row
}

func (row Row) ApplyResult(campaign *Campaign, result Result, now time.Time, random float64) (Row, Effect, bool) {
	next, effect, changed := row.applyResult(campaign, result, now, random)
	if changed {
		next = next.Settle(campaign, now)
	}
	return next, effect, changed
}

func (row Row) applyResult(campaign *Campaign, result Result, now time.Time, random float64) (Row, Effect, bool) {
	if result.Status == ResultReaped || !row.accepts(result.Pid) {
		return row, EffectNone, false
	}
	kind, action, policy := campaign.Kind, campaign.Action, campaign.Policy
	nodeTimeout := campaign.ProcessTimeout()
	if kind.Converging() && result.Status != ResultDenied && result.Satisfied != nil && *result.Satisfied {
		return row.succeed(now, string(result.Status)), EffectNone, true
	}
	switch {
	case result.Status == ResultStarted:
		if row.State == StateDelivered {
			return row, EffectNone, false
		}
		row.State = StateDelivered
		row.DeliveredAt = timestamp(now)
		row.Unreached = 0
		if row.DeadlineAt == nil || row.DeadlineAt.Before(now.Add(nodeTimeout)) {
			row.DeadlineAt = timestamp(now.Add(nodeTimeout))
		}
		row.NextAttemptAt = nil
		row.FinishedAt = nil
		return row, EffectNone, true
	case result.Status == ResultRunning:
		return row.running(string(result.Status), now, nodeTimeout), EffectNone, true
	case result.Status == ResultDuplicate && kind.Converging() && result.Satisfied != nil:
		return row.awaitVerification(string(result.Status), ""), EffectNone, true
	case result.Status == ResultDuplicate:
		return row.running(string(result.Status), now, nodeTimeout), EffectNone, true
	case result.Status == ResultAlreadySatisfied:
		return row.succeed(now, string(result.Status)), EffectNone, true
	case result.Status == ResultDenied:
		row.State = StateFailed
		row.FinishedAt = timestamp(now)
		row.LastStatus = string(result.Status)
		row.LastError = result.Error
		row.DeadlineAt = nil
		row.NextAttemptAt = nil
		return row, EffectPauseForDenied, true
	case result.Status == ResultUnreachable || ((result.Status == ResultTimedOut || result.Status == ResultError) && !result.Delivered):
		if !row.State.InFlight() || row.State == StateDelivered || row.DeliveredAt != nil {
			return row, EffectNone, false
		}
		updated, changed := row.Undelivered(string(result.Status), result.Error, now, policy.Backoff(row.Unreached+1, random))
		return updated, EffectNone, changed
	case result.Status == ResultEnded && kind.Converging():
		return row.awaitVerification(string(result.Status), result.Error), EffectNone, true
	case result.Status == ResultTimedOut || result.Status == ResultEnded:
		return row.deliveredWithoutResult(campaign, string(result.Status), result.Error, now), EffectNone, true
	case result.Status == ResultSucceeded:
		if kind == KindQuarantine || kind.Converging() {
			return row.awaitVerification(string(result.Status), ""), EffectNone, true
		}
		return row.succeed(now, string(result.Status)), EffectNone, true
	case result.Status == ResultFailed || result.Status == ResultError:
		if kind == KindQuarantine && !action.RequireScriptSuccess {
			return row.awaitVerification(string(result.Status), result.Error), EffectNone, true
		}
		return row.fail(policy, string(result.Status), result.Error, now, random), EffectNone, true
	}
	return row, EffectNone, false
}

func (row Row) running(status string, now time.Time, nodeTimeout time.Duration) Row {
	row.State = StateDelivered
	row.Unreached = 0
	if row.DeliveredAt == nil {
		row.DeliveredAt = timestamp(now)
	}
	if row.DeadlineAt == nil || row.DeadlineAt.Before(now.Add(nodeTimeout)) {
		row.DeadlineAt = timestamp(now.Add(nodeTimeout))
	}
	row.NextAttemptAt = nil
	row.FinishedAt = nil
	row.LastStatus = status
	return row
}

func (row Row) awaitVerification(status, reason string) Row {
	row.State = StateVerifying
	row.LastStatus = status
	row.LastError = reason
	row.DeadlineAt = nil
	row.NextAttemptAt = nil
	return row
}

func (row Row) deliveredWithoutResult(campaign *Campaign, status, reason string, now time.Time) Row {
	row.LastStatus = status
	row.LastError = reason
	row.DeadlineAt = nil
	if campaign.Kind == KindQuarantine && !campaign.Action.RequireScriptSuccess {
		return row.awaitVerification(status, reason)
	}
	if campaign.Kind.OneShot() {
		row.State = StateUnknown
		row.FinishedAt = timestamp(now)
		return row
	}
	row.State = StatePending
	row.NextAttemptAt = timestamp(now)
	return row
}

func (row Row) Expire(campaign *Campaign, now time.Time, delay time.Duration) (Row, bool) {
	if !row.State.InFlight() || row.DeadlineAt == nil || row.DeadlineAt.After(now) {
		return row, false
	}
	if row.State == StateDelivered {
		return row.deliveredWithoutResult(campaign, "deadline", "no result before the deadline", now).Settle(campaign, now), true
	}
	expired, changed := row.Undelivered("deadline", "dawn did not report delivery before the deadline", now, delay)
	return expired.Settle(campaign, now), changed
}

func (row Row) Verify(satisfied bool, factsNamespace string, policy Policy, now time.Time, random float64) (Row, bool) {
	if row.State != StateVerifying {
		return row, false
	}
	if satisfied {
		return row.succeed(now, row.LastStatus), true
	}
	if factsNamespace == "" || factsNamespace == row.NamespaceID {
		return row, false
	}
	return row.fail(policy, string(ResultFailed), verificationFailure, now, random), true
}

func (row Row) Satisfied(now time.Time) (Row, bool) {
	if row.State.Terminal() {
		return row, false
	}
	return row.succeed(now, string(ResultAlreadySatisfied)), true
}

func (row Row) Drifted() (Row, bool) {
	if row.State != StateSucceeded {
		return row, false
	}
	row.State = StatePending
	row.Attempt++
	row.Failures = 0
	row.Unreached = 0
	row.Pid = 0
	row.FinishedAt = nil
	row.NextAttemptAt = nil
	row.LastStatus = ""
	row.LastError = "the reported state drifted from the desired state"
	return row, true
}

func (row Row) Exclude() (Row, bool) {
	if row.State != StatePending && row.State != StateBackoff {
		return row, false
	}
	row.State = StateExcluded
	return row, true
}

func (row Row) Include() (Row, bool) {
	if row.State != StateExcluded {
		return row, false
	}
	row.State = StatePending
	return row, true
}

func (row Row) Conflict() (Row, bool) {
	if row.State != StatePending && row.State != StateBackoff && row.State != StateExcluded {
		return row, false
	}
	row.State = StateConflict
	return row, true
}

func (row Row) Unconflict() (Row, bool) {
	if row.State != StateConflict {
		return row, false
	}
	row.State = StatePending
	return row, true
}

func (row Row) Cancel(now time.Time) (Row, bool) {
	switch row.State {
	case StatePending, StateBackoff, StateExcluded, StateConflict:
		row.State = StateCancelled
		row.FinishedAt = timestamp(now)
		row.NextAttemptAt = nil
		return row, true
	}
	return row, false
}

func (row Row) Retry(now time.Time) (Row, bool) {
	switch row.State {
	case StateFailed, StateUnknown, StateCancelled:
	default:
		return row, false
	}
	row.State = StatePending
	row.Attempt++
	row.Failures = 0
	row.Unreached = 0
	row.Pid = 0
	row.FinishedAt = nil
	row.NextAttemptAt = timestamp(now)
	row.DeadlineAt = nil
	return row, true
}

func (row Row) Resolve(campaign *Campaign, succeeded bool, reason string, now time.Time) (Row, bool) {
	if row.State != StateUnknown {
		return row, false
	}
	switch {
	case succeeded && campaign.Kind == KindQuarantine:
		row.FinishedAt = nil
		return row.awaitVerification("resolved", reason).Settle(campaign, now), true
	case succeeded:
		row = row.succeed(now, "resolved")
	default:
		row.State = StateFailed
		row.LastStatus = "resolved"
		row.FinishedAt = timestamp(now)
	}
	row.LastError = reason
	return row, true
}

func (row Row) QuarantineWithoutScript(now time.Time) (Row, bool) {
	if !row.Due(now) {
		return row, false
	}
	return row.awaitVerification("flip", ""), true
}

func (row Row) Withdrawn(now time.Time, lifecycle string) (Row, bool) {
	if row.State != StateVerifying {
		return row, false
	}
	row = row.succeed(now, "withdrawn")
	row.LastError = "the node is " + lifecycle + "; quarantine leaves its lifecycle as it is"
	return row, true
}

func (row Row) Flipped(now time.Time) (Row, bool) {
	if row.State != StateVerifying {
		return row, false
	}
	return row.succeed(now, row.LastStatus), true
}

func (row Row) Disconnected(at time.Time, epoch int64) (Row, bool) {
	if row.State != StateSucceeded || row.EventAt != nil || epoch < row.Epoch || row.FinishedAt == nil || at.Before(*row.FinishedAt) {
		return row, false
	}
	row.EventAt = timestamp(at)
	return row, true
}

func (row Row) Reconnected(at time.Time, window time.Duration) (Row, bool) {
	if row.State != StateSucceeded || row.EventAt == nil || row.BackAt != nil {
		return row, false
	}
	row.BackAt = timestamp(at)
	silent := at.Sub(*row.EventAt) > window
	row.Silent = &silent
	return row, true
}

func (row Row) Lapsed(now time.Time, window time.Duration) (Row, bool) {
	if row.State != StateSucceeded || row.EventAt == nil || row.BackAt != nil || row.Silent != nil || now.Sub(*row.EventAt) <= window {
		return row, false
	}
	silent := true
	row.Silent = &silent
	return row, true
}
