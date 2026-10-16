create function murmur2(data bytea) returns integer
language plpgsql
immutable
strict
parallel safe
as $$
declare
    size integer := length(data);
    hash bigint := 2538058380 # length(data);
    word bigint;
    consumed integer := 0;
begin
    while consumed + 4 <= size loop
        word := get_byte(data, consumed)::bigint | (get_byte(data, consumed + 1)::bigint << 8)
            | (get_byte(data, consumed + 2)::bigint << 16) | (get_byte(data, consumed + 3)::bigint << 24);
        word := (word * 1540483477) & 4294967295;
        word := word # (word >> 24);
        word := (word * 1540483477) & 4294967295;
        hash := ((hash * 1540483477) & 4294967295) # word;
        consumed := consumed + 4;
    end loop;
    if size - consumed >= 3 then
        hash := hash # (get_byte(data, consumed + 2)::bigint << 16);
    end if;
    if size - consumed >= 2 then
        hash := hash # (get_byte(data, consumed + 1)::bigint << 8);
    end if;
    if size - consumed >= 1 then
        hash := ((hash # get_byte(data, consumed)::bigint) * 1540483477) & 4294967295;
    end if;
    hash := hash # (hash >> 13);
    hash := (hash * 1540483477) & 4294967295;
    hash := hash # (hash >> 15);
    return (hash - case when hash >= 2147483648 then 4294967296 else 0 end)::integer;
end
$$;

create function node_partition(device_id text, installation_id text, partitions integer) returns smallint
language plpgsql
immutable
strict
parallel safe
set search_path from current
as $$
begin
    return ((murmur2(convert_to(device_id || '/' || installation_id, 'UTF8')) & 2147483647) % partitions)::smallint;
end
$$;

create table node_partition_leases (
    partition smallint primary key check (partition >= 0),
    owner text,
    epoch bigint not null default 0,
    renewed_at timestamptz
);

do $$
declare
    partitions integer;
begin
    partitions := nullif(current_setting('twilight.node_partitions', true), '')::integer;
    if partitions is null or partitions < 1 or partitions > 32767 then
        raise exception 'twilight.node_partitions must be the partition count of the process results topic, between 1 and 32767; twilight migrate reads it from the topic';
    end if;
    insert into node_partition_leases (partition) select generate_series(0, partitions - 1);
    execute format('alter table nodes add column partition smallint not null generated always as (node_partition(device_id, installation_id, %s)) stored', partitions);
    execute format('alter table node_presence add column partition smallint not null generated always as (node_partition(device_id, installation_id, %s)) stored', partitions);
    execute format('alter table campaign_nodes add column partition smallint not null generated always as (node_partition(device_id, installation_id, %s)) stored', partitions);
end
$$;

create index nodes_partition on nodes (partition, device_id, installation_id);
create index campaign_nodes_partition on campaign_nodes (campaign_id, partition, state);
create index campaign_nodes_in_flight on campaign_nodes (partition, deadline_at)
    where state in ('dispatching', 'dispatched', 'delivered');
