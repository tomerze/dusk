package inventory

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"dusk/services/twilight/internal/selector"
)

var ErrNotFound = errors.New("node not found")

type Store struct {
	pool *pgxpool.Pool
}

func NewStore(pool *pgxpool.Pool) *Store {
	return &Store{pool: pool}
}

const nodeColumns = `n.device_id, n.installation_id, n.cert_fingerprint, n.lifecycle, n.lifecycle_reason, n.lifecycle_changed_at,
	n.country, n.os_name, n.os_version, n.os_build, n.dusk_version, n.hardware_class, n.tenant, n.locale, n.hostname, n.impl, n.target_arch,
	n.facts, n.reported_version, n.reported_config_hash, n.reported_services, n.reported_at, n.facts_namespace_id, n.facts_read_at,
	n.enrolled_at, n.first_seen_at, n.updated_at`

func scanNode(row pgx.Row, extra ...any) (Node, error) {
	var node Node
	failure := row.Scan(append([]any{&node.DeviceID, &node.InstallationID, &node.CertFingerprint, &node.Lifecycle, &node.LifecycleReason, &node.LifecycleChangedAt,
		&node.Country, &node.OSName, &node.OSVersion, &node.OSBuild, &node.DuskVersion, &node.HardwareClass, &node.Tenant, &node.Locale,
		&node.Hostname, &node.Impl, &node.TargetArch, &node.FactsJSON, &node.ReportedVersion, &node.ReportedConfigHash, &node.ReportedServices,
		&node.ReportedAt, &node.FactsNamespaceID, &node.FactsReadAt, &node.EnrolledAt, &node.FirstSeenAt, &node.UpdatedAt}, extra...)...)
	return node, failure
}

func (store *Store) Get(operation context.Context, key Key) (Node, error) {
	node, failure := scanNode(store.pool.QueryRow(operation, `select `+nodeColumns+` from nodes n where n.device_id = $1 and n.installation_id = $2`, key.DeviceID, key.InstallationID))
	if errors.Is(failure, pgx.ErrNoRows) {
		return Node{}, ErrNotFound
	}
	return node, failure
}

func split(keys []Key) ([]string, []string) {
	devices := make([]string, len(keys))
	installations := make([]string, len(keys))
	for index, key := range keys {
		devices[index] = key.DeviceID
		installations[index] = key.InstallationID
	}
	return devices, installations
}

const batchLimit = 1000

func (store *Store) GetMany(operation context.Context, keys []Key) (map[Key]*Node, error) {
	found := make(map[Key]*Node, len(keys))
	for start := 0; start < len(keys); start += batchLimit {
		devices, installations := split(keys[start:min(len(keys), start+batchLimit)])
		rows, failure := store.pool.Query(operation, `select `+nodeColumns+` from unnest($1::text[], $2::text[]) as wanted(device_id, installation_id)
			join nodes n on n.device_id = wanted.device_id and n.installation_id = wanted.installation_id`, devices, installations)
		if failure != nil {
			return nil, fmt.Errorf("read nodes: %w", failure)
		}
		for rows.Next() {
			node, failure := scanNode(rows)
			if failure != nil {
				rows.Close()
				return nil, fmt.Errorf("read nodes: %w", failure)
			}
			found[node.Key()] = &node
		}
		rows.Close()
		if failure := rows.Err(); failure != nil {
			return nil, fmt.Errorf("read nodes: %w", failure)
		}
	}
	return found, nil
}

func arguments(compiled selector.Compiled) []any {
	values := make([]any, len(compiled.Parameters))
	for index, parameter := range compiled.Parameters {
		values[index] = parameter
	}
	return values
}

type Page struct {
	Nodes []Node `json:"nodes"`
	Next  string `json:"next"`
}

var ErrMalformedCursor = errors.New("malformed cursor")

type sortColumn struct {
	expression string
	cast       string
}

