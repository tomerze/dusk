create table alerts (
    id bigint generated always as identity primary key,
    time timestamptz not null default now(),
    last_seen_at timestamptz not null default now(),
    occurrences bigint not null default 1,
    severity text not null check (severity in ('critical', 'high', 'medium', 'low')),
    kind text not null,
    fingerprint text not null,
    detail jsonb not null default '{}',
    acknowledged_by text,
    acknowledged_at timestamptz,
    resolved_by text,
    resolved_at timestamptz
);

create unique index alerts_open_fingerprint on alerts (fingerprint) where resolved_at is null;
create index alerts_time on alerts (time desc, id desc);

create table reconcile_offsets (
    partition integer primary key,
    "offset" bigint not null default -1,
    processed_at timestamptz,
    tail_instance text,
    tail_sequence bigint,
    tail_hash text,
    updated_at timestamptz not null default now()
);

create table ledger_chain_heads (
    instance text not null,
    partition integer not null,
    sequence bigint not null,
    hash text not null,
    checkpoint_sequence bigint,
    unsigned_since timestamptz,
    updated_at timestamptz not null default now(),
    primary key (instance, partition)
);

create table reconcile_processes (
    pid numeric(20, 0) primary key,
    first_seen_at timestamptz not null default now(),
    sessions text[] not null default '{}',
    created_at timestamptz,
    last_call_at timestamptz,
    commands integer not null default 0,
    delivered_at timestamptz,
    result_status text,
    result_at timestamptz,
    result_source jsonb,
    ledger_checked boolean not null default false
);

create index reconcile_processes_first_seen on reconcile_processes (first_seen_at);
create index reconcile_processes_unledgered on reconcile_processes (delivered_at)
    where created_at is null and not ledger_checked;

create table reconcile_default_shell (
    device_id text collate "C" not null,
    installation_id text collate "C" not null,
    window_start timestamptz not null,
    commands integer not null,
    last_call_at timestamptz not null,
    primary key (device_id, installation_id, window_start)
);

create index reconcile_default_shell_last_call on reconcile_default_shell (last_call_at);

create table leadership (
    id smallint primary key default 1 check (id = 1),
    term bigint not null default 0,
    holder text,
    acquired_at timestamptz
);

insert into leadership (id, term) values (1, 0);

create table api_tokens (
    id uuid primary key,
    name text not null check (length(name) between 1 and 200),
    token_sha256 bytea not null unique check (length(token_sha256) = 32),
    role text not null check (role in ('viewer', 'operator', 'admin')),
    created_by text not null,
    created_at timestamptz not null default now(),
    revoked_at timestamptz
);
