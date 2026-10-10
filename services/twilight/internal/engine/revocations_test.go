package engine

import (
	"errors"
	"strings"
	"testing"
	"time"
)

func TestARevocationNamesOneCredentialAndAReason(test *testing.T) {
	after := time.Date(2026, 10, 1, 2, 0, 0, 0, time.UTC)
	before := after.Add(time.Hour)
	valid := []RevocationRequest{
		{CredentialKind: "fleet_token", Credential: "retail-eu-2026", Reason: "leaked token"},
		{CredentialKind: "install_token", Credential: "factory-2026", EnrolledAfter: &after, EnrolledBefore: &before, Reason: "signing key leaked"},
		{CredentialKind: "fleet_token", Credential: "retail-eu-2026", DryRun: true},
	}
	for _, request := range valid {
		scope, failure := request.scope()
		if failure != nil || scope.Credential != request.Credential || scope.CredentialKind != request.CredentialKind {
			test.Errorf("%+v refused: %v", request, failure)
		}
	}
	invalid := []RevocationRequest{
		{CredentialKind: "certificate", Credential: "x", Reason: "r"},
		{CredentialKind: "fleet_token", Credential: " ", Reason: "r"},
		{CredentialKind: "fleet_token", Credential: strings.Repeat("a", 257), Reason: "r"},
		{CredentialKind: "fleet_token", Credential: "x", Reason: ""},
		{CredentialKind: "fleet_token", Credential: "x", Reason: "r", EnrolledAfter: &before, EnrolledBefore: &after},
	}
	for _, request := range invalid {
		if _, failure := request.scope(); !errors.Is(failure, ErrInvalidArgument) {
			test.Errorf("%+v accepted: %v", request, failure)
		}
	}
}