var sortColumns = map[string]sortColumn{
	"hostname":         {"%[1]s.hostname", "text"},
	"lifecycle":        {"%[1]s.lifecycle", "text"},
	"country":          {"%[1]s.country", "text"},
	"os_name":          {"%[1]s.os_name", "text"},
	"os_version":       {"%[1]s.os_version", "text"},
	"os_build":         {"%[1]s.os_build", "text"},
	"dusk_version":     {"semver_key(%[1]s.dusk_version)", "numeric[]"},
	"hardware_class":   {"%[1]s.hardware_class", "text"},
	"tenant":           {"%[1]s.tenant", "text"},
	"locale":           {"%[1]s.locale", "text"},
	"impl":             {"%[1]s.impl", "text"},
	"target_arch":      {"%[1]s.target_arch", "text"},
	"reported_version": {"semver_key(%[1]s.reported_version)", "numeric[]"},
}

func SortColumns() []string {
	names := []string{"device_id"}
	for name := range sortColumns {
		names = append(names, name)
	}
	sort.Strings(names[1:])
	return names
}

type Order struct {
	Column     string
	Descending bool
}

func ParseOrder(text string) (Order, error) {
	if text == "" {
		return Order{Column: "device_id"}, nil
	}
	order := Order{Column: strings.TrimPrefix(text, "-"), Descending: strings.HasPrefix(text, "-")}
	if _, known := sortColumns[order.Column]; !known && order.Column != "device_id" {
		return Order{}, fmt.Errorf("sort must be one of %s, optionally prefixed with - for descending", strings.Join(SortColumns(), ", "))
	}
	return order, nil
}

func (order Order) String() string {
	if order.Descending {
		return "-" + order.Column
	}
	return order.Column
}

type cursor struct {
	Order          string  `json:"o"`
	Value          *string `json:"v"`
	DeviceID       string  `json:"d"`
	InstallationID string  `json:"i"`
}

func (order Order) CursorAfter(node Node) string {
	encoded, _ := json.Marshal(cursor{Order: order.String(), Value: node.sortValue, DeviceID: node.DeviceID, InstallationID: node.InstallationID})
	return base64.RawURLEncoding.EncodeToString(encoded)
}

func (order Order) decode(text string) (cursor, error) {
	var decoded cursor
	raw, failure := base64.RawURLEncoding.DecodeString(text)
	if failure != nil || json.Unmarshal(raw, &decoded) != nil || decoded.Order != order.String() || decoded.DeviceID == "" || decoded.InstallationID == "" {
		return cursor{}, ErrMalformedCursor
	}
	return decoded, nil
}

func (order Order) sql(values []any, after *cursor) (string, string, string, []any) {
	key := "(n.device_id, n.installation_id)"
	direction, comparison := "", ">"
	if order.Descending {
		direction, comparison = " desc", "<"
	}
	column, sorted := sortColumns[order.Column]
	if !sorted {
		condition := ""
		if after != nil {
			values = append(values, after.DeviceID, after.InstallationID)
			condition = fmt.Sprintf(` and %s %s ($%d, $%d)`, key, comparison, len(values)-1, len(values))
		}
		return `, null::text`, condition, ` order by n.device_id` + direction + `, n.installation_id` + direction, values
	}
	expression := fmt.Sprintf(column.expression, "n")
	nulls := " nulls last"
	if order.Descending {
		nulls = " nulls first"
	}
	ordering := ` order by ` + expression + direction + nulls + `, n.device_id` + direction + `, n.installation_id` + direction
	if after == nil {
		return `, (` + expression + `)::text`, "", ordering, values
	}
	values = append(values, after.DeviceID, after.InstallationID)
	tie := fmt.Sprintf(`%s %s ($%d, $%d)`, key, comparison, len(values)-1, len(values))
	var condition string
	switch {
	case after.Value == nil && !order.Descending:
		condition = fmt.Sprintf(` and (%s is null and %s)`, expression, tie)
	case after.Value == nil:
		condition = fmt.Sprintf(` and ((%s is null and %s) or %s is not null)`, expression, tie, expression)
	default:
		values = append(values, *after.Value)
		value := fmt.Sprintf(`$%d::%s`, len(values), column.cast)
		condition = fmt.Sprintf(` and (%s %s %s or (%s = %s and %s)`, expression, comparison, value, expression, value, tie)
		if order.Descending {
			condition += `)`
		} else {
			condition += fmt.Sprintf(` or %s is null)`, expression)
		}
	}
	return `, (` + expression + `)::text`, condition, ordering, values
}

