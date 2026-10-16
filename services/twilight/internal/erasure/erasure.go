package erasure

import (
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
)

type Scope string

const (
	ScopeDevice Scope = "device"
	ScopeTenant Scope = "tenant"

	StatusRunning   = "running"
	StatusCompleted = "completed"
)

type StepState string

const (
	StepPending StepState = "pending"
	StepRunning StepState = "running"
	StepDone    StepState = "done"
)

const (
	StepScope      = "scope"
	StepClickHouse = "clickhouse"
	StepLake       = "lake"
	StepFiles      = "files"
	StepInventory  = "inventory"
)

type Step struct {
	Name       string           `json:"name"`
	State      StepState        `json:"state"`
	Detail     string           `json:"detail"`
	Counts     map[string]int64 `json:"counts"`
	Issued     []string         `json:"issued,omitempty"`
	Cursor     string           `json:"cursor,omitempty"`
	StartedAt  *time.Time       `json:"started_at"`
	FinishedAt *time.Time       `json:"finished_at"`
}

type Retained struct {
	Store string     `json:"store"`
	Data  string     `json:"data"`
	Until *time.Time `json:"until"`
	Why   string     `json:"why"`
}

type Erasure struct {
	ID            uuid.UUID  `json:"id"`
	Scope         Scope      `json:"scope"`
	DeviceID      *string    `json:"device_id"`
	Tenant        *string    `json:"tenant"`
	Reason        string     `json:"reason"`
	RequestedBy   string     `json:"requested_by"`
	RequestedAt   time.Time  `json:"requested_at"`
	Status        string     `json:"status"`
	Steps         []Step     `json:"steps"`
	Retained      []Retained `json:"retained"`
	Attempts      int        `json:"attempts"`
	LastError     *string    `json:"last_error"`
	NextAttemptAt time.Time  `json:"-"`
	CompletedAt   *time.Time `json:"completed_at"`
}

func (erasure *Erasure) Subject() string {
	if erasure.DeviceID != nil {
		return "device " + *erasure.DeviceID
	}
	if erasure.Tenant != nil {
		return "tenant " + *erasure.Tenant
	}
	return "nothing"
}

func newSteps() []Step {
	names := []string{StepScope, StepClickHouse, StepLake, StepFiles, StepInventory}
	steps := make([]Step, len(names))
	for index, name := range names {
		steps[index] = Step{Name: name, State: StepPending, Counts: map[string]int64{}}
	}
	return steps
}

var (
	ErrNotFound      = errors.New("erasure not found")
	ErrInvalidReason = errors.New("a reason of 1 to 1000 characters is required")
)

type NotReadyError struct {
	Subject          string
	NotWithdrawn     int
	Online           int
	RunningCampaigns int
}

func (failure *NotReadyError) Error() string {
	var problems []string
	if failure.NotWithdrawn > 0 {
		problems = append(problems, fmt.Sprintf("%d of its installations are neither retired nor revoked", failure.NotWithdrawn))
	}
	if failure.Online > 0 {
		problems = append(problems, fmt.Sprintf("%d of its installations are online", failure.Online))
	}
	if failure.RunningCampaigns > 0 {
		problems = append(problems, fmt.Sprintf("%d of its campaigns are running or paused", failure.RunningCampaigns))
	}
	return failure.Subject + " cannot be erased yet: " + strings.Join(problems, ", ") + "; retire or revoke it and finish its campaigns first, so that nothing new about it is stored while it is erased"
}

type table struct {
	name      string
	telemetry bool
}

var erasedTables = []table{
	{"connections", false},
	{"enrollments", false},
	{"process_results", false},
	{"process_output", false},
	{"files", false},
	{"otel_logs", true},
	{"otel_spans", true},
}

func scopeSet(erasure uuid.UUID, kind string) string {
	return fmt.Sprintf("(SELECT value FROM dusk.erasure_scope WHERE erasure_id = '%s' AND kind = '%s')", erasure, kind)
}

func predicate(current table, erasure uuid.UUID) string {
	devices := scopeSet(erasure, "device")
	condition := "device_id IN " + devices
	if current.telemetry {
		condition = "(attributes['device_id'] IN " + devices + " OR resource_attributes['host.id'] IN " + scopeSet(erasure, "host") + ")"
	}
	return condition
}

func hostOf(namespace string) string {
	trimmed := strings.TrimLeft(strings.ToLower(namespace), "0")
	if trimmed == "" {
		return "0"
	}
	return trimmed
}
