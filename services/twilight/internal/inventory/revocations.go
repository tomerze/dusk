package inventory

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

var ErrRevocationNotFound = errors.New("revocation not found")

type RevocationScope struct {
	CredentialKind string     `json:"credential_kind"`
	Credential     string     `json:"credential"`
	EnrolledAfter  *time.Time `json:"enrolled_after"`
	EnrolledBefore *time.Time `json:"enrolled_before"`
}

type Revocation struct {
	ID uuid.UUID `json:"id"`
	RevocationScope
	Reason     string     `json:"reason"`
	Actor      string     `json:"actor"`
	CreatedAt  time.Time  `json:"created_at"`
	Matched    int64      `json:"matched"`
	Revoked    int64      `json:"revoked"`
	FinishedAt *time.Time `json:"finished_at"`
	cursor     Key
}

type RevocationPage struct {
	Revocations []Revocation `json:"revocations"`
	Next        string       `json:"next"`
}

const revocationColumns = `id, credential_kind, credential, enrolled_after, enrolled_before, reason, actor, created_at, matched, revoked, finished_at,
	cursor_device_id, cursor_installation_id`

func scanRevocation(row pgx.Row) (Revocation, error) {
	var revocation Revocation
	failure := row.Scan(&revocation.ID, &revocation.CredentialKind, &revocation.Credential, &revocation.EnrolledAfter, &revocation.EnrolledBefore,
		&revocation.Reason, &revocation.Actor, &revocation.CreatedAt, &revocation.Matched, &revocation.Revoked, &revocation.FinishedAt,
		&revocation.cursor.DeviceID, &revocation.cursor.InstallationID)
	if errors.Is(failure, pgx.ErrNoRows) {
		return revocation, ErrRevocationNotFound
	}
	return revocation, failure
}

func (scope RevocationScope) condition(first int) (string, []any) {
	column := "credential_ref"
	if scope.CredentialKind == "install_token" {
		column = "credential_issuer"
	}
	conditions := []string{fmt.Sprintf("n.%s = $%d and n.credential_kind = $%d and n.lifecycle <> 'revoked'", column, first, first+1)}
	values := []any{scope.Credential, scope.CredentialKind}
	if scope.EnrolledAfter != nil {
		values = append(values, *scope.EnrolledAfter)
		conditions = append(conditions, fmt.Sprintf("n.enrolled_at >= $%d", first+len(values)-1))
	}
	if scope.EnrolledBefore != nil {
		values = append(values, *scope.EnrolledBefore)
		conditions = append(conditions, fmt.Sprintf("n.enrolled_at < $%d", first+len(values)-1))
	}
	return strings.Join(conditions, " and "), values
}

func (store *Store) CountRevocable(operation context.Context, scope RevocationScope) (int64, error) {
	condition, values := scope.condition(1)
	var count int64
	failure := store.pool.QueryRow(operation, `select count(*) from nodes n where `+condition, values...).Scan(&count)
	return count, failure
}

func (store *Store) CreateRevocation(operation context.Context, scope RevocationScope, reason, actor string, at time.Time) (Revocation, error) {
	identifier, failure := uuid.NewV7()
	if failure != nil {
		return Revocation{}, failure
	}
	condition, values := scope.condition(9)
	arguments := append([]any{identifier, scope.CredentialKind, scope.Credential, scope.EnrolledAfter, scope.EnrolledBefore, reason, actor, at}, values...)
	return scanRevocation(store.pool.QueryRow(operation, `insert into revocations (id, credential_kind, credential, enrolled_after, enrolled_before, reason, actor, created_at, matched)
		select $1, $2, $3, $4, $5, $6, $7, $8, (select count(*) from nodes n where `+condition+`)
		returning `+revocationColumns, arguments...))
}

func (store *Store) GetRevocation(operation context.Context, identifier uuid.UUID) (Revocation, error) {
	return scanRevocation(store.pool.QueryRow(operation, `select `+revocationColumns+` from revocations where id = $1`, identifier))
}

