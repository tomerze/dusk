# Deploying the Dusk stack

This page is for whoever runs the Dusk stack: you bring up its services, keep
their secrets, size the machines under them and upgrade them. It covers the
local stack in Docker Compose, the Kubernetes overlays, every secret and who
creates it, the certificate authorities, what to back up, sizing, and the order
to upgrade in.

Everything below lives under `infra/` in the repository: `infra/compose/` for
the local stack, `infra/k8s/` for Kubernetes, and the Dockerfiles of the three
services next to their code (`services/nightfall/Dockerfile`,
`services/dawn/Dockerfile`, `services/twilight/Dockerfile`) plus
`infra/node/Dockerfile` for a node that dials out to nightfall.

## What runs

| Component | What it does | Image | Ports |
|-----------|--------------|-------|-------|
| nightfall | Terminates node mTLS, provisions nodes, serves nodes to dawn, writes the ledger | `dusk-nightfall` (built here) | 8443 fleet and provisioning, 8444 inner, 8445 relay, 9100 health and metrics |
| dawn | Drives nodes through nightfall for twilight | `dusk-dawn` (built here) | 8443 API, 9101 health and metrics |
| twilight | Campaigns, inventory, the UI | `dusk-twilight` (built here) | 8080 UI and API, 9102 health and metrics |
| step-ca | The CA that signs node certificates | `smallstep/step-ca:0.30.2` | 9000 |
| Kafka | Every event, the ledger, telemetry | `apache/kafka:4.2.2` (Strimzi in prod) | 9092 (9093 TLS in prod) |
| Postgres | twilight's inventory, SigNoz's metadata | `postgres:17.11-trixie` (CloudNativePG in prod) | 5432 |
| ClickHouse and Keeper | The queryable copy of events and telemetry | `clickhouse/clickhouse-server:26.8.19.9`, `clickhouse/clickhouse-keeper:26.8.19.9` | 8123, 9000, 9181 (8443 and 9440 TLS in prod) |
| Ceph RGW | The object store: ledger evidence, the Parquet lake, collected files | `quay.io/ceph/demo` pinned by digest (development only; Rook in prod) | 8080 (80 in prod) |
| OTel collector | Receives OTLP, scrapes the services, exports to Kafka | `otel/opentelemetry-collector-contrib:0.162.0` | 4317, 4318 |
| Vector | Kafka to ClickHouse, the lake and the evidence bucket | `dusk-vector` (built here on `timberio/vector:0.59.0-debian`) | 8686, 9598 |
| Grafana | Dashboards and alert rules over Postgres and ClickHouse | `dusk-grafana` (built here on `grafana/grafana:13.2.3`) | 3000 |
| SigNoz | Logs, traces and metrics UI over ClickHouse | `signoz/signoz:v0.145.0`, `signoz/signoz-otel-collector:v0.144.12` | 8080 |

## The local stack (Docker Compose)

The local stack is the whole Dusk stack and a few nodes on one machine, for
development and evaluation. It is not hardened: Kafka and ClickHouse run
without TLS, the object store is a single Ceph demo container whose users get
their keys on `radosgw-admin`'s command line, and certificates are issued by a
script.

You need Docker Engine 26.0 or newer and Docker Compose 2.26.0 or newer: the
stack mounts single subdirectories and files of volumes, which older releases
cannot. It was tested with Docker Engine 29.1.3 and Docker Compose 2.40.3. Building the images
compiles Rust, Go and the UI from source; the node image alone took 19 minutes
with `CARGO_BUILD_JOBS=3` on a 14-core machine.

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

`GIT_REV` names the revision the images are built from; the Rust builds refuse
to start without it. secrets-init generates every secret once, the fleet token
among them, and the node image compiles that token in: compose hands
`DUSK_FLEET_TOKEN` to the node build as the build secret `fleet-token`, so it is
never a build argument, an environment variable or part of the image's history.
The node binary in the image carries it, so whoever holds the image holds the
token. The node build stops when the token is missing.

On a machine you share, `CARGO_BUILD_JOBS` limits the Rust builds'
parallelism and `DUSK_BUILD_CPUSET` (a CPU list such as `0-11`) keeps the
compiles of nightfall, dawn, twilight and the node on those CPUs. Docker's own
`--cpuset-cpus` does not: BuildKit accepts it and ignores it. Those compiles
always run at idle CPU priority, so they take only the CPU time other programs
leave unused.