func (store *Store) List(operation context.Context, parsed *selector.Selector, order Order, after string, limit int) (Page, error) {
	if limit < 1 || limit > 1000 {
		return Page{}, errors.New("limit must be between 1 and 1000")
	}
	var position *cursor
	if after != "" {
		decoded, failure := order.decode(after)
		if failure != nil {
			return Page{}, failure
		}
		position = &decoded
	}
	compiled := parsed.Compile("n", 1)
	sortValue, condition, ordering, values := order.sql(arguments(compiled), position)
	values = append(values, limit+1)
	query := `select ` + nodeColumns + sortValue + ` from nodes n where (` + compiled.SQL + `)` + condition + ordering + fmt.Sprintf(` limit $%d`, len(values))
	rows, failure := store.pool.Query(operation, query, values...)
	if failure != nil {
		return Page{}, fmt.Errorf("list nodes: %w", failure)
	}
	nodes, failure := pgx.CollectRows(rows, func(row pgx.CollectableRow) (Node, error) {
		var value *string
		node, failure := scanNode(row, &value)
		node.sortValue = value
		return node, failure
	})
	if failure != nil {
		return Page{}, fmt.Errorf("list nodes: %w", failure)
	}
	page := Page{Nodes: nodes}
	if len(nodes) > limit {
		page.Nodes = nodes[:limit]
		page.Next = order.CursorAfter(page.Nodes[limit-1])
	}
	return page, nil
}

func (store *Store) Count(operation context.Context, parsed *selector.Selector) (int64, error) {
	compiled := parsed.Compile("n", 1)
	var count int64
	failure := store.pool.QueryRow(operation, `select count(*) from nodes n where `+compiled.SQL, arguments(compiled)...).Scan(&count)
	return count, failure
}

func (store *Store) LifecycleCounts(operation context.Context) (map[string]int64, error) {
	rows, failure := store.pool.Query(operation, `select lifecycle, count(*) from nodes group by lifecycle`)
	if failure != nil {
		return nil, failure
	}
	counts := map[string]int64{}
	for rows.Next() {
		var lifecycle string
		var count int64
		if failure := rows.Scan(&lifecycle, &count); failure != nil {
			rows.Close()
			return nil, failure
		}
		counts[lifecycle] = count
	}
	rows.Close()
	return counts, rows.Err()
}

type Enrollment struct {
	Key
	Operation       string
	CertFingerprint *string
	Tenant          *string
	DuskVersion     *string
	Impl            *string
	TargetArch      *string
	Hostname        *string
	At              time.Time
}

func (store *Store) RecordEnrollment(operation context.Context, enrollment Enrollment) error {
	if enrollment.Operation == "renew" {
		_, failure := store.pool.Exec(operation, `insert into nodes (device_id, installation_id, cert_fingerprint, tenant, first_seen_at)
			values ($1, $2, $3, $4, $5)
			on conflict (device_id, installation_id) do update set cert_fingerprint = excluded.cert_fingerprint, updated_at = now()`,
			enrollment.DeviceID, enrollment.InstallationID, enrollment.CertFingerprint, enrollment.Tenant, enrollment.At)
		return failure
	}
	_, failure := store.pool.Exec(operation, `insert into nodes (device_id, installation_id, cert_fingerprint, tenant, dusk_version, impl, target_arch, hostname, enrolled_at, first_seen_at, lifecycle, lifecycle_changed_at, lifecycle_reason)
		values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, 'enrolled', $9, 'enrolled')
		on conflict (device_id, installation_id) do update set cert_fingerprint = excluded.cert_fingerprint, tenant = excluded.tenant,
			enrolled_at = coalesce(nodes.enrolled_at, excluded.enrolled_at), updated_at = now()`,
		enrollment.DeviceID, enrollment.InstallationID, enrollment.CertFingerprint, enrollment.Tenant, enrollment.DuskVersion,
		enrollment.Impl, enrollment.TargetArch, enrollment.Hostname, enrollment.At)
	return failure
}

type Sighting struct {
	Key
	At     time.Time
	Tenant *string
}

