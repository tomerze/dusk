---
name: stack
description: Use when working on the Dusk stack - nightfall, dawn, twilight or its web UI, the Kafka contracts, nodes that dial out, the compose stack, kind or the Kubernetes overlays under infra/ - or on the CI jobs and docs that cover them; when building, running or testing any part of the stack, or deciding how work reaches and runs on a node.
---

# The Dusk stack

The services dusk runs on Kubernetes, on the user's own infrastructure, to
manage a fleet of Dusk nodes from one place. Load `dusk-developer` and
`activate` first; this skill adds what is particular to the stack. The
user-facing map is `docs/docs/stack/index.md`; the security model is
`docs/docs/stack/security.md`.

## The words

The project is **dusk**. A **fleet** is the set of nodes dusk manages, never a
name for the server side. The services are **the Dusk stack**, in layers from
the top: the Dusk node; **Nightfall**, the security layer (nightfall, step-ca);
**Dawn**, the client layer (dawn); **Twilight**, the orchestration layer
(twilight and its UI); the **data layer** (Kafka, Vector, S3, ClickHouse as the
events DB, Postgres as the inventory DB, sinks the user connects to Vector);
the **observability layer** (the OTel collector, SigNoz, Grafana, the user's
own OTLP endpoint). Words about nodes joining the fleet - fleet token, a
node's fleet link, fleet-server CA - are fine. "Dusk Fleet" and "fleet
services" are never written: not in code, strings, docs or commit subjects.
Docs go in `docs/docs/stack/`; branches are `stack-*`.

## The rules this work follows

**The node is ready.** The user, verbatim: "dusk was built by someone smarter
than me. the code of the node is likely ready". Stack work builds on the
node's mechanisms - fixed pids and the namespace process table, the `sh`
program through the existing Python API, kvs, logs, cp, `fs_driver()` - and
never changes the node to suit a service. A node change the stack truly needs
is its own GitHub issue and pull request on `master` (`drive-issue`), never a
commit on a stack branch. When the node seems unable to do something, find how
it already does it; when it truly cannot, stop and ask.

**A process is a job.** The user, verbatim: "the correct approach here is to
use the shell program as already provided by the python api". There is no job
concept anywhere - no job program, no job ids, no capnp interface or
credential for work, no epoch carried in a command. Work on a node is a
process at a pid:

