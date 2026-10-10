create table nodes (
    device_id text collate "C" not null check (device_id ~ '^[0-9a-f]{32}$'),
    installation_id text collate "C" not null check (installation_id ~ '^[0-9a-f]{32}$'),
    cert_fingerprint text collate "C",
    lifecycle text collate "C" not null default 'enrolled'
        check (lifecycle in ('enrolled', 'active', 'quarantined', 'retired', 'revoked')),
    lifecycle_reason text,
    lifecycle_changed_at timestamptz,
    country text collate "C",
    os_name text collate "C",
    os_version text collate "C",
    os_build text collate "C",
    dusk_version text collate "C",
    hardware_class text collate "C",
    tenant text collate "C",
    locale text collate "C",
    hostname text collate "C",
    impl text collate "C",
    target_arch text collate "C",
    facts jsonb not null default '{}' check (jsonb_typeof(facts) = 'object' and not facts ? 'dusk.device.id'),
    reported_version text collate "C",
    reported_config_hash text collate "C",
    reported_services text[],
    reported_at timestamptz,
    facts_namespace_id text,
    facts_read_at timestamptz,
    enrolled_at timestamptz,
    first_seen_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    primary key (device_id, installation_id)
);

create index nodes_cert_fingerprint on nodes (cert_fingerprint);
create index nodes_lifecycle on nodes (lifecycle, device_id, installation_id);
create index nodes_country on nodes (country, device_id, installation_id);
create index nodes_os_name on nodes (os_name, device_id, installation_id);
create index nodes_os_version on nodes (os_version, device_id, installation_id);
create index nodes_os_build on nodes (os_build, device_id, installation_id);
create index nodes_dusk_version on nodes (dusk_version);
create index nodes_dusk_version_semver on nodes (semver_key(dusk_version), device_id, installation_id);
create index nodes_hardware_class on nodes (hardware_class, device_id, installation_id);
create index nodes_tenant on nodes (tenant, device_id, installation_id);
create index nodes_locale on nodes (locale, device_id, installation_id);
create index nodes_hostname on nodes (hostname, device_id, installation_id);
create index nodes_impl on nodes (impl, device_id, installation_id);
create index nodes_target_arch on nodes (target_arch, device_id, installation_id);
create index nodes_reported_version on nodes (reported_version);
create index nodes_reported_version_semver on nodes (semver_key(reported_version), device_id, installation_id);
create index nodes_reported_config_hash on nodes (reported_config_hash);
create index nodes_facts on nodes using gin (facts);

create table devices (
    device_id text collate "C" primary key check (device_id ~ '^[0-9a-f]{32}$'),
    lifecycle text collate "C" not null check (lifecycle in ('active', 'retired', 'revoked')),
    lifecycle_reason text not null,
    lifecycle_changed_at timestamptz not null,
    actor text not null
);

create table node_presence (
    device_id text collate "C" not null,
    installation_id text collate "C" not null,
    online boolean not null default false,
    last_seen_at timestamptz,
    connected_at timestamptz,
    disconnected_at timestamptz,
    sessions jsonb not null default '[]',
    primary key (device_id, installation_id),
    foreign key (device_id, installation_id) references nodes on delete cascade
) with (fillfactor = 50);
