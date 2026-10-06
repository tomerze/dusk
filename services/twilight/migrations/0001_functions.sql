create function semver_key(version text) returns numeric[]
language sql
immutable
strict
parallel safe
return (
    select case
        when parts is null then null
        else array[parts[1]::numeric, parts[2]::numeric, parts[3]::numeric, case when parts[4] is null then 1 else 0 end]
    end
    from (
        select regexp_match(
            version,
            '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
        ) as parts
    ) as matched
);

create function phase_bucket(device_id text, installation_id text, salt bytea) returns numeric
language sql
immutable
strict
parallel safe
return (
    select signed::numeric + case when signed < 0 then 18446744073709551616 else 0 end
    from (
        select ('x' || encode(substring(sha256(convert_to(device_id || '/' || installation_id || '/', 'UTF8') || salt) from 1 for 8), 'hex'))::bit(64)::bigint as signed
    ) as hashed
);
