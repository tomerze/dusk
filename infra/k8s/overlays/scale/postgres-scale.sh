#!/bin/bash
set -euo pipefail

password_file=${DUSK_SCALE_PASSWORD_FILE:-/run/secrets/postgres-scale/password}

psql --variable ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres --set password_file="$password_file" <<'SQL'
\set scale_password `cat :'password_file'`
CREATE ROLE scale LOGIN PASSWORD :'scale_password';
ALTER ROLE scale SET default_transaction_read_only = on;
GRANT CONNECT ON DATABASE inventory TO scale;
\connect inventory
GRANT USAGE ON SCHEMA public TO scale;
CREATE FUNCTION public.grant_scale_report_tables() RETURNS event_trigger LANGUAGE plpgsql AS $$
DECLARE
    created record;
BEGIN
    FOR created IN
        SELECT object_identity FROM pg_event_trigger_ddl_commands()
        WHERE command_tag = 'CREATE TABLE' AND object_type = 'table' AND schema_name = 'public'
            AND object_identity = ANY (ARRAY['public.nodes', 'public.node_presence', 'public.campaigns', 'public.campaign_nodes', 'public.intended_processes'])
    LOOP
        EXECUTE format('GRANT SELECT ON %s TO scale', created.object_identity);
    END LOOP;
END
$$;
CREATE EVENT TRIGGER grant_scale_report_tables ON ddl_command_end WHEN TAG IN ('CREATE TABLE') EXECUTE FUNCTION public.grant_scale_report_tables();
SQL
echo "created the role scale, which reads what a scale run's report counts"
