create table alert_deliveries (
    transition_id bigint not null references alert_transitions (id) on delete cascade,
    receiver text not null,
    alert_id bigint not null references alerts (id) on delete cascade,
    idempotency_key uuid not null default gen_random_uuid() unique,
    state text not null default 'pending' check (state in ('pending', 'delivered', 'failed')),
    attempts integer not null default 0,
    next_attempt_at timestamptz not null default now(),
    leased_by text,
    leased_until timestamptz,
    last_attempt_at timestamptz,
    last_error text,
    delivered_at timestamptz,
    created_at timestamptz not null default now(),
    primary key (transition_id, receiver)
);

create index alert_deliveries_due on alert_deliveries (next_attempt_at) where state = 'pending';
create index alert_deliveries_alert on alert_deliveries (alert_id, receiver, transition_id);
