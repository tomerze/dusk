alter table intended_processes
    add column revision bigint not null default 1 check (revision >= 1),
    add column published_revision bigint not null default 0 check (published_revision >= 0),
    add column tombstoned_at timestamptz;

create index intended_processes_unpublished on intended_processes (created_at)
    where published_revision < revision;
create index intended_processes_untombstoned on intended_processes (expires_at)
    where tombstoned_at is null;
