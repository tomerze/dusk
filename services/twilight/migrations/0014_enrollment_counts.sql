create table enrollment_counts (
    signal text collate "C" not null check (signal in ('issued', 'refused', 'new_network', 'quota')),
    scope text collate "C" not null check (scope in ('fleet', 'credential', 'network')),
    name text collate "C" not null,
    hour timestamptz not null,
    count bigint not null,
    primary key (signal, scope, name, hour)
);

create index enrollment_counts_hour on enrollment_counts (hour);
