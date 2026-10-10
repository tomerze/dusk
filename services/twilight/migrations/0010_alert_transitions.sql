create table alert_transitions (
    id bigint generated always as identity primary key,
    alert_id bigint not null references alerts (id) on delete cascade,
    transition text not null check (transition in ('opened', 're_escalated', 'acknowledged', 'resolved')),
    severity text not null,
    actor text,
    at timestamptz not null,
    alert jsonb not null,
    hostname text,
    campaign_name text,
    routed_at timestamptz
);

create index alert_transitions_unrouted on alert_transitions (id) where routed_at is null;
create index alert_transitions_alert on alert_transitions (alert_id, id);

create function record_alert_transition() returns trigger
language plpgsql
as $$
declare
    snapshot jsonb;
    host text;
    campaign text;
begin
    if tg_op = 'UPDATE' and alert_severity_rank(new.severity) <= alert_severity_rank(old.severity)
        and (old.acknowledged_at is not null or new.acknowledged_at is null)
        and (old.resolved_at is not null or new.resolved_at is null) then
        return null;
    end if;
    snapshot := to_jsonb(new);
    select node.hostname into host from nodes node
        where node.device_id = new.detail ->> 'device_id' and node.installation_id = new.detail ->> 'installation_id';
    select named.name into campaign from campaigns named
        where named.id = case when new.detail ->> 'campaign_id' ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
            then (new.detail ->> 'campaign_id')::uuid end;
    if tg_op = 'INSERT' then
        insert into alert_transitions (alert_id, transition, severity, at, alert, hostname, campaign_name)
            values (new.id, 'opened', new.severity, new.time, snapshot, host, campaign);
        return null;
    end if;
    if alert_severity_rank(new.severity) > alert_severity_rank(old.severity) then
        insert into alert_transitions (alert_id, transition, severity, at, alert, hostname, campaign_name)
            values (new.id, 're_escalated', new.severity, new.last_seen_at, snapshot, host, campaign);
    end if;
    if old.acknowledged_at is null and new.acknowledged_at is not null then
        insert into alert_transitions (alert_id, transition, severity, actor, at, alert, hostname, campaign_name)
            values (new.id, 'acknowledged', new.severity, new.acknowledged_by, new.acknowledged_at, snapshot, host, campaign);
    end if;
    if old.resolved_at is null and new.resolved_at is not null then
        insert into alert_transitions (alert_id, transition, severity, actor, at, alert, hostname, campaign_name)
            values (new.id, 'resolved', new.severity, new.resolved_by, new.resolved_at, snapshot, host, campaign);
    end if;
    return null;
end
$$;

create trigger alerts_record_transition after insert or update on alerts
    for each row execute function record_alert_transition();
