#!/bin/bash
set -euo pipefail

secrets=${DUSK_SECRETS_DIRECTORY:-/run/secrets}

psql --variable ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres --set secrets="$secrets" <<'SQL'
\set twilight_password `cat :'secrets'/postgres-twilight/password`
\set grafana_password `cat :'secrets'/postgres-grafana/password`
\set signoz_password `cat :'secrets'/postgres-signoz/password`
CREATE ROLE twilight LOGIN PASSWORD :'twilight_password';
CREATE DATABASE inventory OWNER twilight;
CREATE ROLE grafana LOGIN PASSWORD :'grafana_password';
ALTER ROLE grafana SET default_transaction_read_only = on;
CREATE ROLE signoz LOGIN PASSWORD :'signoz_password';
CREATE DATABASE signoz OWNER signoz;
REVOKE ALL ON DATABASE inventory FROM PUBLIC;
REVOKE ALL ON DATABASE signoz FROM PUBLIC;
GRANT CONNECT ON DATABASE inventory TO grafana;
\connect inventory
REVOKE ALL ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO grafana;
CREATE FUNCTION public.grant_grafana_reporting_tables() RETURNS event_trigger LANGUAGE plpgsql AS $$
DECLARE
    created record;
BEGIN
    FOR created IN
        SELECT object_identity FROM pg_event_trigger_ddl_commands()
        WHERE command_tag = 'CREATE TABLE' AND object_type = 'table' AND schema_name = 'public'
            AND object_identity = ANY (ARRAY['public.nodes', 'public.node_presence', 'public.campaigns', 'public.campaign_counters', 'public.campaign_events', 'public.campaign_nodes', 'public.alerts', 'public.ledger_chain_heads'])
    LOOP
        EXECUTE format('GRANT SELECT ON %s TO grafana', created.object_identity);
    END LOOP;
END
$$;
CREATE EVENT TRIGGER grant_grafana_reporting_tables ON ddl_command_end WHEN TAG IN ('CREATE TABLE') EXECUTE FUNCTION public.grant_grafana_reporting_tables();
SQL
echo "created roles twilight, grafana and signoz and databases inventory and signoz"