func (store *Store) RecordConnections(operation context.Context, sightings []Sighting) ([]Key, error) {
	var activated []Key
	for start := 0; start < len(sightings); start += batchLimit {
		chunk := sightings[start:min(len(sightings), start+batchLimit)]
		devices := make([]string, len(chunk))
		installations := make([]string, len(chunk))
		times := make([]time.Time, len(chunk))
		tenants := make([]*string, len(chunk))
		for index, sighting := range chunk {
			devices[index], installations[index], times[index], tenants[index] = sighting.DeviceID, sighting.InstallationID, sighting.At, sighting.Tenant
		}
		failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
			if _, failure := transaction.Exec(operation, `insert into nodes (device_id, installation_id, first_seen_at, tenant)
				select device_id, installation_id, seen, tenant from unnest($1::text[], $2::text[], $3::timestamptz[], $4::text[]) as sighted(device_id, installation_id, seen, tenant)
				on conflict (device_id, installation_id) do nothing`, devices, installations, times, tenants); failure != nil {
				return failure
			}
			rows, failure := transaction.Query(operation, `update nodes n set lifecycle = 'active', lifecycle_changed_at = sighted.seen, lifecycle_reason = 'first connection', updated_at = now()
				from unnest($1::text[], $2::text[], $3::timestamptz[]) as sighted(device_id, installation_id, seen)
				where n.device_id = sighted.device_id and n.installation_id = sighted.installation_id and n.lifecycle = 'enrolled'
				returning n.device_id, n.installation_id`, devices, installations, times)
			if failure != nil {
				return failure
			}
			keys, failure := pgx.CollectRows(rows, pgx.RowToStructByPos[Key])
			activated = append(activated, keys...)
			return failure
		})
		if failure != nil {
			return activated, fmt.Errorf("record connections: %w", failure)
		}
	}
	return activated, nil
}

type Reported struct {
	VersionKey *string
	Version    *string
	ConfigHash *string
	Services   []string
	Facts      map[string]json.RawMessage
}

const ConfigHashFact = "dusk.config.hash"

func (store *Store) ApplyReport(operation context.Context, key Key, namespaceID string, at time.Time, report Reported) error {
	return pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		var current json.RawMessage
		var readAt *time.Time
		if failure := transaction.QueryRow(operation, `select facts, facts_read_at from nodes where device_id = $1 and installation_id = $2 for update`,
			key.DeviceID, key.InstallationID).Scan(&current, &readAt); failure != nil {
			if errors.Is(failure, pgx.ErrNoRows) {
				return ErrNotFound
			}
			return failure
		}
		if readAt != nil && readAt.After(at) {
			return nil
		}
		facts := map[string]json.RawMessage{}
		replaced := report.Facts != nil
		if replaced {
			for name, value := range report.Facts {
				facts[name] = value
			}
		} else {
			if failure := json.Unmarshal(current, &facts); failure != nil {
				return fmt.Errorf("stored facts: %w", failure)
			}
		}
		if report.VersionKey != nil && report.Version != nil {
			encoded, _ := json.Marshal(*report.Version)
			facts[*report.VersionKey] = encoded
		}
		if report.ConfigHash != nil {
			encoded, _ := json.Marshal(*report.ConfigHash)
			facts[ConfigHashFact] = encoded
		}
		columns, cleaned := Normalize(facts)
		encoded, failure := json.Marshal(cleaned)
		if failure != nil {
			return failure
		}
		var reportedVersion *string
		if version, found := factReader(cleaned).text("dusk.version"); found {
			reportedVersion = &version
		}
		configHash := factReader(cleaned).pointer(ConfigHashFact)
		statement := `update nodes set facts = $3::jsonb, country = coalesce($4, country), os_name = $5, os_version = $6, os_build = $7,
			dusk_version = coalesce($8, dusk_version), hardware_class = $9, locale = $10, hostname = coalesce($11, hostname), impl = coalesce($12, impl),
			target_arch = coalesce($13, target_arch), reported_version = coalesce($14, reported_version), reported_config_hash = coalesce($15, reported_config_hash),
			reported_services = coalesce($16, reported_services), reported_at = $17, updated_at = now()`
		values := []any{key.DeviceID, key.InstallationID, encoded, columns.Country, columns.OSName, columns.OSVersion, columns.OSBuild,
			columns.DuskVersion, columns.HardwareClass, columns.Locale, columns.Hostname, columns.Impl, columns.TargetArch,
			reportedVersion, configHash, report.Services, at}
		if replaced {
			values = append(values, namespaceID)
			statement += `, facts_namespace_id = $18, facts_read_at = $17`
		}
		_, failure = transaction.Exec(operation, statement+` where device_id = $1 and installation_id = $2`, values...)
		return failure
	})
}

