create table revocations (
    id uuid primary key,
    credential_kind text collate "C" not null check (credential_kind in ('fleet_token', 'install_token')),
    credential text collate "C" not null check (length(credential) between 1 and 256),
    enrolled_after timestamptz,
    enrolled_before timestamptz,
    reason text not null,
    actor text not null,
    created_at timestamptz not null,
    matched bigint not null,
    revoked bigint not null default 0,
    cursor_device_id text collate "C" not null default '',
    cursor_installation_id text collate "C" not null default '',
    finished_at timestamptz
);

create index revocations_running on revocations (created_at, id) where finished_at is null;
create index revocations_created on revocations (created_at desc, id desc);
