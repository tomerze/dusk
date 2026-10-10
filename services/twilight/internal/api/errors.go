package api

import (
	"context"
	"errors"
	"net/http"
	"strconv"
	"strings"

	"dusk/services/twilight/internal/alerts"
	"dusk/services/twilight/internal/campaign"
	"dusk/services/twilight/internal/dawn"
	"dusk/services/twilight/internal/engine"
	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/selector"
)

type Problem struct {
	Status  int            `json:"-"`
	Code    string         `json:"code"`
	Message string         `json:"message"`
	Details map[string]any `json:"details"`
	cause   error
}

func (problem *Problem) Error() string {
	return problem.Code + ": " + problem.Message
}

func (problem *Problem) Unwrap() error {
	return problem.cause
}

func newProblem(status int, code, message string) *Problem {
	return &Problem{Status: status, Code: code, Message: message, Details: map[string]any{}}
}

func (problem *Problem) with(key string, value any) *Problem {
	problem.Details[key] = value
	return problem
}

func invalid(message string) *Problem {
	return newProblem(http.StatusBadRequest, "invalid_argument", message)
}

func classify(failure error) *Problem {
	var problem *Problem
	var validation *campaign.ValidationError
	var selectorError *selector.Error
	var overlap *engine.OverlapError
	var dawnError *dawn.Error
	switch {
	case errors.As(failure, &problem):
		return problem
	case errors.As(failure, &validation):
		return newProblem(http.StatusBadRequest, "invalid_argument", validation.Error()).with("field", validation.Field)
	case errors.As(failure, &selectorError):
		return newProblem(http.StatusBadRequest, "invalid_selector", selectorError.Message).with("position", selectorError.Position).with("end", selectorError.End)
	case errors.Is(failure, engine.ErrInvalidArgument):
		return newProblem(http.StatusBadRequest, "invalid_argument", strings.TrimPrefix(failure.Error(), engine.ErrInvalidArgument.Error()+": "))
	case errors.Is(failure, campaign.ErrNotFound), errors.Is(failure, campaign.ErrRowNotFound), errors.Is(failure, inventory.ErrNotFound), errors.Is(failure, alerts.ErrNotFound),
		errors.Is(failure, inventory.ErrRevocationNotFound):
		return newProblem(http.StatusNotFound, "not_found", failure.Error())
	case errors.As(failure, &overlap):
		campaigns := make([]string, len(overlap.Campaigns))
		for index, identifier := range overlap.Campaigns {
			campaigns[index] = identifier.String()
		}
		return newProblem(http.StatusConflict, "overlap", overlap.Error()).with("count", overlap.Count).with("campaigns", campaigns)
	case errors.Is(failure, campaign.ErrGateOverrideRequired):
		return newProblem(http.StatusConflict, "gate_override_required", failure.Error())
	case errors.Is(failure, campaign.ErrVersionConflict):
		return newProblem(http.StatusConflict, "version_conflict", failure.Error())
	case errors.Is(failure, campaign.ErrNotDraft):
		return newProblem(http.StatusConflict, "not_draft", failure.Error())
	case errors.Is(failure, campaign.ErrNotArchivable):
		return newProblem(http.StatusConflict, "not_archivable", failure.Error())
	case errors.Is(failure, campaign.ErrInvalidTransition):
		return newProblem(http.StatusConflict, "invalid_transition", failure.Error())
	case errors.Is(failure, engine.ErrNodeOffline):
		return newProblem(http.StatusConflict, "node_offline", failure.Error())
	case errors.As(failure, &dawnError):
		switch dawnError.Outcome {
		case dawn.Busy:
			return newProblem(http.StatusServiceUnavailable, "dawn_busy", "dawn is busy with this node; try again shortly").with("retry_after_seconds", 5)
		case dawn.NotDelivered:
			return newProblem(http.StatusServiceUnavailable, "dawn_unavailable", "no dawn instance could be reached").with("retry_after_seconds", 5)
		case dawn.Rejected:
			return newProblem(http.StatusBadGateway, "dawn_rejected", "dawn refused the request").with("dawn_status", dawnError.Status).with("dawn_message", dawnError.Message)
		default:
			return newProblem(http.StatusGatewayTimeout, "dawn_unanswered", "dawn did not answer; the request may or may not have reached the node")
		}
	case errors.Is(failure, context.DeadlineExceeded):
		return newProblem(http.StatusServiceUnavailable, "timeout", "the request did not finish in time; try again shortly").with("retry_after_seconds", 5)
	}
	return &Problem{Status: http.StatusInternalServerError, Code: "internal", Message: "internal error", Details: map[string]any{}, cause: failure}
}

func retryAfter(problem *Problem) string {
	if seconds, found := problem.Details["retry_after_seconds"].(int); found {
		return strconv.Itoa(seconds)
	}
	return ""
}
