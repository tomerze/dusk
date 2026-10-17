create table campaign_sweep_cursors (
    campaign_id uuid not null references campaigns on delete cascade,
    partition smallint not null,
    cursor jsonb not null,
    updated_at timestamptz not null default now(),
    primary key (campaign_id, partition)
);

alter table campaigns drop column sweep_cursor;