The first start takes a few minutes: secrets-init generates every secret,
pki-init builds the certificate authorities, kafka-init creates the topics,
clickhouse-schema creates the tables once the object store answers, and
ceph-init creates the buckets. `docker compose ps -a` shows the one-shot
services as exited with code 0 when they are done.

Three nodes start with the stack. Each dials out to nightfall, enrolls with the
fleet token compiled into its image and keeps its identity in its kvs
persistent store on a volume of its own. Run more with:

```bash
DUSK_NODES=10 docker compose up -d node
```

Open the UIs, published on 127.0.0.1 only:

| UI | Address | Sign in |
|----|---------|---------|
| twilight | http://127.0.0.1:8080 | `docker compose exec twilight twilight token create --name admin --role admin` prints a token |
| Grafana | http://127.0.0.1:3000 | user `admin`, password from `docker compose exec grafana cat /run/secrets/grafana-admin/password` |
| SigNoz | http://127.0.0.1:8081 | user `admin@dusk.test`, password from `docker compose exec signoz cat /run/secrets/signoz-admin/password` (needs the `signoz` profile, below) |

Nodes outside Docker reach nightfall on 127.0.0.1:8443. They must connect by
the names on its certificates, `fleet.dusk.test` and `provision.dusk.test`,
so point those names at 127.0.0.1 on that machine, and they must be built with
the stack's fleet token. The CA they need is in the `pki` volume at
`trust-anchors/fleet-server-ca.crt`:

```bash
docker compose cp pki-init:/pki/trust-anchors/fleet-server-ca.crt .
```

