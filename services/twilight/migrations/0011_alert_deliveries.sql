create table alert_deliveries (
    transition_id bigint not null references alert_transitions (id) on delete cascade,
    receiver text not null,
    alert_id bigint not null references alerts (id) on delete cascade,
    kind text not null,
    transition text not null,
    severity text not null,
    idempotency_key uuid not null default gen_random_uuid() unique,
    state text not null default 'pending' check (state in ('pending', 'delivered', 'failed')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    lease uuid,
    leased_by text,
    leased_until timestamptz,
    last_attempt_at timestamptz,
    last_error text,
    delivered_at timestamptz,
    created_at timestamptz not null default now(),
    primary key (transition_id, receiver)
);

create index alert_deliveries_due on alert_deliveries (receiver, next_attempt_at) where state = 'pending';
create index alert_deliveries_alert on alert_deliveries (alert_id, receiver, transition_id);
create index alert_deliveries_lease on alert_deliveries (lease) where lease is not null;
create index alert_deliveries_kind on alert_deliveries (receiver, kind, transition, last_attempt_at);

create table alert_receivers (
    name text primary key,
    sends_at timestamptz not null default '-infinity',
    paused_until timestamptz
);