func (store *Store) SetLifecycle(operation context.Context, key Key, lifecycle, reason string, at time.Time, before func(previous string) error) (string, error) {
	var previous string
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		failure := transaction.QueryRow(operation, `select lifecycle from nodes where device_id = $1 and installation_id = $2 for update`, key.DeviceID, key.InstallationID).Scan(&previous)
		if errors.Is(failure, pgx.ErrNoRows) {
			return ErrNotFound
		}
		if failure != nil {
			return failure
		}
		if failure := before(previous); failure != nil {
			return failure
		}
		_, failure = transaction.Exec(operation, `update nodes set lifecycle = $3, lifecycle_reason = $4, lifecycle_changed_at = $5, updated_at = now()
			where device_id = $1 and installation_id = $2 and lifecycle <> $3`, key.DeviceID, key.InstallationID, lifecycle, reason, at)
		return failure
	})
	return previous, failure
}

type DeviceLifecycle struct {
	DeviceID  string    `json:"device_id"`
	Lifecycle string    `json:"lifecycle"`
	Reason    string    `json:"reason"`
	ChangedAt time.Time `json:"changed_at"`
	Actor     string    `json:"actor"`
}

func (store *Store) SetDeviceLifecycle(operation context.Context, changed DeviceLifecycle, before func(previous string) error) (string, error) {
	previous := "active"
	failure := pgx.BeginFunc(operation, store.pool, func(transaction pgx.Tx) error {
		if _, failure := transaction.Exec(operation, `select pg_advisory_xact_lock(hashtextextended('devices/' || $1, 0))`, changed.DeviceID); failure != nil {
			return failure
		}
		failure := transaction.QueryRow(operation, `select lifecycle from devices where device_id = $1`, changed.DeviceID).Scan(&previous)
		if failure != nil && !errors.Is(failure, pgx.ErrNoRows) {
			return failure
		}
		if failure := before(previous); failure != nil {
			return failure
		}
		_, failure = transaction.Exec(operation, `insert into devices (device_id, lifecycle, lifecycle_reason, lifecycle_changed_at, actor) values ($1, $2, $3, $4, $5)
			on conflict (device_id) do update set lifecycle = excluded.lifecycle, lifecycle_reason = excluded.lifecycle_reason,
				lifecycle_changed_at = excluded.lifecycle_changed_at, actor = excluded.actor`,
			changed.DeviceID, changed.Lifecycle, changed.Reason, changed.ChangedAt, changed.Actor)
		return failure
	})
	return previous, failure
}

func (store *Store) GetDeviceLifecycle(operation context.Context, device string) (*DeviceLifecycle, error) {
	found := &DeviceLifecycle{DeviceID: device}
	failure := store.pool.QueryRow(operation, `select lifecycle, lifecycle_reason, lifecycle_changed_at, actor from devices where device_id = $1`, device).
		Scan(&found.Lifecycle, &found.Reason, &found.ChangedAt, &found.Actor)
	if errors.Is(failure, pgx.ErrNoRows) {
		return nil, nil
	}
	if failure != nil {
		return nil, failure
	}
	return found, nil
}

type Lifecycle struct {
	Key
	Lifecycle string
	ChangedAt time.Time
}

func (store *Store) Withdrawn(operation context.Context) ([]Lifecycle, error) {
	rows, failure := store.pool.Query(operation, `select n.device_id, n.installation_id, coalesce(d.lifecycle, n.lifecycle), coalesce(d.lifecycle_changed_at, n.lifecycle_changed_at, n.updated_at)
		from nodes n
		join node_presence p on p.device_id = n.device_id and p.installation_id = n.installation_id
		left join devices d on d.device_id = n.device_id and d.lifecycle in ('revoked', 'retired')
		where (n.lifecycle in ('revoked', 'retired') or d.device_id is not null) and p.online`)
	if failure != nil {
		return nil, failure
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Lifecycle, error) {
		var entry Lifecycle
		return entry, row.Scan(&entry.DeviceID, &entry.InstallationID, &entry.Lifecycle, &entry.ChangedAt)
	})
}

