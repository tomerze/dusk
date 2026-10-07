package tokens

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

const prefix = "twilight_"

var Roles = []string{"viewer", "operator", "admin"}

var (
	ErrNotFound = errors.New("no such API token")
	ErrInvalid  = errors.New("the API token is unknown or revoked")
)

type Token struct {
	ID        uuid.UUID  `json:"id"`
	Name      string     `json:"name"`
	Role      string     `json:"role"`
	CreatedBy string     `json:"created_by"`
	CreatedAt time.Time  `json:"created_at"`
	RevokedAt *time.Time `json:"revoked_at"`
}

type Store struct {
	pool *pgxpool.Pool
}

func NewStore(pool *pgxpool.Pool) *Store {
	return &Store{pool: pool}
}

func digest(secret string) []byte {
	sum := sha256.Sum256([]byte(secret))
	return sum[:]
}

func wellFormed(secret string) bool {
	encoded, found := strings.CutPrefix(secret, prefix)
	if !found {
		return false
	}
	decoded, failure := base64.RawURLEncoding.DecodeString(encoded)
	return failure == nil && len(decoded) == 32
}

func Validate(name, role string) error {
	if strings.TrimSpace(name) == "" || utf8.RuneCountInString(name) > 200 {
		return errors.New("a token name holds 1 to 200 characters")
	}
	for _, known := range Roles {
		if role == known {
			return nil
		}
	}
	return fmt.Errorf("a token role is one of %s", strings.Join(Roles, ", "))
}

const columns = `id, name, role, created_by, created_at, revoked_at`

func scan(row pgx.Row) (Token, error) {
	var token Token
	failure := row.Scan(&token.ID, &token.Name, &token.Role, &token.CreatedBy, &token.CreatedAt, &token.RevokedAt)
	if errors.Is(failure, pgx.ErrNoRows) {
		return token, ErrNotFound
	}
	return token, failure
}

func (store *Store) Create(operation context.Context, name, role, createdBy string, now time.Time) (Token, string, error) {
	if failure := Validate(name, role); failure != nil {
		return Token{}, "", failure
	}
	material := make([]byte, 32)
	if _, failure := rand.Read(material); failure != nil {
		return Token{}, "", fmt.Errorf("draw a token: %w", failure)
	}
	secret := prefix + base64.RawURLEncoding.EncodeToString(material)
	identifier, failure := uuid.NewV7()
	if failure != nil {
		return Token{}, "", failure
	}
	token, failure := scan(store.pool.QueryRow(operation, `insert into api_tokens (id, name, token_sha256, role, created_by, created_at) values ($1, $2, $3, $4, $5, $6) returning `+columns,
		identifier, name, digest(secret), role, createdBy, now))
	if failure != nil {
		return Token{}, "", fmt.Errorf("store the token: %w", failure)
	}
	return token, secret, nil
}

func (store *Store) Revoke(operation context.Context, identifier uuid.UUID, now time.Time) (Token, error) {
	return scan(store.pool.QueryRow(operation, `update api_tokens set revoked_at = coalesce(revoked_at, $2) where id = $1 returning `+columns, identifier, now))
}

func (store *Store) Authenticate(operation context.Context, secret string) (Token, error) {
	if !wellFormed(secret) {
		return Token{}, ErrInvalid
	}
	token, failure := scan(store.pool.QueryRow(operation, `select `+columns+` from api_tokens where token_sha256 = $1`, digest(secret)))
	switch {
	case errors.Is(failure, ErrNotFound):
		return Token{}, ErrInvalid
	case failure != nil:
		return Token{}, failure
	case token.RevokedAt != nil:
		return Token{}, ErrInvalid
	}
	return token, nil
}
