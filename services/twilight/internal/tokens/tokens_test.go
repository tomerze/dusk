package tokens

import (
	"strings"
	"testing"
)

func TestValidateNamesAndRoles(test *testing.T) {
	for _, role := range Roles {
		if failure := Validate("ci", role); failure != nil {
			test.Errorf("%s refused: %v", role, failure)
		}
	}
	for name, role := range map[string]string{"": "admin", "   ": "viewer", strings.Repeat("n", 201): "viewer", "ci": "root"} {
		if Validate(name, role) == nil {
			test.Errorf("name %q role %q accepted", name, role)
		}
	}
}

func TestOnlyWellFormedSecretsReachTheDatabase(test *testing.T) {
	for _, secret := range []string{"", "twilight_", "Bearer twilight_abc", "twilight_" + strings.Repeat("A", 42), "other_" + strings.Repeat("A", 43)} {
		if wellFormed(secret) {
			test.Errorf("%q is well formed", secret)
		}
	}
	if !wellFormed("twilight_" + strings.Repeat("A", 43)) {
		test.Error("a 32-byte secret is not well formed")
	}
}
