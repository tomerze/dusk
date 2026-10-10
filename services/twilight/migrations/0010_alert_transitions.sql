create table alert_transitions (
    id bigint generated always as identity primary key,
    alert_id bigint not null references alerts (id) on delete cascade,
    transition text not null check (transition in ('opened', 're_escalated', 'acknowledged', 'resolved')),
    severity text not null,
    actor text,
    at timestamptz not null,
    routed_at timestamptz
);

create index alert_transitions_unrouted on alert_transitions (id) where routed_at is null;
create index alert_transitions_alert on alert_transitions (alert_id, id);

create function record_alert_transition() returns trigger
language plpgsql
as $$
begin
    if tg_op = 'INSERT' then
        insert into alert_transitions (alert_id, transition, severity, at) values (new.id, 'opened', new.severity, new.time);
        return null;
    end if;
    if alert_severity_rank(new.severity) > alert_severity_rank(old.severity) then
        insert into alert_transitions (alert_id, transition, severity, at) values (new.id, 're_escalated', new.severity, new.last_seen_at);
    end if;
    if old.acknowledged_at is null and new.acknowledged_at is not null then
        insert into alert_transitions (alert_id, transition, severity, actor, at)
            values (new.id, 'acknowledged', new.severity, new.acknowledged_by, new.acknowledged_at);
    end if;
    if old.resolved_at is null and new.resolved_at is not null then
        insert into alert_transitions (alert_id, transition, severity, actor, at)
            values (new.id, 'resolved', new.severity, new.resolved_by, new.resolved_at);
    end if;
    return null;
end
$$;

create trigger alerts_record_transition after insert or update on alerts
    for each row execute function record_alert_transition();