- twilight derives a campaign's pid: the first 8 bytes, big-endian, of
  `SHA-256("dusk-pid-v1" + campaign_id + "/" + device_id + "/" +
  installation_id + "/" + attempt)`, re-hashed with `/1`, `/2`, ... appended
  while it falls in the reserved range (below 65536, or a pid a `.capnp` names
  as a `const`, such as `sh.capnp`'s `defaultPid`). Other work gets a random
  pid. Either is recorded in `intended_processes` and written to the
  compacted topic `dusk.intended-processes` - dawn is called only once Kafka
  acknowledged it; a tombstone follows expiry or reap.
- dawn runs `ps` in the node's default shell - `dusk.Dusk(address, port, ...)`
  without `sh_server_pid` connects to `defaultPid`. The pid present, running
  or exited and not reaped, means `duplicate`: nothing runs again, ever.
- Otherwise `dusk.Dusk(address, port, sh_server_pid=<pid>, server_name=...,
  ca=..., certificate=..., key=...)` puts a shell server at that pid and
  `.sh(script)` runs the script in it; dawn reports `started` as soon as that
  shell is up. Once the script succeeded, failed or `ended`, dawn kills that
  shell from the default shell; the exited, unreaped process is the dedupe
  marker. After any other outcome the shell stays until twilight has the pid
  reaped.
- Reads (`kvs get`, `logs dump`), kills and reaps all run in the default shell;
  dawn starts no other shells. A broken stream is resolved from
  `logs dump --replay-only`: the shell's `sh_exec` span for the pid not ended
  means `running`, ended means `ended`. nightfall refuses a default-shell
  command while none of the node's intended processes is open, and one beyond
  their budgets (`reconcile.default_shell`); twilight's reconcile counts the same
  afterwards: `default_shell_without_intent` when none is open,
  `process_shape` beyond the budgets.
- A facts read, a file collection, a log stream or an interactive session runs
  in a shell at its own random pid, and dawn kills and reaps that pid itself
  when it is done.
- Reap is `kill <pid>` then `kill --signal 8 <pid>` from the default shell,
  asked by twilight only once no duplicate can come. A node restart empties the
  process table, so a resend after one runs again.
- nightfall admits a call only when an intended process of its node allows it -
  its pid, its deadline, its command budget - and refuses everything else with
  `denied: not intended` before forwarding (`docs/docs/stack/membrane.md`,
  Admission). A `Dusk.process` without a pid is refused, but the read-only
  `kvs bind` the `kvs` client starts for `kvs get`; a client-side argument
  builder that starts any other helper process does not work through dawn.
- nightfall attributes every call to the pid of the process at the root of its
  capability, the ledger carries `pid` and the intent of that pid, and twilight's
  reconcile holds the ledger against `intended_processes` as the check that
  admission held.

`docs/docs/stack/processes.md` is the reference.

**State of the node pull requests.** Every node change of the spec is a pull
request on `master`, and four of them are still in review there: connect mode,
`nightfall -c` (pull request #174, for issue #149), the fleet token and
`Dusk.fleetToken` (issue #143), sensitive kvs keys (issue #144) and persistent
kvs keys (issue #93); #174 is stacked on the other three. The stack integrates
on `master` plus those pull requests. On `master` alone, `nightfall_provisioning`
does not compile, and with it the `nightfall` package, CI's `rust` job and the
`dusk-nightfall` image: it takes `provision_capnp` from `dusk_program_nightfall`,
which only #174 puts there. A node built from `master` cannot dial out. The
docs say so where they describe the node's side. Remove this paragraph, and
those notes, once the four are merged.

**Everything else** - no comments in any language (Go directives, `#!` and a
Dockerfile `# syntax=` line are not comments), nothing personal (the Go module
is `dusk/services/twilight`, images are `dusk-<name>`, never the repository
owner's handle), names and user-facing strings listed for review, atomic
commits - is in `dusk-developer` and `atomic-commit`, and holds in YAML, TOML,
SQL, shell, Go and TypeScript exactly as in Rust.

## Where everything lives

| Part | Path | Language, package |
|------|------|-------------------|
| Node that dials out | `base/nightfall` (`nightfall -c`, #174), `artifacts/dusk_node` - node code, changed only by pull requests on `master` | Rust, `dusk_program_nightfall` |
| nightfall | `services/nightfall/` with `membrane/`, `ledger/`, `provisioning/`, `schemas/` | Rust, `nightfall`, `nightfall_membrane`, `nightfall_ledger`, `nightfall_provisioning` |
| TLS client | `dusk/src/dusk_connection`, `dusk/src/dusk_py` - node-side code too | Rust, Python extension `dusk` |
| dawn | `services/dawn/` | Python 3.14, uv project `dawn` |
| twilight | `services/twilight/` | Go, module `dusk/services/twilight` |
| twilight UI | `services/twilight/web/` | TypeScript, React, Vite, npm |
| Kafka contracts | `services/contracts/kafka/` | JSON Schema draft 2020-12 |
| Images | `services/{nightfall,dawn,twilight}/Dockerfile`, `infra/{node,vector,grafana}/Dockerfile` | build context: the repository root |
| Compose stack | `infra/compose/` | |
| Kubernetes | `infra/k8s/base/`, `infra/k8s/overlays/{dev,prod}` | kustomize |
| Vector, Grafana, ClickHouse schema | `infra/vector/`, `infra/grafana/`, `infra/k8s/base/clickhouse/schema/` | |
| Docs | `docs/docs/stack/` | Markdown, mkdocs |

Every Kafka message matches `services/contracts/kafka/<topic>.schema.json`; the
topics dawn writes are `dusk.process-results`, `dusk.process-output` and
`dusk.files`, and twilight writes `dusk.node-state` and
`dusk.intended-processes`. A `/v1` schema never changes once released: any change is a new
`<topic>/v2` schema, consumers first, producers after
(`services/contracts/README.md`).

rustls everywhere uses ring and never the process-default provider: rustls-family
dependencies without default features, every config built with
`builder_with_provider`, and the four `builder` methods in clippy
`disallowed-methods` in each crate's `clippy.toml`.

## Build, run, test

From the repository root unless a directory is named. A fresh worktree needs
`git submodule update --init vendor/capnproto` before any Rust build or image.

| Part | Command |
|------|---------|
| Contracts | `uv run --no-project --with jsonschema --with referencing python services/contracts/validate.py`; `uv run --no-project --with pytest --with jsonschema --with referencing pytest services/contracts/tests -q` |
| nightfall | `cargo check -p nightfall`; `cargo nextest run -p nightfall -p nightfall_membrane -p nightfall_ledger -p nightfall_provisioning`; `cargo run -p nightfall -- --config <file>`; `nightfall verify-ledger`, `nightfall ledger-hash` |
| nightfall, gated tests | Kafka: `NIGHTFALL_KAFKA_TEST_BROKERS=<host:port>` with `--run-ignored ignored-only`; step-ca: `NIGHTFALL_STEP_CA_TEST_URL`, `NIGHTFALL_STEP_CA_TEST_ROOT`, `NIGHTFALL_STEP_CA_TEST_PROVISIONER_KEY` (see `services/nightfall/provisioning/tests/step_ca.rs`) |
| Node that dials out | `DUSK_FLEET_TOKEN=<token> DUSK_NODE_KVS_PERSISTENT=<file> DUSK_NODE_INIT_SCRIPT="nightfall -c <host:port> --provision <host:port> --ca <pem>" cargo build -p dusk_node_bin`; it keeps its identity in persistent kvs keys, in the file `DUSK_NODE_KVS_PERSISTENT` names (`docs/docs/stack/nodes.md`). Needs pull request #174 and the node pull requests it is stacked on |
| dawn | in `services/dawn`: `uv run pytest` (builds the `dusk` extension through maturin first; `uv sync --locked --no-install-package dusk` then `.venv/bin/python -m pytest -q` skips it, as CI does); `uvx pyright -p services/dawn` from the root; markers `load` and `integration` are off by default |
| twilight | `go -C services/twilight vet ./...`; `go -C services/twilight test ./...`; integration: `go -C services/twilight test -tags integration ./...` (starts its own Postgres and Redpanda containers in Docker; `TWILIGHT_TEST_CONTAINER_PREFIX` names them, `TWILIGHT_TEST_LOGS=1` shows service logs) |
| twilight, run | `twilight serve --config <file> [--migrate]`; `TWILIGHT_DEV=1` gives an admin login on a loopback `listen` only; `twilight token create --name <name> --role admin` |
| twilight UI | in `services/twilight/web`: `npm ci`; `npm run dev` (mock API), `npm run dev:live`; `npm run lint` (eslint, then prettier --check), `npm run typecheck`, `npm test`, `npm run build` (to `dist/`); `go -C services/twilight build -tags ui ./cmd/twilight` embeds `dist/` |
| Images | `docker build -f <Dockerfile> --build-arg GIT_REV=$(git rev-parse HEAD) -t <image>:dev .`. The compiling images take `--build-arg CPUSET=<cores>` and `--build-arg CARGO_BUILD_JOBS=<n>`: BuildKit ignores `docker build --cpuset-cpus`, so the Dockerfiles compile under `taskset`, always at idle CPU priority (`chrt -i 0 nice -n 19`). The node image also needs `--build-arg FLEET=<host:port> --build-arg PROVISION=<host:port>` and the fleet token as a build secret, `--secret id=fleet-token,env=DUSK_FLEET_TOKEN` - never a build argument, which an image layer would keep |
| Compose | in `infra/compose`: `export GIT_REV=$(git rev-parse HEAD)`; `docker compose run --rm secrets-init`; `export DUSK_FLEET_TOKEN=$(docker compose run --rm --no-deps --entrypoint cat secrets-init /secrets/fleet-token/fleet-token)`, which compose passes to the node image as the `fleet-token` build secret; on a machine shared with the user, `export CARGO_BUILD_JOBS=2 DUSK_BUILD_CPUSET=0-9` (cores 10-13 stay the user's; the compiles inside the images run at idle CPU priority on their own); `docker compose build`; `docker compose up -d`. Check the infrastructure in a project of its own: `docker compose -p dusk-verify --profile verify run --rm verify`. `--profile signoz` adds SigNoz. `docker compose down --volumes` destroys every secret, the device id key included (`infra/compose/README.md`) |
| Kubernetes | `kubectl kustomize infra/k8s/overlays/dev` and `.../prod`; kind: `kind create cluster --name <name>`, `kind load docker-image --name <name> <image>:dev` per image, `kubectl apply -k infra/k8s/overlays/dev` (`docs/docs/stack/deploy.md`) |
| Docs | `uv run --no-project --with mkdocs --with mkdocs-material mkdocs build --strict -f docs/mkdocs.yml -d <scratch directory>`; CI runs `uv run mkdocs build --strict` in `docs/` |

## What checks it

- **CI** (`.github/workflows/ci.yml`): `go` (gofmt, vet, `go test -short`),
  `go-integration`, `ui` (lint, typecheck, vitest, build, `go build -tags ui`),
  `contracts`, `rust` (clippy `-D warnings` and nextest, both `--locked`, on the
  nightfall crates, `dusk_program_nightfall`, `dusk_connection`, `dusk_capnp`,
  `dusk_py`, `dusk_api_tests`, `dusk_py_tests`), `kustomize`, `docker` (every
  image, no push; the node image with a throwaway fleet token as its build
  secret), and dawn's own job. CI runs on pull requests and on pushes to
  `master` and `services`.
- **pre-commit**: gofmt and go vet on `services/twilight`, the UI's lint (needs
  `npm ci` in `services/twilight/web`), ruff and pyright on `services/dawn`.
- `Cargo.lock` is tracked and every image and CI build is `--locked`: a change
  that adds or bumps a Rust dependency commits the updated `Cargo.lock`.
- `mkdocs build --strict` fails while a page in the Dusk stack navigation is
  missing.

## Common mistakes

| Mistake | Instead |
|---------|---------|
| A job program, a job id, a capnp interface, a token or a Python method for running work once | A process at a pid: `ps` in the default shell, then the shell server at the pid through `sh_server_pid` and `.sh` |
| Running work again because its outcome is unknown | A pid already in `ps` is `duplicate`; nothing runs, and twilight decides what the row means |
| Reaping a pid whose row can still be resent | Reap only once the row is finished and the campaign's retry horizon has passed |
| A shell other than the default shell and the one at the pid | Reads, kills and reaps run in the default shell |
| A node change on a stack branch | Its own issue and pull request on `master` |
| "Dusk Fleet", "fleet services" | The Dusk stack; a fleet is the nodes |
| A Rust config built with `ServerConfig::builder()` | `builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))` - two providers in one build panic |
| Changing a released `/v1` schema | A `<topic>/v2` schema beside it |
| A topic created by a service | Only `kafka-init` (or Strimzi's `KafkaTopic` in prod) creates topics |
| The fleet token as a build argument, in a file in the image, or printed | The `fleet-token` build secret; `dusk_core` compiles it in and never prints it (#143) |
| `docker build --cpuset-cpus` to keep a compile off the user's cores | `--build-arg CPUSET=<cores>` (compose: `DUSK_BUILD_CPUSET`); `--cpuset-cpus` only on `docker run` |
| A test container with a fixed name, left running | A unique name, removed when the test ends; never touch the host's firewall, sysctl or systemd |
| A `# comment` in YAML, a `//` in Go or TypeScript | The explanation goes in the commit message |
