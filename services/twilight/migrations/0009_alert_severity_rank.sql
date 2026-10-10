create function alert_severity_rank(severity text) returns integer
language sql
immutable
strict
parallel safe
return case severity when 'critical' then 4 when 'high' then 3 when 'medium' then 2 when 'low' then 1 else 0 end;
