create table api_sessions (
    id_sha256 bytea primary key check (length(id_sha256) = 32),
    csrf_sha256 bytea not null check (length(csrf_sha256) = 32),
    subject text not null check (length(subject) between 1 and 1000),
    name text,
    role text not null check (role in ('viewer', 'operator', 'admin')),
    authentication text not null check (authentication in ('oidc', 'dev')),
    created_at timestamptz not null,
    last_seen_at timestamptz not null,
    expires_at timestamptz not null
);

create index api_sessions_expires_at on api_sessions (expires_at);
create index api_sessions_last_seen_at on api_sessions (last_seen_at);
