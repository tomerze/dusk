create table node_state_checks (
    topic text collate "C" primary key,
    identity text not null,
    checked_at timestamptz not null
);
