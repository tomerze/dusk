create table api_login_key (
    id smallint primary key default 1 check (id = 1),
    key bytea not null check (length(key) = 32),
    created_at timestamptz not null default now()
);
