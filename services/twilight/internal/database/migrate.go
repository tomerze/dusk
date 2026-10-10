package database

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"regexp"
	"sort"
	"strconv"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

type Migration struct {
	Version  int64
	Name     string
	SQL      string
	Checksum string
}

type MigrationStatus struct {
	Applied []int64
	Pending []Migration
	Unknown []int64
}

var migrationName = regexp.MustCompile(`^([0-9]{4,})_([a-z0-9_]+)\.sql$`)

func LoadMigrations(files fs.FS) ([]Migration, error) {
	entries, failure := fs.ReadDir(files, ".")
	if failure != nil {
		return nil, fmt.Errorf("list migrations: %w", failure)
	}
	var loaded []Migration
	seen := map[int64]string{}
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		match := migrationName.FindStringSubmatch(entry.Name())
		if match == nil {
			if len(entry.Name()) > 4 && entry.Name()[len(entry.Name())-4:] == ".sql" {
				return nil, fmt.Errorf("migration %s is not named <version>_<name>.sql", entry.Name())
			}
			continue
		}
		version, _ := strconv.ParseInt(match[1], 10, 64)
		if previous, duplicate := seen[version]; duplicate {
			return nil, fmt.Errorf("migrations %s and %s share version %d", previous, entry.Name(), version)
		}
		seen[version] = entry.Name()
		content, failure := fs.ReadFile(files, entry.Name())
		if failure != nil {
			return nil, fmt.Errorf("read migration %s: %w", entry.Name(), failure)
		}
		digest := sha256.Sum256(content)
		loaded = append(loaded, Migration{Version: version, Name: match[2], SQL: string(content), Checksum: hex.EncodeToString(digest[:])})
	}
	sort.Slice(loaded, func(left, right int) bool { return loaded[left].Version < loaded[right].Version })
	return loaded, nil
}

const createMigrationsTable = `create table if not exists schema_migrations (
    version bigint primary key,
    name text not null,
    checksum text not null,
    applied_at timestamptz not null default now()
)`

func status(operation context.Context, connection *pgx.Conn, migrations []Migration) (MigrationStatus, error) {
	var result MigrationStatus
	var exists bool
	if failure := connection.QueryRow(operation, `select to_regclass('schema_migrations') is not null`).Scan(&exists); failure != nil {
		return result, fmt.Errorf("look up schema_migrations: %w", failure)
	}
	applied := map[int64]string{}
	if exists {
		rows, failure := connection.Query(operation, `select version, checksum from schema_migrations order by version`)
		if failure != nil {
			return result, fmt.Errorf("read schema_migrations: %w", failure)
		}
		for rows.Next() {
			var version int64
			var checksum string
			if failure := rows.Scan(&version, &checksum); failure != nil {
				rows.Close()
				return result, fmt.Errorf("read schema_migrations: %w", failure)
			}
			applied[version] = checksum
		}
		rows.Close()
		if failure := rows.Err(); failure != nil {
			return result, fmt.Errorf("read schema_migrations: %w", failure)
		}
	}
	known := map[int64]bool{}
	for _, migration := range migrations {
		known[migration.Version] = true
		checksum, done := applied[migration.Version]
		if !done {
			result.Pending = append(result.Pending, migration)
			continue
		}
		if checksum != migration.Checksum {
			return result, fmt.Errorf("migration %04d_%s was applied with checksum %s but this build carries %s; migrations never change once applied", migration.Version, migration.Name, checksum, migration.Checksum)
		}
		result.Applied = append(result.Applied, migration.Version)
	}
	for version := range applied {
		if !known[version] {
			result.Unknown = append(result.Unknown, version)
		}
	}
	sort.Slice(result.Unknown, func(left, right int) bool { return result.Unknown[left] < result.Unknown[right] })
	return result, nil
}

func Status(operation context.Context, pool *pgxpool.Pool, migrations []Migration) (MigrationStatus, error) {
	connection, failure := pool.Acquire(operation)
	if failure != nil {
		return MigrationStatus{}, fmt.Errorf("acquire connection: %w", failure)
	}
	defer connection.Release()
	return status(operation, connection.Conn(), migrations)
}

func Migrate(operation context.Context, pool *pgxpool.Pool, migrations []Migration, logger *slog.Logger) ([]Migration, error) {
	connection, failure := pool.Acquire(operation)
	if failure != nil {
		return nil, fmt.Errorf("acquire connection: %w", failure)
	}
	defer connection.Release()
	if _, failure := connection.Exec(operation, `select pg_advisory_lock($1)`, migrationLockKey); failure != nil {
		return nil, fmt.Errorf("take the migration lock: %w", failure)
	}
	defer func() {
		unlocking, cancel := context.WithTimeout(context.WithoutCancel(operation), 10*time.Second)
		defer cancel()
		if _, failure := connection.Exec(unlocking, `select pg_advisory_unlock($1)`, migrationLockKey); failure != nil {
			logger.Warn("release the migration lock", "error", failure)
			connection.Conn().Close(unlocking)
		}
	}()
	if _, failure := connection.Exec(operation, createMigrationsTable); failure != nil {
		return nil, fmt.Errorf("create schema_migrations: %w", failure)
	}
	current, failure := status(operation, connection.Conn(), migrations)
	if failure != nil {
		return nil, failure
	}
	if len(current.Unknown) > 0 {
		logger.Warn("the database holds migrations this build does not know", "versions", current.Unknown)
	}
	var applied []Migration
	for _, migration := range current.Pending {
		failure := pgx.BeginFunc(operation, connection.Conn(), func(transaction pgx.Tx) error {
			if _, failure := transaction.Exec(operation, migration.SQL); failure != nil {
				return failure
			}
			_, failure := transaction.Exec(operation, `insert into schema_migrations (version, name, checksum) values ($1, $2, $3)`, migration.Version, migration.Name, migration.Checksum)
			return failure
		})
		if failure != nil {
			return applied, fmt.Errorf("apply migration %04d_%s: %w", migration.Version, migration.Name, failure)
		}
		logger.Info("migration applied", "version", migration.Version, "name", migration.Name)
		applied = append(applied, migration)
	}
	return applied, nil
}

var ErrPendingMigrations = errors.New("the database lacks migrations this build needs; run twilight migrate")
