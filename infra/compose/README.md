# The Dusk stack on one machine

`compose.yaml` runs the Dusk stack in Docker for development and evaluation:
nightfall, dawn, twilight, everything they depend on, and nodes that dial out
to nightfall. It needs Docker Engine 26.0 or newer and Docker Compose 2.26.0 or
newer (tested with Docker Engine 29.1.3 and Docker Compose 2.40.3).

From the repository root:

```bash
git submodule update --init vendor/capnproto
cd infra/compose
export GIT_REV=$(git rev-parse HEAD)
docker compose run --rm secrets-init
export DUSK_FLEET_TOKEN=$(docker compose run --rm --no-deps --entrypoint cat secrets-init /secrets/fleet-token/fleet-token)
docker compose build
docker compose up -d
```

On a machine you share, also export `CARGO_BUILD_JOBS` (Rust build
parallelism) and `DUSK_BUILD_CPUSET` (the CPUs the compiles run on, such as
`0-11`) before `docker compose build`.

| UI | Address | Sign in |
|----|---------|---------|
| twilight | http://127.0.0.1:8080 | `docker compose exec twilight twilight token create --name admin --role admin` |
| Grafana | http://127.0.0.1:3000 | `admin`, password from `docker compose exec grafana cat /run/secrets/grafana-admin/password` |
| SigNoz | http://127.0.0.1:8081 | after `docker compose --profile signoz up -d`: `admin@dusk.test`, password from `docker compose exec signoz cat /run/secrets/signoz-admin/password` |
| Mailpit | http://127.0.0.1:8025 | none: the alert mail twilight and Grafana send |
| Alert sink | http://127.0.0.1:8089/__admin/requests | none: the alert notifications twilight and Grafana post to their PagerDuty, Slack, Teams and webhook receivers |

Check the infrastructure in a project of its own, since the tests write test
data, then throw it away:

```bash
docker compose -p dusk-verify up -d --wait step-ca cert-renewer kafka postgres clickhouse ceph otel-collector vector grafana
docker compose -p dusk-verify --profile verify run --rm verify
docker compose -p dusk-verify down --volumes
```

Stop it, keeping every secret, all data and the nodes' identities:

```bash
docker compose stop
```

`docker compose down` keeps the secrets and data but not the nodes' identities.
`docker compose down --volumes` also deletes every secret, including the key
every device id is derived from.

Settings, the secrets, the Kubernetes overlays and what to back up are in
`docs/docs/stack/deploy.md`.