func (store *Store) ListRevocations(operation context.Context, cursor string, limit int) (RevocationPage, error) {
	query := `select ` + revocationColumns + ` from revocations`
	values := []any{}
	if cursor != "" {
		decoded, failure := base64.RawURLEncoding.DecodeString(cursor)
		if failure != nil {
			return RevocationPage{}, ErrMalformedCursor
		}
		at, identifierText, found := strings.Cut(string(decoded), "|")
		createdAt, timeFailure := time.Parse(time.RFC3339Nano, at)
		identifier, identifierFailure := uuid.Parse(identifierText)
		if !found || timeFailure != nil || identifierFailure != nil {
			return RevocationPage{}, ErrMalformedCursor
		}
		values = append(values, createdAt, identifier)
		query += ` where (created_at, id) < ($1, $2)`
	}
	values = append(values, limit+1)
	rows, failure := store.pool.Query(operation, query+fmt.Sprintf(` order by created_at desc, id desc limit $%d`, len(values)), values...)
	if failure != nil {
		return RevocationPage{}, failure
	}
	listed, failure := pgx.CollectRows(rows, func(row pgx.CollectableRow) (Revocation, error) { return scanRevocation(row) })
	if failure != nil {
		return RevocationPage{}, failure
	}
	page := RevocationPage{Revocations: listed}
	if len(listed) > limit {
		page.Revocations = listed[:limit]
		last := page.Revocations[limit-1]
		page.Next = base64.RawURLEncoding.EncodeToString([]byte(last.CreatedAt.UTC().Format(time.RFC3339Nano) + "|" + last.ID.String()))
	}
	if page.Revocations == nil {
		page.Revocations = []Revocation{}
	}
	return page, nil
}

func (store *Store) RunningRevocations(operation context.Context) ([]Revocation, error) {
	rows, failure := store.pool.Query(operation, `select `+revocationColumns+` from revocations where finished_at is null order by created_at, id limit 100`)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Revocation, error) { return scanRevocation(row) })
}

func (store *Store) RevocationBatch(operation context.Context, revocation Revocation, limit int) ([]Key, error) {
	condition, values := revocation.condition(4)
	rows, failure := store.pool.Query(operation, `select n.device_id, n.installation_id from nodes n
		where (n.device_id, n.installation_id) > ($1, $2) and `+condition+`
		order by n.device_id, n.installation_id limit $3`,
		append([]any{revocation.cursor.DeviceID, revocation.cursor.InstallationID, limit}, values...)...)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Key, error) {
		var key Key
		return key, row.Scan(&key.DeviceID, &key.InstallationID)
	})
}

var ErrNotLeading = errors.New("this instance no longer leads")

func (store *Store) RevokeBatch(operation context.Context, revocation *Revocation, keys []Key, at time.Time, term int64) error {
	devices := make([]string, len(keys))
	installations := make([]string, len(keys))
	for index, key := range keys {
		devices[index], installations[index] = key.DeviceID, key.InstallationID
	}
	var revoked int64
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		var leading bool
		if failure := transaction.QueryRow(operation, `select exists (select 1 from leadership where id = 1 and term = $1)`, term).Scan(&leading); failure != nil {
			return failure
		}
		if !leading {
			return ErrNotLeading
		}
		tag, failure := transaction.Exec(operation, `update nodes n set lifecycle = 'revoked', lifecycle_reason = $3, lifecycle_changed_at = $4, updated_at = now()
			from unnest($1::text[], $2::text[]) as revoking(device_id, installation_id)
			where n.device_id = revoking.device_id and n.installation_id = revoking.installation_id and n.lifecycle <> 'revoked'`,
			devices, installations, revocation.Reason, at)
		if failure != nil {
			return failure
		}
		revoked = tag.RowsAffected()
		last := keys[len(keys)-1]
		_, failure = transaction.Exec(operation, `update revocations set revoked = revoked + $2, cursor_device_id = $3, cursor_installation_id = $4 where id = $1`,
			revocation.ID, revoked, last.DeviceID, last.InstallationID)
		return failure
	})
	if failure != nil {
		return failure
	}
	revocation.Revoked += revoked
	revocation.cursor = keys[len(keys)-1]
	return nil
}

func (store *Store) FinishRevocation(operation context.Context, revocation *Revocation, at time.Time) error {
	tag, failure := store.pool.Exec(operation, `update revocations set finished_at = $2 where id = $1 and finished_at is null`, revocation.ID, at)
	if failure == nil && tag.RowsAffected() == 1 {
		revocation.FinishedAt = &at
	}
	return failure
}
