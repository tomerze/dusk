create table campaign_budgets (
    campaign_id uuid primary key,
    rate double precision not null check (rate > 0),
    burst double precision not null check (burst >= 1),
    tokens double precision not null default 0,
    refilled_at timestamptz not null default clock_timestamp()
);
