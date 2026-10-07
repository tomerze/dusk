//go:build integration

package tokens

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/google/uuid"

	"dusk/services/twilight/internal/testsupport"
)

func TestMain(suite *testing.M) {
	testsupport.Main(suite)
}

func TestTokensAuthenticateUntilRevoked(test *testing.T) {
	pool, _ := testsupport.Database(test)
	store := NewStore(pool)
	operation := context.Background()
	now := time.Now().UTC().Truncate(time.Microsecond)
	token, secret, failure := store.Create(operation, "ci", "operator", "cli", now)
	if failure != nil {
		test.Fatal(failure)
	}
	if !strings.HasPrefix(secret, prefix) || token.Role != "operator" || token.RevokedAt != nil {
		test.Fatalf("created %+v with secret %q", token, secret)
	}
	var stored []byte
	if failure := pool.QueryRow(operation, `select token_sha256 from api_tokens where id = $1`, token.ID).Scan(&stored); failure != nil || strings.Contains(string(stored), secret) {
		test.Fatalf("the secret itself was stored (%v)", failure)
	}
	found, failure := store.Authenticate(operation, secret)
	if failure != nil || found.ID != token.ID || found.Name != "ci" {
		test.Fatalf("authenticated %+v, error %v", found, failure)
	}
	if _, failure := store.Authenticate(operation, prefix+strings.Repeat("A", 43)); !errors.Is(failure, ErrInvalid) {
		test.Fatalf("an unknown token: %v", failure)
	}
	revoked, failure := store.Revoke(operation, token.ID, now.Add(time.Minute))
	if failure != nil || revoked.RevokedAt == nil {
		test.Fatalf("revoked %+v, error %v", revoked, failure)
	}
	again, failure := store.Revoke(operation, token.ID, now.Add(time.Hour))
	if failure != nil || !again.RevokedAt.Equal(*revoked.RevokedAt) {
		test.Fatalf("revoking twice moved revoked_at: %v then %v", revoked.RevokedAt, again.RevokedAt)
	}
	if _, failure := store.Authenticate(operation, secret); !errors.Is(failure, ErrInvalid) {
		test.Fatalf("a revoked token: %v", failure)
	}
	if _, failure := store.Revoke(operation, uuid.New(), now); !errors.Is(failure, ErrNotFound) {
		test.Fatalf("revoking an unknown token: %v", failure)
	}
	if _, _, failure := store.Create(operation, "ci", "root", "cli", now); failure == nil {
		test.Fatal("an unknown role was accepted")
	}
}
