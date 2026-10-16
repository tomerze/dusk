create table erasures (
    id uuid primary key,
    scope text not null check (scope in ('device', 'tenant')),
    device_id text collate "C" check (device_id ~ '^[0-9a-f]{32}$'),
    tenant text collate "C" check (tenant ~ '^[a-z0-9-]{1,63}$'),
    before timestamptz,
    reason text not null check (length(reason) between 1 and 1000),
    requested_by text not null,
    requested_at timestamptz not null,
    status text not null check (status in ('running', 'completed')),
    steps jsonb not null default '[]',
    retained jsonb not null default '[]',
    attempts integer not null default 0,
    last_error text,
    next_attempt_at timestamptz not null,
    updated_at timestamptz not null,
    completed_at timestamptz,
    check ((scope = 'device') = (device_id is not null)),
    check ((scope = 'tenant') = (tenant is not null)),
    check (scope = 'tenant' or before is null),
    check ((status = 'completed') = (completed_at is not null))
);

create index erasures_running on erasures (next_attempt_at, requested_at) where status = 'running';
create index erasures_device on erasures (device_id, requested_at desc) where device_id is not null;
create index erasures_tenant on erasures (tenant, requested_at desc) where tenant is not null;
create unique index erasures_one_running on erasures (scope, coalesce(device_id, tenant), (before is null)) where status = 'running';
