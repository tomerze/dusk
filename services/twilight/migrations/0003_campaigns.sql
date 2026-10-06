create table campaigns (
    id uuid primary key,
    name text not null check (length(name) between 1 and 200),
    description text not null default '',
    tenant text,
    status text not null check (status in ('draft', 'running', 'paused', 'completed', 'aborted', 'failed', 'archived')),
    kind text not null check (kind in ('run_script', 'ensure_version', 'ensure_config', 'quarantine')),
    selector text not null,
    selector_sql_snapshot text,
    selector_sql_parameters jsonb,
    action jsonb not null,
    policy jsonb not null,
    salt bytea not null check (length(salt) = 32),
    created_by text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    started_at timestamptz,
    paused_at timestamptz,
    finished_at timestamptz,
    current_phase integer not null default 0,
    phase_started_at timestamptz,
    phase_paused_seconds bigint not null default 0,
    gate_override_after timestamptz,
    pause_kind text check (pause_kind in ('operator', 'gate', 'permission')),
    pause_reason text,
    abort_reason text,
    sweep_cursor jsonb,
    last_dispatch_at timestamptz,
    version integer not null default 1
);

create index campaigns_status on campaigns (status, created_at desc, id);
create index campaigns_created on campaigns (created_at desc, id);

create table campaign_counters (
    campaign_id uuid not null references campaigns on delete cascade,
    phase integer not null,
    state text not null,
    count bigint not null,
    primary key (campaign_id, phase, state)
);

create table campaign_events (
    campaign_id uuid not null,
    id bigint generated always as identity,
    time timestamptz not null default now(),
    kind text not null,
    actor text not null,
    detail jsonb not null default '{}',
    primary key (campaign_id, id)
) partition by list (campaign_id);

create table campaign_nodes (
    campaign_id uuid not null,
    device_id text collate "C" not null,
    installation_id text collate "C" not null,
    phase integer not null,
    state text not null check (state in (
        'pending', 'dispatching', 'dispatched', 'delivered', 'verifying', 'backoff',
        'succeeded', 'failed', 'unknown', 'excluded', 'conflict', 'cancelled'
    )),
    attempt integer not null default 1,
    failures integer not null default 0,
    unreached integer not null default 0,
    pid numeric(20, 0) check (pid between 65536 and 18446744073709551615 and pid <> 17505437192229758416),
    epoch bigint,
    namespace_id text,
    dispatched_at timestamptz,
    delivered_at timestamptz,
    deadline_at timestamptz,
    finished_at timestamptz,
    next_attempt_at timestamptz,
    last_error text,
    last_status text,
    event_at timestamptz,
    back_at timestamptz,
    silent boolean,
    reaped_at timestamptz,
    breakdown jsonb not null default '{}',
    revision bigint not null default 0,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (campaign_id, device_id, installation_id)
) partition by list (campaign_id);

create index campaign_nodes_node on campaign_nodes (device_id, installation_id);
create index campaign_nodes_deadline on campaign_nodes (deadline_at)
    where state in ('dispatching', 'dispatched', 'delivered');
create index campaign_nodes_backoff on campaign_nodes (next_attempt_at)
    where state in ('pending', 'backoff');
create index campaign_nodes_pid on campaign_nodes (pid) where pid is not null;

create table intended_processes (
    pid numeric(20, 0) not null check (pid between 65536 and 18446744073709551615 and pid <> 17505437192229758416),
    created_at timestamptz not null,
    campaign_id uuid,
    attempt integer check (attempt >= 1),
    device_id text collate "C" not null,
    installation_id text collate "C" not null,
    action_kind text not null check (action_kind in (
        'run_script', 'ensure_version', 'ensure_config', 'quarantine',
        'collect_facts', 'collect_file', 'stream_logs', 'interactive', 'reap'
    )),
    principal text not null,
    subject text not null,
    expires_at timestamptz not null,
    max_commands integer not null check (max_commands >= 0),
    default_shell_commands integer not null check (default_shell_commands >= 0),
    last_dispatched_at timestamptz not null,
    reap_requested_at timestamptz,
    reaped_at timestamptz,
    check ((campaign_id is null) = (attempt is null)),
    primary key (pid, created_at)
) partition by range (created_at);

create index intended_processes_reapable on intended_processes (device_id, installation_id, expires_at)
    where reaped_at is null;
create index intended_processes_open on intended_processes (device_id, installation_id, expires_at);