func (store *Store) WritePresence(operation context.Context, rows []Presence) error {
	for start := 0; start < len(rows); start += 5000 {
		batch := &pgx.Batch{}
		for _, row := range rows[start:min(len(rows), start+5000)] {
			sessions := row.Sessions
			if sessions == nil {
				sessions = []PresenceSession{}
			}
			encoded, failure := json.Marshal(sessions)
			if failure != nil {
				return failure
			}
			firstSeen := time.Now()
			if row.ConnectedAt != nil {
				firstSeen = *row.ConnectedAt
			}
			batch.Queue(`insert into nodes (device_id, installation_id, first_seen_at) values ($1, $2, $3) on conflict do nothing`, row.DeviceID, row.InstallationID, firstSeen)
			batch.Queue(`insert into node_presence (device_id, installation_id, online, last_seen_at, connected_at, disconnected_at, sessions)
				values ($1, $2, $3, $4, $5, $6, $7::jsonb)
				on conflict (device_id, installation_id) do update set online = excluded.online,
					last_seen_at = greatest(node_presence.last_seen_at, excluded.last_seen_at),
					connected_at = coalesce(excluded.connected_at, node_presence.connected_at),
					disconnected_at = coalesce(excluded.disconnected_at, node_presence.disconnected_at),
					sessions = excluded.sessions`,
				row.DeviceID, row.InstallationID, row.Online, row.LastSeenAt, row.ConnectedAt, row.DisconnectedAt, encoded)
		}
		if failure := store.pool.SendBatch(operation, batch).Close(); failure != nil {
			return fmt.Errorf("write presence: %w", failure)
		}
	}
	return nil
}

func (store *Store) UpdateLastSeen(operation context.Context, keys []Key, times []time.Time) error {
	for start := 0; start < len(keys); start += 5000 {
		end := min(len(keys), start+5000)
		devices, installations := split(keys[start:end])
		if _, failure := store.pool.Exec(operation, `update node_presence p set last_seen_at = seen.at
			from unnest($1::text[], $2::text[], $3::timestamptz[]) as seen(device_id, installation_id, at)
			where p.device_id = seen.device_id and p.installation_id = seen.installation_id
			and (p.last_seen_at is null or p.last_seen_at < seen.at)`, devices, installations, times[start:end]); failure != nil {
			return fmt.Errorf("update last_seen_at: %w", failure)
		}
	}
	return nil
}

const presencePage = 5000

func (store *Store) ScanPresence(operation context.Context, visit func(Presence) error) error {
	var after Key
	for {
		rows, failure := store.pool.Query(operation, `select device_id, installation_id, online, last_seen_at, connected_at, disconnected_at, sessions from node_presence
			where (device_id, installation_id) > ($1, $2) order by device_id, installation_id limit $3`, after.DeviceID, after.InstallationID, presencePage)
		if failure != nil {
			return failure
		}
		page, failure := pgx.CollectRows(rows, func(row pgx.CollectableRow) (Presence, error) {
			var scanned Presence
			var sessions json.RawMessage
			if failure := row.Scan(&scanned.DeviceID, &scanned.InstallationID, &scanned.Online, &scanned.LastSeenAt, &scanned.ConnectedAt, &scanned.DisconnectedAt, &sessions); failure != nil {
				return scanned, failure
			}
			if failure := json.Unmarshal(sessions, &scanned.Sessions); failure != nil {
				return scanned, fmt.Errorf("presence of %s: %w", scanned.Key, failure)
			}
			return scanned, nil
		})
		if failure != nil {
			return failure
		}
		for _, row := range page {
			if failure := visit(row); failure != nil {
				return failure
			}
		}
		if len(page) < presencePage {
			return nil
		}
		after = page[len(page)-1].Key
	}
}

func (store *Store) GetPresence(operation context.Context, key Key) (Presence, error) {
	row := Presence{Key: key}
	var sessions json.RawMessage
	failure := store.pool.QueryRow(operation, `select online, last_seen_at, connected_at, disconnected_at, sessions from node_presence where device_id = $1 and installation_id = $2`,
		key.DeviceID, key.InstallationID).Scan(&row.Online, &row.LastSeenAt, &row.ConnectedAt, &row.DisconnectedAt, &sessions)
	if errors.Is(failure, pgx.ErrNoRows) {
		return row, ErrNotFound
	}
	if failure != nil {
		return row, failure
	}
	return row, json.Unmarshal(sessions, &row.Sessions)
}