Operators reach dawn's API, and with it `/v1/connect`, `/v1/sh` and `/mcp`, on
127.0.0.1:9443. Its certificate names `dawn`, signed by the internal CA in the
`pki` volume at `trust-anchors/internal-ca.crt`. The shipped configuration
admits only twilight's certificate, so give dawn a tokens file with an operator
token first ([dawn's authentication](dawn.md#authentication-and-roles)), in a
`compose.override.yaml` next to `compose.yaml`, which `docker compose` reads on
its own:

```bash
token=$(openssl rand -hex 32)
cat > dawn-tokens.toml <<EOF
[[token]]
sha256 = "$(printf '%s' "$token" | sha256sum | cut -d' ' -f1)"
role = "operator"
subject = "operator"
EOF
cat > compose.override.yaml <<'EOF'
services:
  dawn:
    environment:
      DAWN__AUTH__TOKENS_FILE: /etc/dawn/tokens.toml
    volumes:
      - ./dawn-tokens.toml:/etc/dawn/tokens.toml:ro
EOF
docker compose up -d dawn
docker compose cp pki-init:/pki/trust-anchors/internal-ca.crt .
curl --cacert internal-ca.crt --resolve dawn:9443:127.0.0.1 \
  -H "Authorization: Bearer $token" https://dawn:9443/v1/help
```

`POST /v1/connect` then takes the node and the pid twilight's
`POST /api/v1/nodes/{device}/{installation}/sessions` answers with.

### Settings

Every setting is an environment variable read by `docker compose`:

| Variable | Default | What it sets |
|----------|---------|--------------|
| `GIT_REV` | none | The revision the images are built from. Required to build. |
| `DUSK_FLEET_TOKEN` | none | The fleet token compiled into the node image. Required to build the node image. |
| `CARGO_BUILD_JOBS` | `default` | Rust build parallelism. |
| `DUSK_BUILD_CPUSET` | none | The CPUs the compiles run on, as a list such as `0-11`. |
| `DUSK_DOMAIN` | `dusk.test` | The domain in nightfall's names: `fleet.<domain>`, `provision.<domain>`, `<namespace>.fleet.<domain>`. Set before the first start; the certificates are issued once. |
| `DUSK_NODES` | `3` | How many nodes run. |
| `DUSK_SUBNET` | `10.231.64.0/24` | The stack network. Change it when it overlaps a network you use. |
| `DUSK_CEPH_IP` | `10.231.64.10` | Ceph's fixed address, inside `DUSK_SUBNET`. |
| `DUSK_FLEET_PORT` | `8443` | Host port for nightfall. Set it empty for a random port. |
| `DUSK_TWILIGHT_PORT` | `8080` | Host port for twilight. Set it empty for a random port. |
| `DUSK_DAWN_PORT` | `9443` | Host port for dawn's API. Set it empty for a random port. |
| `DUSK_GRAFANA_PORT` | `3000` | Host port for Grafana. Set it empty for a random port. |
| `DUSK_SIGNOZ_PORT` | `8081` | Host port for SigNoz. Set it empty for a random port. |

### Running only the infrastructure

The services' images take the longest to build. To work on the infrastructure
alone, start what the services depend on and leave them out:

```bash
docker compose up -d --wait step-ca kafka postgres clickhouse ceph otel-collector vector grafana
```

### Checking the infrastructure

The `verify` profile runs the infrastructure's tests from inside a stack:
topics and their configuration, ClickHouse's tables and the ledger chain view,
the Postgres roles, every bucket and its policy, step-ca's certificate policy,
messages from Kafka to ClickHouse and the evidence bucket, OTLP through the
collector, and every Grafana datasource, dashboard and alert rule.

The tests write: they produce every contract example to its Kafka topic, put
objects into the buckets, have step-ca sign node certificates and create
scratch databases in ClickHouse. A nightfall reading those topics would refuse
to continue the ledger chain an example ends, and twilight would take the
examples for real nodes and results. So the tests run in a compose project of
their own, `dusk-verify`, holding the dependency tier alone, and they stop
when started in any other project or next to nightfall, dawn or twilight.
Throw the project away afterwards:

```bash
docker compose -p dusk-verify up -d --wait step-ca cert-renewer kafka postgres clickhouse ceph otel-collector vector grafana
docker compose -p dusk-verify --profile verify run --rm verify
VERIFY_LAKE=1 docker compose -p dusk-verify --profile verify run --rm verify
docker compose -p dusk-verify down --volumes
```

The third command also waits for the Parquet lake, which Vector writes in
batches of up to five minutes. While the stack itself runs on the same
machine, give the verify project its own network and Grafana port by setting
`DUSK_SUBNET=10.231.65.0/24`, `DUSK_CEPH_IP=10.231.65.10` and
`DUSK_GRAFANA_PORT=` (empty for a random port) for every command above.

### SigNoz

```bash
docker compose --profile signoz up -d
```

SigNoz uses the stack's ClickHouse and keeps its own metadata in the `signoz`
Postgres database. Its ingester reads `dusk.otel-logs`, `dusk.otel-spans` and
`dusk.otel-metrics` from Kafka as consumer group `signoz`, from the earliest
offset still in Kafka. The image is SigNoz's own `signoz/signoz`, which
includes code under the SigNoz Enterprise License; only that build can keep its
metadata in Postgres.

SigNoz starts with its root account, `admin@dusk.test`, whose password is the
`signoz-admin` secret; SigNoz's UI cannot change or delete that account. The
address is `signoz.env.signoz_user_root_email` in
`infra/k8s/base/signoz/values.yaml` and `SIGNOZ_USER_ROOT_EMAIL` in the compose
file.

Log pipelines edited in SigNoz's UI do not reach the ingester: it runs from the
configuration in `infra/k8s/base/signoz/collector.yaml` and not under SigNoz's
remote management.

### Stopping

```bash
docker compose down
```

keeps every named volume, and the next `docker compose up -d` resumes with the
same secrets, certificates and data. The nodes are the exception: each keeps
its identity on an anonymous volume, which `docker compose up` creates afresh
for a container that `down` removed, so every node enrolls again as a new
installation and the inventory keeps a row for the old one. To keep the nodes,
stop and start the stack instead:

```bash
docker compose stop
docker compose start
```

```bash
docker compose down --volumes
```

destroys every secret, including the device-id key and the CAs. Every node
enrolled with this stack then has to be enrolled again and gets a new device
id.

## Kubernetes

`infra/k8s/base/` holds one directory per component; `infra/k8s/overlays/`
assembles them. Both overlays put the Dusk stack in the namespace `dusk`. Build them
with `kubectl kustomize` (kubectl 1.37.1 was used, with kustomize v5.8.1):

```bash
kubectl kustomize infra/k8s/overlays/dev
kubectl kustomize infra/k8s/overlays/prod
```

Every component has a NetworkPolicy, and the namespace starts from a policy
that denies everything but DNS. Each component then allows exactly its own
flows: nodes reach nightfall's 8443 from anywhere, dawn reaches nightfall's
8444, nightfall reaches other nightfall pods' 8445, twilight reaches dawn,
Postgres and Kafka, and so on. Two policies allow more than in-cluster traffic
and are yours to narrow: `bootstrap` lets the init jobs reach the Kubernetes
API on TCP 443 and 6443 anywhere, and `twilight-oidc` lets twilight reach public
addresses on 443 for its OIDC issuer and alert webhook. twilight's, Grafana's
and SigNoz's policies accept traffic from the `ingress-nginx` namespace; change
that selector if your ingress controller runs elsewhere. In the prod overlay,
dawn's accepts the ingress controller's pods on 8443 too, those that
`dawn-ingress-settings` names.

### The dev overlay

The dev overlay runs everything once, in one namespace, with small requests:
the in-repo Kafka, Postgres and Ceph instead of operators, and certificates
from the pki-init job. It fits a single-node kind cluster:

```bash
kind create cluster --name dusk
for image in dusk-nightfall dusk-dawn dusk-twilight dusk-vector dusk-grafana; do
  kind load docker-image --name dusk $image:dev
done
kubectl apply -k infra/k8s/overlays/dev
```

Build the images first, from the repository root (the compose file builds
them too). On a machine you share, add `--build-arg CPUSET=<cpu list>` to the
first three to keep their compiles on those CPUs:

```bash
docker build -f services/nightfall/Dockerfile --build-arg GIT_REV=$(git rev-parse HEAD) -t dusk-nightfall:dev .
docker build -f services/dawn/Dockerfile --build-arg GIT_REV=$(git rev-parse HEAD) -t dusk-dawn:dev .
docker build -f services/twilight/Dockerfile --build-arg GIT_REV=$(git rev-parse HEAD) -t dusk-twilight:dev .
docker build -f infra/vector/Dockerfile -t dusk-vector:dev .
docker build -f infra/grafana/Dockerfile -t dusk-grafana:dev .
```

kind has no load balancer, so the `nightfall` Service stays `<pending>` and is
reached through its node port. Nodes outside the cluster dial that port on
the kind node that runs `nightfall-0` (the Service keeps
`externalTrafficPolicy: Local`, so only that node answers). Find both, then
build the node image with `FLEET` and `PROVISION` set to names that resolve
to that node, and run the nodes on the `kind` Docker network:

```bash
kubectl -n dusk get service nightfall -o jsonpath='{.spec.ports[0].nodePort}'
kubectl -n dusk get pod nightfall-0 -o jsonpath='{.status.hostIP}'
docker build -f infra/node/Dockerfile --build-arg GIT_REV=$(git rev-parse HEAD) \
  --build-arg FLEET=fleet.dusk.test:<node port> --build-arg PROVISION=provision.dusk.test:<node port> \
  --secret id=fleet-token,env=DUSK_FLEET_TOKEN -t dusk-node:dev .
docker run -d --network kind --add-host fleet.dusk.test:<host ip> \
  --add-host provision.dusk.test:<host ip> dusk-node:dev
```

The fleet token is the one in the cluster's `fleet-token` Secret, and the
node's `--ca` is the fleet-server CA in the `dusk-trust-anchors` ConfigMap
(see Node images below).

If `kind load docker-image` fails on an image with `content digest ... not
found` (Docker's containerd image store with a multi-platform image), import
that image into each kind node directly:

```bash
docker save --platform linux/amd64 <image> \
  | docker exec -i <kind node> ctr -n k8s.io images import --platform linux/amd64 -
```

To start the infrastructure alone, without nightfall, dawn and twilight:

```bash
kubectl apply -k infra/k8s/overlays/dev -l 'app.kubernetes.io/name notin (nightfall,dawn,twilight)'
```

On a kind cluster on a 14-core machine, the infrastructure with SigNoz was
ready 352 seconds after `kubectl apply`, most of it Ceph starting and
ClickHouse's lake views waiting for it. Reach the UIs with port forwarding:

```bash
kubectl -n dusk port-forward service/grafana 3000:3000
kubectl -n dusk port-forward service/twilight 8080:8080
```

The dev Ceph is the same demo container as in compose, its users' keys on
`radosgw-admin`'s command line included. It keeps its data in the pod: a
restarted container keeps it, a new pod starts empty and needs ceph-init again
(`kubectl -n dusk delete job ceph-init` and apply). Ceph is started with its
file-descriptor limit lowered to 1048576, because Ceph's monitor walks every
possible descriptor and kind's containerd allows two billion.

### The prod overlay

The prod overlay replaces the in-repo stateful services with operators. Install
them first; these versions were used to validate the overlay against their
custom resource definitions:

| Operator | Version | Used for |
|----------|---------|----------|
| Strimzi | 1.2.0 | Kafka 4.2.1 on three combined controller and broker nodes, the topics, one user per producer and consumer with the ACLs of the Kafka contracts |
| CloudNativePG | 1.30.1 | Postgres 17.11, three instances; twilight connects to `postgres-rw` |
| cert-manager | v1.21.2 | The internal and fleet-server CAs and nightfall's certificates, with its own approver turned off (`--controllers=*,-certificaterequests-approver`) |
| cert-manager csi-driver | v0.16.0 | dawn's and twilight's per-pod certificates, `urn:dusk:principal:<pod>`, valid 24 hours, requested as the pod's ServiceAccount (`--use-token-request`) |
| cert-manager approver-policy | | Which certificate requests in `dusk` are signed |
| Rook | v1.21.0 | Ceph 20.2.4 and the object store `dusk` in the namespace `rook-ceph` |
| An ingress controller | | twilight's Ingress, `twilight.dusk.example` with the TLS Secret `twilight-ingress-tls`, and dawn's, `dawn.dusk.example` (ingress-nginx's annotations) |

Before applying it, make it yours:

- Replace `dusk.example` with your domain in
  `infra/k8s/overlays/prod/dusk/certificates.yaml`,
  `infra/k8s/overlays/prod/dusk/certificate-policies.yaml`,
  `infra/k8s/overlays/prod/dusk/configuration/nightfall.toml`,
  `infra/k8s/overlays/prod/dusk/configuration/dawn.toml`,
  `infra/k8s/overlays/prod/dusk/patches/environment.yaml`,
  `infra/k8s/overlays/prod/dusk/patches/ingress.yaml` and
  `infra/k8s/overlays/prod/dusk/dawn-ingress.yaml`.
- Point the images at your registry: the `images` list in
  `infra/k8s/overlays/prod/dusk/kustomization.yaml` names
  `registry.dusk.example/<image>` with tag `0.1.0` for `dusk-nightfall`,
  `dusk-dawn`, `dusk-twilight`, `dusk-vector` and `dusk-grafana`; set each
  `newName` and `newTag` to where you pushed it.
- Review `infra/k8s/overlays/prod/rook-ceph/ceph-cluster.yaml`: it uses every
  unused device of every node.

Then:

```bash
kubectl apply -k infra/k8s/overlays/prod
```

A principal certificate is what nightfall and dawn trust: nightfall gives the
dawn role, which controls every node, to any `urn:dusk:principal:dawn-*`.
approver-policy signs one only for the pods that should hold it. The policy
`dusk-dawn` approves a `dawn-*` principal for the ServiceAccount `dawn` alone
and `dusk-twilight` a `twilight-*` principal for the ServiceAccount `twilight`
alone, each for 24 hours at most. What cert-manager's controller asks for on
behalf of the Certificates in `dusk` passes one policy per issuer (its
ServiceAccount is taken to be `cert-manager` in the namespace `cert-manager`;
change the RoleBinding `dusk-certificates` if yours differs):
`dusk-certificate-authorities` approves a CA certificate only from the
self-signed `dusk-bootstrap` issuer, so neither the internal nor the
fleet-server CA ever signs another CA; `dusk-fleet-server-certificates`
approves only `fleet.<domain>` and `provision.<domain>`; and
`dusk-internal-certificates` approves only the names of nightfall's inner and
admin listeners and of ClickHouse, and among principals only `admin-*`.
Whoever may create a Certificate in `dusk` can therefore obtain an `admin-*`
principal: keep that right to the operators of the stack. The
CertificateRequestPolicy resources were not validated against approver-policy's
custom resource definitions.

ClickHouse's clients reach it over TLS with a server certificate from the
internal CA, which each verifies: Vector on `https://clickhouse:8443`, Grafana
and SigNoz on the native protocol's 9440, and the schema job with a
clickhouse-client configuration. The network policies let them reach no other
ClickHouse port.

Inside the cluster, three flows stay in clear text in prod: objects between the
Dusk stack and Rook's gateway (collected files, the Parquet lake, the ledger
evidence copy, SigNoz's cold tier) go over HTTP to port 80, OTLP from dawn to
the collector is plain gRPC and HTTP, and the collector scrapes the services'
metrics ports over HTTP. No secret crosses any of them: S3 requests are signed
and carry no key, and OTLP and the scrapes carry no credential. The data itself
can be read by whoever can watch the pod network.

Operators reach dawn's API, and with it `/v1/connect`, `/v1/sh` and `/mcp`,
through the Ingress `dawn` at `https://dawn.<domain>`. The local-config
ConfigMap `dawn-ingress-settings` in
`infra/k8s/overlays/prod/dusk/dawn-ingress.yaml` holds its host, the namespace
the ingress controller runs in and the controller pods'
`app.kubernetes.io/name` label (`dawn.dusk.example`, `ingress-nginx`,
`ingress-nginx`); kustomize writes them into the Ingress, its Certificate, the
policy that approves it and dawn's network policy, and they are not applied as
a ConfigMap. TLS ends at the controller with the Secret `dawn-ingress-tls`,
which cert-manager issues from the internal CA under the policy
`dusk-dawn-ingress`, so operators trust the internal CA's `ca.crt`; point the
Certificate `dawn-ingress` at another issuer if they should trust another. The
controller opens its own TLS connection to dawn's 8443, so a client
certificate does not reach dawn through it: operators call with an OIDC access
token or a bearer token from dawn's tokens file
([dawn's authentication](dawn.md#authentication-and-roles)).

nightfall's LoadBalancer Service sends port 443 to its pods' 8443 with
`externalTrafficPolicy: Local`, so nightfall sees each node's own address,
which its per-address limits depend on. On AWS use a Network Load Balancer in
IP target mode for the same reason.

## Secrets

The secrets-init job (compose: the `secrets-init` service) generates every
secret on its first run and never replaces one that exists, so running it again
rotates nothing. In compose each secret is a directory in the `secrets` volume;
in Kubernetes it is a Secret of the same name in the `dusk` namespace.

| Secret | Holds | Read by |
|--------|-------|---------|
| `fleet-device-id` | `device-id.key`, the key every device id is derived from | nightfall |
| `fleet-token` | `fleet-token`, the token nodes enroll with; `fleet-tokens.toml`, its SHA-256 for nightfall | node image builds, nightfall |
| `fleet-install-token-keys` | `install-token-jwks.json`, keys of per-install tokens (empty) | nightfall |
| `fleet-provisioner` | `provisioner.jwk`, the key nightfall signs step-ca tokens with; `provisioner.pub.jwk`, its public half | nightfall; pki-init gives step-ca the public half |
| `step-ca-password` | `password`, the password step-ca's keys are encrypted with | step-ca, pki-init |
| `nightfall-ledger` | `ledger-signing.key` (Ed25519) and `ledger-param.key`; `ledger-verify-jwks.json`, the public key | nightfall; secrets-init reads it to derive `nightfall-ledger-verify` |
| `nightfall-ledger-verify` | `ledger-verify-jwks.json`, the public key of `nightfall-ledger`, alone | twilight and verifiers |
| `dawn-output-key` | `output.key`, the HMAC key of process output digests | dawn |
| `ceph-admin`, `ceph-vector`, `ceph-ledger-writer`, `ceph-dawn`, `ceph-reader`, `ceph-signoz` | `access-key`, `secret-key` of each object store user | ceph-init; Vector (`vector`, `ledger-writer`), dawn, ClickHouse (`reader`, `signoz`), SigNoz |
| `postgres-superuser`, `postgres-twilight`, `postgres-grafana`, `postgres-signoz` | `username`, `password`, `pgpass` | Postgres; twilight, Grafana, SigNoz |
| `clickhouse-admin`, `clickhouse-vector`, `clickhouse-grafana`, `clickhouse-signoz` | `password` | ClickHouse; the schema job, Vector, Grafana, SigNoz |
| `grafana-admin` | `password` | Grafana |
| `signoz-admin` | `password`, the password of SigNoz's root account | SigNoz |

pki-init adds, in Kubernetes:

| Secret or ConfigMap | Holds | Read by |
|---------------------|-------|---------|
| `step-ca` | ca.json, root and intermediate certificates, the encrypted intermediate key | step-ca, pki-init |
| `step-ca-root-key` | `root_ca_key`, the encrypted root key of the fleet-client CA | nobody; move it offline |
| `pki-ca` (dev) | The fleet-server and internal roots with their keys | pki-init |
| `nightfall-fleet-tls`, `nightfall-provision-tls`, `nightfall-inner-tls`, `nightfall-admin-tls`, `dawn-tls`, `twilight-tls`, `admin-tls` (dev) | `tls.crt`, `tls.key`, `ca.crt` | nightfall, dawn, twilight, operators |
| ConfigMap `dusk-trust-anchors` | `fleet-server-ca.crt`, `fleet-client-ca.crt`, `internal-ca.crt` | nightfall, dawn, twilight |

pki-init creates `step-ca-root-key` once, with step-ca, and nothing in the
Dusk stack reads it: no pod mounts it and no role the stack defines may get it.
step-ca needs only the intermediate key; the root key signs only a new
intermediate. Copy it somewhere offline, then
delete it from the cluster:

```bash
kubectl -n dusk get secret step-ca-root-key -o yaml > step-ca-root-key.yaml
kubectl -n dusk delete secret step-ca-root-key
```

It is encrypted with the password in `step-ca-password`; keep the two apart. If
pki-init stops between creating `step-ca-root-key` and `step-ca`, its next run
refuses to continue until you delete the orphaned `step-ca-root-key`.

In prod, cert-manager issues the `*-tls` Secrets (ClickHouse's `clickhouse-tls`
among them) and the CAs `fleet-server-ca` and `internal-ca`, and the Strimzi
user operator issues one Secret per Kafka user (`nightfall`, `dawn`,
`twilight`, `vector`, `otel-collector`, `signoz`). pki-init reads the two CA
Secrets once, to copy their certificates into `dusk-trust-anchors`; prod runs
no pki-renew CronJob. cert-manager renews a CA certificate a year before its
ten years run out, keeping its key: run pki-init again then (delete the
finished job and apply) so `dusk-trust-anchors` carries the renewed
certificate.

To use a secret of your own instead of a generated one, create it before the
first run of secrets-init, with the same name and keys.

## Node images

A node that dials out is built for one Dusk stack: its init script names
nightfall's fleet and provisioning addresses, and dusk_core compiles in the
fleet token it enrolls with. Build it from the repository root with the token
from the `fleet-token` Secret, passed as a build secret so that it is never a
build argument or recorded in the image's history:

```bash
export DUSK_FLEET_TOKEN=$(kubectl -n dusk get secret fleet-token -o jsonpath='{.data.fleet-token}' | base64 --decode)
docker build -f infra/node/Dockerfile --build-arg GIT_REV=$(git rev-parse HEAD) \
  --build-arg FLEET=fleet.dusk.example:443 --build-arg PROVISION=provision.dusk.example:443 \
  --secret id=fleet-token,env=DUSK_FLEET_TOKEN -t dusk-node .
```

Anyone who holds the image holds the fleet token. The node keeps its identity
in its kvs persistent store under `/var/lib/dusk`, so run it with a volume
there that lives as long as the installation, and with nightfall's
fleet-server CA at `/etc/dusk/fleet-server-ca.pem`.

## The certificate authorities

Three authorities never share a root:

- **fleet-server**: signs nightfall's fleet and provisioning certificates.
  Every node trusts only this CA (its `--ca`), so nodes keep working through
  any change on the server side except a new fleet-server key. Its key is
  never rotated: cert-manager's CA Certificate has `rotationPolicy: Never`.
- **fleet-client**: step-ca, which signs node certificates and nothing else.
  Its only provisioner is nightfall's JWK provisioner, and its certificate
  template refuses any request that is not exactly one device and one
  installation URI. Node certificates live 168 hours. step-ca's database, on
  its own volume, remembers every one-time token it accepted, so a token cannot
  be used twice. In production the intermediate key belongs in a KMS or HSM
  (`kms` in ca.json); the overlay does not configure one.
- **internal**: signs everything inside the cluster: nightfall's inner and
  admin listeners, dawn, twilight and operator principals. nightfall refuses a
  client certificate on its inner and admin listeners that carries a node
  identity, and dawn and twilight trust only this CA.

In compose, cert-renewer checks every hour and re-issues each service
certificate (nightfall's, dawn's, twilight's and the admin principal's) ten
days before its 30 days run out. In the dev overlay, the pki-renew CronJob does
the same every day at 03:17 in the controller manager's time zone.

## Backups

Back up, before anything runs in production and whenever it changes:

| What | Where | Why |
|------|-------|-----|
| `fleet-device-id` | Secret | Every device id is derived from it. With a new key every device becomes a new device, and the inventory, campaign history and ledger no longer match the machines they describe. It is never rotated; losing it cannot be undone. |
| `fleet-server-ca` (prod) or `pki-ca` (dev) | Secret | Every node trusts this CA. With a new one every node must be reinstalled with the new anchor. |
| `step-ca`, `step-ca-password`, and the offline copy of `step-ca-root-key` | Secrets | Every node certificate chains to this CA; without it nodes cannot renew and must re-enroll. The root key is needed only to sign a new intermediate. |
| step-ca's `database` volume | PersistentVolumeClaim | The one-time tokens step-ca accepted; losing it lets a token captured in the last five minutes be used again. |
| `nightfall-ledger` | Secret | Checkpoint signatures are verified with its public key (also in `nightfall-ledger-verify`); keep every public key ever used. |
| `inventory` database | Postgres | twilight's state: nodes, campaigns, alerts. |
| `dusk-ledger-evidence` | Object store | The evidence copy of the ledger. It is object-locked (COMPLIANCE, 400 days in prod), so it is its own backup against deletion; replicate it to another site against losing the site. |

ClickHouse and Kafka hold copies: ClickHouse's tables can be refilled from the
Parquet lake, and Kafka is a buffer with retention of days.

## Sizing

- **Connection tracking.** Each node holds one TCP connection to nightfall.
  Set `nf_conntrack_max` to at least twice nightfall's `max_sessions` on every
  node that runs nightfall, or exempt port 8443 from tracking with a NOTRACK
  rule.
- **File descriptors.** nightfall and dawn hold one descriptor per connection.
  nightfall clamps `max_sessions` to its descriptor limit minus 10 000 at start
  and logs the clamp; compose gives nightfall and dawn 1048576.
- **Memory.** The shipped requests (nightfall 2 GiB, dawn 1 GiB, twilight
  1 GiB) are a starting point, not a measurement. Measure memory per session
  under your own load and set nightfall's requests from it.
- **Kafka partitions.** `dusk.ledger` needs at least as many partitions as
  nightfall has replicas, since each replica writes its own partition: prod
  creates 32.

## Upgrades and rollout order

- Roll out the infrastructure first, then nightfall, then dawn and twilight,
  then nodes. nightfall must know the schemas a node speaks before the node
  arrives: its image carries every released schema bundle.
- nightfall rolls one pod at a time and drains each for up to `drain_seconds`
  (300 s) before stopping it; its pods have 360 s to terminate.
- A Kafka contract never changes in place: a changed message is a new
  `<topic>/v2`. Upgrade every consumer to accept both before any producer
  writes v2.
- Jobs cannot be changed once they exist. Before applying a new version, delete
  the finished init jobs (`kubectl -n dusk delete job --ignore-not-found
  secrets-init pki-init kafka-init clickhouse-schema ceph-init
  signoz-telemetrystore-migrator`; prod has no kafka-init); they run again and
  change only what is missing.
- SigNoz's Kubernetes manifests are rendered from its chart. To move to another
  version, change `--version` and render again:

  ```bash
  helm repo add signoz https://charts.signoz.io
  helm template signoz signoz/signoz --version 0.145.0 --namespace dusk --skip-tests \
    -f infra/k8s/base/signoz/values.yaml | grep -v '^\s*#' > infra/k8s/base/signoz/signoz.yaml
  ```
