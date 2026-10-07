package campaign

import (
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/selector"
)

type Status string

const (
	StatusDraft     Status = "draft"
	StatusRunning   Status = "running"
	StatusPaused    Status = "paused"
	StatusCompleted Status = "completed"
	StatusAborted   Status = "aborted"
	StatusFailed    Status = "failed"
	StatusArchived  Status = "archived"
)

func (status Status) Valid() bool {
	switch status {
	case StatusDraft, StatusRunning, StatusPaused, StatusCompleted, StatusAborted, StatusFailed, StatusArchived:
		return true
	}
	return false
}

func (status Status) Finished() bool {
	return status == StatusCompleted || status == StatusAborted || status == StatusFailed || status == StatusArchived
}

func (status Status) Active() bool {
	return status == StatusRunning || status == StatusPaused
}

type PauseKind string

const (
	PauseOperator   PauseKind = "operator"
	PauseGate       PauseKind = "gate"
	PausePermission PauseKind = "permission"
)

type Transition string

const (
	TransitionStart    Transition = "start"
	TransitionPause    Transition = "pause"
	TransitionResume   Transition = "resume"
	TransitionAbort    Transition = "abort"
	TransitionFail     Transition = "fail"
	TransitionComplete Transition = "complete"
	TransitionArchive  Transition = "archive"
)

var ErrInvalidTransition = errors.New("invalid campaign transition")
var ErrGateOverrideRequired = errors.New("the campaign was paused by a health gate; resume it with override_gate and a reason")

var transitions = map[Transition]map[Status]Status{
	TransitionStart:    {StatusDraft: StatusRunning},
	TransitionPause:    {StatusRunning: StatusPaused},
	TransitionResume:   {StatusPaused: StatusRunning},
	TransitionAbort:    {StatusDraft: StatusAborted, StatusRunning: StatusAborted, StatusPaused: StatusAborted},
	TransitionFail:     {StatusRunning: StatusFailed, StatusPaused: StatusFailed},
	TransitionComplete: {StatusRunning: StatusCompleted, StatusPaused: StatusCompleted},
	TransitionArchive:  {StatusCompleted: StatusArchived, StatusAborted: StatusArchived, StatusFailed: StatusArchived},
}

func NextStatus(current Status, transition Transition) (Status, error) {
	next, allowed := transitions[transition][current]
	if !allowed {
		return "", fmt.Errorf("%w: cannot %s a %s campaign", ErrInvalidTransition, transition, current)
	}
	return next, nil
}

type Definition struct {
	Name        string  `json:"name"`
	Description string  `json:"description"`
	Tenant      *string `json:"tenant"`
	Selector    string  `json:"selector"`
	Action      Action  `json:"action"`
	Policy      Policy  `json:"policy"`
}

func (definition *Definition) Normalize() {
	definition.Name = strings.TrimSpace(definition.Name)
	definition.Action.Normalize()
	definition.Policy.ApplyDefaults(definition.Action.Kind)
}

func (definition Definition) Validate(now time.Time) error {
	if definition.Name == "" || len(definition.Name) > 200 {
		return invalid("name", "is required and at most 200 characters")
	}
	if len(definition.Description) > 10000 {
		return invalid("description", "is at most 10000 characters")
	}
	if definition.Tenant != nil && !tenantPattern.MatchString(*definition.Tenant) {
		return invalid("tenant", "must be 1 to 63 lowercase letters, digits or dashes")
	}
	parsed, failure := selector.Parse(definition.Selector)
	if failure != nil {
		var selectorError *selector.Error
		if errors.As(failure, &selectorError) {
			return &ValidationError{Field: "selector", Message: selectorError.Message}
		}
		return invalid("selector", "%v", failure)
	}
	if parsed.MatchesAll() {
		return invalid("selector", "is empty; a campaign must say which nodes it targets, for example has(device_id) for every node")
	}
	if failure := definition.Action.Validate(); failure != nil {
		return failure
	}
	return definition.Policy.Validate(now)
}

type Campaign struct {
	ID                 uuid.UUID  `json:"id"`
	Name               string     `json:"name"`
	Description        string     `json:"description"`
	Tenant             *string    `json:"tenant"`
	Status             Status     `json:"status"`
	Kind               Kind       `json:"kind"`
	Selector           string     `json:"selector"`
	SelectorSQL        string     `json:"-"`
	SelectorParameters []string   `json:"-"`
	Action             Action     `json:"action"`
	Policy             Policy     `json:"policy"`
	Salt               []byte     `json:"-"`
	CreatedBy          string     `json:"created_by"`
	CreatedAt          time.Time  `json:"created_at"`
	UpdatedAt          time.Time  `json:"updated_at"`
	StartedAt          *time.Time `json:"started_at"`
	PausedAt           *time.Time `json:"paused_at"`
	FinishedAt         *time.Time `json:"finished_at"`
	CurrentPhase       int        `json:"current_phase"`
	PhaseStartedAt     *time.Time `json:"phase_started_at"`
	PhasePausedSeconds int64      `json:"phase_paused_seconds"`
	GateOverrideAfter  *time.Time `json:"gate_override_after"`
	PauseKind          *PauseKind `json:"pause_kind"`
	PauseReason        *string    `json:"pause_reason"`
	AbortReason        *string    `json:"abort_reason"`
	LastDispatchAt     *time.Time `json:"last_dispatch_at"`
	Version            int        `json:"version"`
	thresholds         PhaseThresholds
}

func (campaign *Campaign) Thresholds() PhaseThresholds {
	if campaign.thresholds == nil {
		return campaign.Policy.Thresholds()
	}
	return campaign.thresholds
}

func (campaign *Campaign) PhaseOf(device, installation string) (int, uint64) {
	bucket := Bucket(device, installation, campaign.Salt)
	return campaign.Thresholds().PhaseOf(bucket), bucket
}

func (campaign *Campaign) InOpenPhase(device, installation string) bool {
	return campaign.Thresholds().Open(Bucket(device, installation, campaign.Salt), campaign.CurrentPhase)
}

func (campaign *Campaign) LastPhase() bool {
	return campaign.CurrentPhase >= len(campaign.Policy.Phases)-1
}

func (campaign *Campaign) BakeAccrued(now time.Time) time.Duration {
	if campaign.PhaseStartedAt == nil {
		return 0
	}
	end := now
	if campaign.Status == StatusPaused && campaign.PausedAt != nil {
		end = *campaign.PausedAt
	}
	accrued := end.Sub(*campaign.PhaseStartedAt) - time.Duration(campaign.PhasePausedSeconds)*time.Second
	return max(0, accrued)
}

func (campaign *Campaign) BakeComplete(now time.Time) bool {
	bake := time.Duration(campaign.Policy.Phases[campaign.CurrentPhase].BakeSeconds) * time.Second
	return campaign.BakeAccrued(now) >= bake
}

func (campaign *Campaign) Subject() string {
	return "campaign:" + campaign.ID.String()
}

func (campaign *Campaign) Deadline() *time.Time {
	return campaign.Policy.Deadline
}

func (campaign *Campaign) ProcessTimeout() time.Duration {
	seconds := campaign.Policy.NodeTimeoutSeconds * (1 + len(campaign.Action.CollectFiles))
	if campaign.Action.StreamLogs != nil {
		seconds += campaign.Action.StreamLogs.DurationSeconds
	}
	return time.Duration(seconds) * time.Second
}

func (campaign *Campaign) ArchivableAt() time.Time {
	if campaign.LastDispatchAt == nil {
		return time.Time{}
	}
	return campaign.LastDispatchAt.Add(campaign.ProcessTimeout() + time.Hour)
}
