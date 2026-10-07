FROM grafana/grafana:13.2.3
ENV GF_PATHS_PLUGINS=/usr/share/grafana/dusk-plugins \
    GF_PLUGINS_PREINSTALL_DISABLED=true \
    GF_ANALYTICS_REPORTING_ENABLED=false \
    GF_ANALYTICS_CHECK_FOR_UPDATES=false \
    GF_ANALYTICS_CHECK_FOR_PLUGIN_UPDATES=false \
    GF_NEWS_NEWS_FEED_ENABLED=false \
    GF_AUTH_ANONYMOUS_ENABLED=false \
    GF_USERS_ALLOW_SIGN_UP=false \
    GF_LOG_MODE=console \
    GF_LOG_CONSOLE_FORMAT=json \
    DUSK_POSTGRES_ADDRESS=postgres:5432 \
    DUSK_POSTGRES_USER=grafana \
    DUSK_POSTGRES_SSLMODE=disable \
    DUSK_POSTGRES_ROOT_CERTIFICATE= \
    DUSK_CLICKHOUSE_HOST=clickhouse \
    DUSK_CLICKHOUSE_PORT=9000 \
    DUSK_CLICKHOUSE_SECURE=false \
    DUSK_CLICKHOUSE_CA= \
    DUSK_CLICKHOUSE_USER=grafana
USER root
RUN mkdir -p "$GF_PATHS_PLUGINS" \
    && grafana cli --pluginsDir "$GF_PATHS_PLUGINS" plugins install grafana-clickhouse-datasource 4.22.1 \
    && chown -R grafana:root "$GF_PATHS_PLUGINS"
COPY infra/grafana/provisioning /etc/grafana/provisioning
COPY infra/grafana/dashboards /etc/dusk/grafana/dashboards
USER grafana
