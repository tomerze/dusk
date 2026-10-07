package api

import (
	"context"
	"crypto/rand"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

var ErrSessionNotFound = errors.New("no such session")

type Session struct {
	Digest         []byte
	CSRFDigest     []byte
	Subject        string
	Name           *string
	Role           Role
	Authentication string
	CreatedAt      time.Time
	LastSeenAt     time.Time
	ExpiresAt      time.Time
}

type SessionStore interface {
	CreateSession(operation context.Context, session Session, idleSince time.Time) error
	Session(operation context.Context, digest []byte) (Session, error)
	TouchSession(operation context.Context, digest []byte, at time.Time) error
	DeleteSession(operation context.Context, digest []byte) error
	LoginKey(operation context.Context) ([]byte, error)
}

type PostgresSessions struct {
	pool *pgxpool.Pool
}

func NewPostgresSessions(pool *pgxpool.Pool) *PostgresSessions {
	return &PostgresSessions{pool: pool}
}

func (store *PostgresSessions) CreateSession(operation context.Context, session Session, idleSince time.Time) error {
	return pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `delete from api_sessions where expires_at <= $1 or last_seen_at < $2`, session.CreatedAt, idleSince); failure != nil {
			return failure
		}
		_, failure := transaction.Exec(operation, `insert into api_sessions (id_sha256, csrf_sha256, subject, name, role, authentication, created_at, last_seen_at, expires_at)
			values ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
			session.Digest, session.CSRFDigest, session.Subject, session.Name, session.Role.String(), session.Authentication, session.CreatedAt, session.LastSeenAt, session.ExpiresAt)
		return failure
	})
}

func (store *PostgresSessions) Session(operation context.Context, digest []byte) (Session, error) {
	session := Session{Digest: digest}
	var role string
	failure := store.pool.QueryRow(operation, `select csrf_sha256, subject, name, role, authentication, created_at, last_seen_at, expires_at from api_sessions where id_sha256 = $1`, digest).
		Scan(&session.CSRFDigest, &session.Subject, &session.Name, &role, &session.Authentication, &session.CreatedAt, &session.LastSeenAt, &session.ExpiresAt)
	if errors.Is(failure, pgx.ErrNoRows) {
		return Session{}, ErrSessionNotFound
	}
	if failure != nil {
		return Session{}, failure
	}
	parsed, known := ParseRole(role)
	if !known {
		return Session{}, ErrSessionNotFound
	}
	session.Role = parsed
	return session, nil
}

func (store *PostgresSessions) TouchSession(operation context.Context, digest []byte, at time.Time) error {
	_, failure := store.pool.Exec(operation, `update api_sessions set last_seen_at = $2 where id_sha256 = $1 and last_seen_at < $2`, digest, at)
	return failure
}

func (store *PostgresSessions) DeleteSession(operation context.Context, digest []byte) error {
	_, failure := store.pool.Exec(operation, `delete from api_sessions where id_sha256 = $1`, digest)
	return failure
}

func (store *PostgresSessions) LoginKey(operation context.Context) ([]byte, error) {
	candidate := make([]byte, 32)
	if _, failure := rand.Read(candidate); failure != nil {
		return nil, failure
	}
	if _, failure := store.pool.Exec(operation, `insert into api_login_key (id, key) values (1, $1) on conflict (id) do nothing`, candidate); failure != nil {
		return nil, failure
	}
	var key []byte
	failure := store.pool.QueryRow(operation, `select key from api_login_key where id = 1`).Scan(&key)
	return key, failure
}
