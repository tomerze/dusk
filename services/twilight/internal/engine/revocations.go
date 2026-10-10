package engine

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/inventory"
	"dusk/services/twilight/internal/kafka"
)

const revocationBatch = 500

type RevocationRequest struct {
	CredentialKind string     `json:"credential_kind"`
	Credential     string     `json:"credential"`
	EnrolledAfter  *time.Time `json:"enrolled_after"`
	EnrolledBefore *time.Time `json:"enrolled_before"`
	Reason         string     `json:"reason"`
	DryRun         bool       `json:"dry_run"`
}

type RevocationStarted struct {
	Matched    int64                 `json:"matched"`
	Revocation *inventory.Revocation `json:"revocation"`
}

func (request RevocationRequest) scope() (inventory.RevocationScope, error) {
	scope := inventory.RevocationScope{CredentialKind: request.CredentialKind, Credential: request.Credential,
		EnrolledAfter: request.EnrolledAfter, EnrolledBefore: request.EnrolledBefore}
	if request.CredentialKind != "fleet_token" && request.CredentialKind != "install_token" {
		return scope, invalidArgument("credential_kind must be fleet_token or install_token")
	}
	if strings.TrimSpace(request.Credential) == "" || len(request.Credential) > 256 {
		return scope, invalidArgument("credential must name a fleet token entry or an install token key in 1 to 256 bytes")
	}
	if request.EnrolledAfter != nil && request.EnrolledBefore != nil && !request.EnrolledAfter.Before(*request.EnrolledBefore) {
		return scope, invalidArgument("enrolled_after must be before enrolled_before")
	}
	if !request.DryRun && strings.TrimSpace(request.Reason) == "" {
		return scope, invalidArgument("a reason is required")
	}
	return scope, nil
}

func (service *Service) StartRevocation(operation context.Context, request RevocationRequest, actor string) (RevocationStarted, error) {
	scope, failure := request.scope()
	if failure != nil {
		return RevocationStarted{}, failure
	}
	if request.DryRun {
		matched, failure := service.engine.Inventory.CountRevocable(operation, scope)
		return RevocationStarted{Matched: matched}, failure
	}
	revocation, failure := service.engine.Inventory.CreateRevocation(operation, scope, request.Reason, actor, time.Now())
	if failure != nil {
		return RevocationStarted{}, failure
	}
	service.engine.Logger.Info("revocation started", "revocation_id", revocation.ID, "credential_kind", scope.CredentialKind, "credential", scope.Credential,
		"enrolled_after", scope.EnrolledAfter, "enrolled_before", scope.EnrolledBefore, "matched", revocation.Matched, "principal", actor, "reason", request.Reason)
	return RevocationStarted{Matched: revocation.Matched, Revocation: &revocation}, nil
}

func (service *Service) Revocation(operation context.Context, identifier uuid.UUID) (inventory.Revocation, error) {
	return service.engine.Inventory.GetRevocation(operation, identifier)
}

func (service *Service) Revocations(operation context.Context, cursor string, limit int) (inventory.RevocationPage, error) {
	page, failure := service.engine.Inventory.ListRevocations(operation, cursor, limit)
	if errors.Is(failure, inventory.ErrMalformedCursor) {
		return page, invalidArgument("malformed cursor")
	}
	return page, failure
}

func (engine *Engine) runRevocations(operation context.Context, term int64) {
	rate := engine.Config.Engine.RevocationsPerSecond
	for sleep(operation, time.Second) {
		running, failure := engine.Inventory.RunningRevocations(operation)
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("reading the running revocations failed", "error", failure)
			}
			continue
		}
		budget := rate
		for index := 0; index < len(running) && budget > 0; index++ {
			done, stop := engine.revokeSome(operation, term, &running[index], budget)
			budget -= done
			if stop {
				break
			}
		}
	}
}

func (engine *Engine) revokeSome(operation context.Context, term int64, revocation *inventory.Revocation, budget int) (int, bool) {
	done := 0
	for done < budget {
		keys, failure := engine.Inventory.RevocationBatch(operation, *revocation, min(budget-done, revocationBatch))
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("reading the nodes of a revocation failed; it continues on the next round", "revocation_id", revocation.ID, "error", failure)
			}
			return done, true
		}
		if len(keys) == 0 {
			now := time.Now()
			if failure := engine.Inventory.FinishRevocation(operation, revocation, now); failure != nil {
				if operation.Err() == nil {
					engine.Logger.Warn("a finished revocation was not marked finished; it is checked again on the next round", "revocation_id", revocation.ID, "error", failure)
				}
				return done, true
			}
			engine.Logger.Info("revocation finished", "revocation_id", revocation.ID, "credential_kind", revocation.CredentialKind, "credential", revocation.Credential,
				"matched", revocation.Matched, "revoked", revocation.Revoked, "seconds", now.Sub(revocation.CreatedAt).Seconds())
			return done, false
		}
		now := time.Now()
		states := make([]kafka.NodeState, len(keys))
		for position, key := range keys {
			states[position] = kafka.NewInstallationState(key.DeviceID, key.InstallationID, "revoked", &revocation.Reason, revocation.Actor, now)
		}
		producing, cancel := context.WithTimeout(operation, 30*time.Second)
		failure = engine.NodeState.ProduceNodeStates(producing, states)
		cancel()
		if failure != nil {
			if operation.Err() == nil {
				engine.Logger.Warn("the node-state records of a revocation were not acknowledged; the batch is sent again on the next round",
					"revocation_id", revocation.ID, "nodes", len(keys), "error", failure)
			}
			return done, true
		}
		if failure := engine.Inventory.RevokeBatch(operation, revocation, keys, now, term); failure != nil {
			if !errors.Is(failure, inventory.ErrNotLeading) && operation.Err() == nil {
				engine.Logger.Warn("a revocation batch was not recorded; it is sent again on the next round", "revocation_id", revocation.ID, "nodes", len(keys), "error", failure)
			}
			return done, true
		}
		done += len(keys)
		engine.Logger.Debug("revocation progressed", "revocation_id", revocation.ID, "revoked", revocation.Revoked, "matched", revocation.Matched)
	}
	return done, false
}
