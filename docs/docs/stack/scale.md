# Scale runs

This page is for the people who load-test the Dusk stack: how to drive
campaigns against many nodes at once, what to watch while they run, and how to
tell afterwards whether every process ran once and reported its result. The
nodes come from the [node-link simulator](simulator.md), which answers what
dawn asks of a node, plus a few real nodes as canaries. The last section holds
the numbers measured on one machine, which later changes to the stack are
compared against.

A scale run has three parts:

* **the load driver**, `twilight-scale load`, creates campaigns through
  twilight's API, each over its own range of simulated nodes, starts them and
  waits for them to finish;
* **the dashboard**, *Scale run* in Grafana, shows what the stack does
  meanwhile;
* **the report**, `twilight-scale report`, counts from twilight's intended
  processes, nightfall's ledger and `dusk.process-results` whether every
  process ran at most once and reported its outcome, and prints `PASS` or
  `FAIL`.

`twilight-scale` is built from `services/twilight/cmd/twilight-scale`, as the
`scale` target of twilight's Dockerfile, the image `dusk-twilight-scale`.

## The load driver

`twilight-scale load` names its campaigns `<run>/0`, `<run>/1`, and so on, and
refuses a run name that already has a campaign named that way, so two runs
are never judged as one. Campaign `i` selects the simulated nodes numbered
`first + i × n` to `first + (i + 1) × n - 1`, where `n` is
`--nodes-per-campaign`, by a range of device ids: a simulated node's device id
is its simulator's index and its link, 16 hex digits each
([Identities](simulator.md#identities)), so node number `g` is link `g mod L`
of simulator `g / L`, where `L` is `--links-per-simulator`. With
`--real-nodes`, each campaign also selects every node whose `impl` is not
`simulated`, so every real node twilight manages runs every campaign's script:
use it only on a stack whose real nodes are canaries.

twilight evaluates a node against its campaigns only once it has read the
node's facts, whatever the selector says, so a campaign over nodes that just
linked waits for their facts reads. Give a run nodes whose facts are read:
`SELECT count(*) FROM nodes WHERE facts_read_at IS NOT NULL` in the inventory
database counts them.

Every campaign runs its script once on each node (`run_script`, one attempt),
in one phase over all of its nodes, with gates that never pause it, so a run
measures how fast work goes out and comes back rather than what a gate
decides. A campaign completes once every node has a result and its phase has
baked for `--node-timeout-seconds`. While it waits, the driver prints the
campaigns' statuses and their rows by state every `--poll`. It exits with 0
when every campaign completed, 1 when one ended otherwise or the wait ran out,
and 2 when it could not create, start or read the campaigns.

| Flag | Default | What it sets |
|------|---------|--------------|
| `--twilight` | `http://twilight:8080` | twilight's address. |
| `--token-file` | required | A file holding a twilight API token with the operator role. |
| `--run` | `scale-<UTC time>` | The run's name, which the report takes. |
| `--campaigns` | `1` | How many campaigns to create. |
| `--nodes-per-campaign` | `1000` | How many simulated nodes each campaign selects. |
| `--links-per-simulator` | `1000` | The `--links` of every simulator. |
| `--first-node` | `0` | The simulated node the first campaign starts at, counted across simulators. |
| `--script` | `echo dusk-scale` | The script the campaigns run; repeat the flag to give the campaigns different scripts in turn. |
| `--rate` | `50` | Processes each campaign sends per second (`policy.rate.per_second`). |
| `--burst` | the rate, rounded up | Processes each campaign may send at once. |
| `--node-timeout-seconds` | `120` | How long one node's script may take, and how long the phase bakes. |
| `--real-nodes` | `false` | Also select every node that is not simulated. |
| `--wait` | `true` | Wait for every campaign to finish. |
| `--timeout` | `2h` | How long to wait. |
| `--poll` | `5s` | How often to print progress while waiting. |

A simulated node answers any script with one value, `simulated`, so the script
matters only on the real nodes.

## The report

`twilight-scale report --run <run>` reads:

* from twilight's inventory database, with the role `scale`, which may only
  read: the run's campaigns, their rows, the intended processes twilight
  recorded for them (`intended_processes`, reaps left out), and the nodes each
  campaign's selector matches and their presence;
* from ClickHouse, the results dawn wrote to `dusk.process-results` for those
  campaigns (reaps left out), the entries nightfall wrote to the ledger since
  the run's first campaign was created, and nightfall's
  `nightfall_ledger_queue_depth` in `dusk.otel_metrics`;
* from Kafka, how far `dusk.ledger` and `dusk.process-results` reach, to know
  whether ClickHouse's copies of them have caught up.

A run belongs to the campaigns named `<run>/` and a number. The report judges
a run only once nothing it can see may still change the count, in this order:

1. no row of the run is on its way (dispatching, dispatched, delivered or
   verifying);
2. every nightfall instance has reported an empty ledger queue since the run's
   last row finished, so every ledger entry of the run has reached Kafka. The
   report sees nightfall's queue only through that metric, as the OTel collector
   scraped it into ClickHouse;
3. it then reads, once, how far Kafka has committed each partition of
   `dusk.ledger` and `dusk.process-results`, and waits until ClickHouse holds
   every record up to there. Records written later do not move that target, so
   a stack that keeps writing does not keep the report waiting. For
   `dusk.ledger`, which nightfall writes in transactions, the last record is
   the one before the last transaction's commit marker; when a partition ends
   in an aborted transaction the report cannot tell, and waits.

Until all three hold it counts again every 10 seconds; when `--wait` runs out
first it exits with 2 and says what it waited for.

It then makes these checks. Each counts a node and a pid together, since a pid
is unique only on its node.

| Check | Fails when |
|-------|-----------|
| every campaign of the run finished | A campaign is still `running`, `paused` or a `draft`: a run is judged once it is over. |
| every campaign reached the nodes it selected while it ran | A campaign has fewer rows that ended `succeeded`, `failed` or `unknown` than there are nodes its frozen selector matches that were online while it ran - from `node_presence`'s last connect and disconnect, against the campaign's start and finish, so a node whose only connection during the campaign is no longer its latest is not counted. A campaign that was aborted, or that waited for facts it never read, fails here. It counts the whole selection, which every campaign covers once its last phase, always 100%, is open. |
| every intended process has a result | An intended process has no result: none of its results has a status other than `started`. A process that never reached dawn is left out and counted on the line - its row is `pending`, `cancelled` or `excluded` and says that twilight could not write its intent to Kafka (`intent_unpublished`) or that dawn turned the dispatch away (`dawn_busy`, `dawn_unreachable`, `dawn_rejected`), or its row `failed` because dawn refused it (`denied`) - as long as neither dawn nor the ledger shows it started. |
| every started process reported its outcome | A process that dawn reported `started`, or whose `Dusk.process` the ledger shows answered `ok`, has no result with `delivered` true that says how its script ended: `succeeded`, `failed`, `already_satisfied`, `denied`, `timed_out` or `error`. `running` and `ended` - the stream broke, and the node's logs show the script still running or ended without its outcome - and a later dispatch's `duplicate` do not count. |
| no pid was started twice on its node, by dawn's started results | dawn reported `started` more than once for one pid on one node. |
| no pid's Dusk.process was answered ok twice on its node, by the ledger | The ledger holds more than one `Dusk.process` answered `ok` for one pid on one node. A node answers `ok` for a new process and for one still at the pid alike, so this does not say a script ran twice: two dispatches each believed they started the process. |
| no command was forwarded twice under one pid, by the ledger | nightfall forwarded the same `ShPortal.sh` call - the same parameter hash - more than once under one pid on one node. A campaign's process runs its script once in its own shell, so a repeat is the script running twice. |

The counts of starts come from two services that do not share code: what dawn
says it did, and what nightfall saw pass. The report prints each failing
check's first ten examples, and exits with 0 on `PASS`, 1 on `FAIL` and 2 when
it could not count.

This is the report of a run seeded into Postgres, ClickHouse and Kafka by the
report's own test (`report_integration_test.go`), with one node each that has
no result, that started but whose stream broke, whose `Dusk.process` was
answered twice, whose script was forwarded twice, and that never reached dawn,
counted while records kept arriving in both topics. Before it judged, it
waited once for ClickHouse:

```text
dusk.ledger holds dusk.ledger partition 0 up to offset 9 of 24; dusk.process_results holds dusk.process-results partition 0 up to offset 7 of 12; counting again in 10s, until 11:03:44
```

and then printed:

```text
run scale-test: 1 campaign, 6 intended processes on 6 nodes
  campaign scale-test/0 (completed): 6 rows (succeeded 3, unknown 2, cancelled 1); rows created over 0 s
PASS  every campaign of the run finished: 1 of 1 (completed 1)
FAIL  every campaign reached the nodes it selected while it ran: 5 reached of 6 selected
        campaign scale-test/0 reached 5 of the 6 nodes it selected that were online while it ran
FAIL  every intended process has a result: 4 of 5, leaving out 1 that never reached dawn: twilight could not write their intent to Kafka, or dawn turned their dispatch away
        device 00000000000000000000000000000002 installation bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb pid 17000000000000000002: no result (dawn's started results: 0, ledger's Dusk.process results: 0)
FAIL  every started process reported its outcome: 3 of 4
        device 00000000000000000000000000000005 installation eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee pid 17000000000000000005: started (dawn: 1, ledger: 1), no outcome from the dispatch that started it (results: running)
PASS  no pid was started twice on its node, by dawn's started results: 0 pids
FAIL  no pid's Dusk.process was answered ok twice on its node, by the ledger: 1 pid
        device 00000000000000000000000000000003 installation cccccccccccccccccccccccccccccccc pid 17000000000000000003: 2 Dusk.process results
FAIL  no command was forwarded twice under one pid, by the ledger: 1 pid
        device 00000000000000000000000000000004 installation dddddddddddddddddddddddddddddddd pid 17000000000000000004: one ShPortal.sh forwarded 2 times
verdict: FAIL
```

The campaign lines are information, not checks: how many rows each campaign
has in each state, and how fast its rows were created, dispatched and
finished, from the rows' own timestamps.

The test starts Postgres, ClickHouse and Redpanda containers, so it runs only
when asked, with both build tags:
`go -C services/twilight test -tags integration,scale ./cmd/twilight-scale`.

| Flag | Default | What it sets |
|------|---------|--------------|
| `--run` | required | The run's name, as given to `load`. |
| `--postgres` | `postgres://scale@postgres:5432/inventory?sslmode=disable` | twilight's inventory database. The report reads it in a read-only transaction, with a role that may only read. |
| `--clickhouse` | `http://clickhouse:8123` | ClickHouse's HTTP address. |
| `--clickhouse-user` | `grafana` | The ClickHouse user that reads `dusk.ledger` and `dusk.process_results`. |
| `--clickhouse-password-file` | none | A file holding that user's password. |
| `--kafka` | `kafka:9092` | Kafka's brokers, separated by commas, in plaintext: the report checks the copies only against a Kafka that needs no authentication, as the local stack's and the scale overlay's. |
| `--wait` | `2m` | How long to wait for rows on their way and for ClickHouse's copies to catch up. |

## The dashboard

*Scale run* (uid `dusk-scale`) reads what the stack already exports: the
services' metrics in `dusk.otel_metrics`, `dusk.process_results` and
`dusk.ledger` in ClickHouse, and small tables of the inventory database. Its
rows:

| Row | Panels |
|-----|--------|
| Campaigns | rows of the running campaigns by state; final results per second by status |
| twilight | dispatches per second by outcome; row transitions; results applied; which instance leads; the evaluation queue; online nodes; calls to dawn and their mean time; intended processes written to Kafka |
| Kafka and outboxes | twilight's consumer lag; how far behind nightfall's intended processes and node states are; alert notifications waiting in twilight's outbox |
| nightfall | sessions; handshakes; admission refusals; calls by result; mean ledger commit time; the ledger queue; intended processes held |
| dawn | final results per second; node sessions; Kafka produce failures |
| Ledger and Postgres | ledger entries per second by kind; connections to the inventory database by role; lock requests waiting, and the database's commit and row counters |

No panel scans `campaign_nodes`: the rates come from twilight's counters.
Only the overlay and the compose file below add the dashboard to Grafana; it
lives in `infra/k8s/overlays/scale/grafana-scale.json`.

## On the local stack

`infra/compose/simulator.yaml` adds the dashboard to Grafana and two services
under the `scale` profile, `scale-load` and `scale-report`, which run
`twilight-scale` with the right addresses and credentials; the flags you give
`docker compose run` are added to theirs. `scale-load` passes `--real-nodes`,
because the local stack's nodes are its canaries. The report reads with the
role `scale`, which Postgres creates when it initializes its database with
`simulator.yaml` given; a database initialized without it lacks the role, and
`docker compose -f compose.yaml -f simulator.yaml down --volumes` and a new
stack create it. With the stack and
the simulator running ([On the local stack](simulator.md#on-the-local-stack)):

```bash
cd infra/compose
docker compose -f compose.yaml -f simulator.yaml --profile scale build scale-load
export DUSK_SCALE_TOKEN=$(docker compose exec -T twilight twilight token create --name scale --role operator)
docker compose -f compose.yaml -f simulator.yaml --profile scale run --rm scale-load \
  --run scale-1 --campaigns 4 --nodes-per-campaign 5000 --rate 100
docker compose -f compose.yaml -f simulator.yaml --profile scale run --rm scale-report --run scale-1
```

`scale-load` passes `--links-per-simulator` from `DUSK_SIMULATED_LINKS`, the
variable that sets the simulator's links, so the campaigns number the nodes
the way the simulator does; keep `--first-node` plus campaigns times nodes per
campaign at most the links the simulator holds. Open Grafana
([the local stack](deploy.md)) and the *Scale run* dashboard in the *Dusk*
folder.

## On Kubernetes

`infra/k8s/overlays/scale` adds the dashboard to Grafana and two suspended
Jobs, `scale-load` and `scale-report`, whose settings come from the ConfigMap
`twilight-scale`:

| Key | Default | Flag |
|-----|---------|------|
| `SCALE_RUN` | `scale-1` | `--run`, for both Jobs |
| `SCALE_CAMPAIGNS` | `5` | `--campaigns` |
| `SCALE_NODES_PER_CAMPAIGN` | `10000` | `--nodes-per-campaign` |
| `SCALE_LINKS_PER_SIMULATOR` | `50000` | `--links-per-simulator`, and the `--links` of the overlay's simulator Job, which reads the same key |
| `SCALE_RATE` | `100` | `--rate` |

`scale-load` reads its token from the Secret `twilight-scale-token`, which you
create. The report reads with the role `scale` (Secret `postgres-scale`),
Grafana's ClickHouse user, and Kafka; the overlay's NetworkPolicies let it
reach exactly those three. The overlay gives Postgres a second init script that
creates the role when it initializes its database; only the scale overlay and
the compose simulator file create it. On a cluster whose database was
initialized without it, create it once:

```bash
kubectl -n dusk delete job secrets-init --ignore-not-found
kubectl apply -k infra/k8s/overlays/scale
kubectl -n dusk wait --for=condition=complete job/secrets-init --timeout=5m
kubectl -n dusk rollout restart statefulset/postgres
kubectl -n dusk rollout status statefulset/postgres
kubectl -n dusk exec statefulset/postgres -c postgres -- /docker-entrypoint-initdb.d/20-scale.sh
```

secrets-init, run again, adds the Secret `postgres-scale` beside the ones it
keeps; Postgres, restarted, mounts it; the last line creates the role from
it.
With the overlay applied and the simulator's links up:

```bash
docker build -f services/twilight/Dockerfile --target scale -t dusk-twilight-scale:dev .
kind load docker-image --name dusk dusk-twilight-scale:dev
kubectl -n dusk create secret generic twilight-scale-token \
  --from-literal=token="$(kubectl -n dusk exec deploy/twilight -c twilight -- twilight token create --name scale --role operator)"
kubectl -n dusk patch job scale-load --type merge -p '{"spec":{"suspend":false}}'
kubectl -n dusk logs -f job/scale-load
kubectl -n dusk patch job scale-report --type merge -p '{"spec":{"suspend":false}}'
kubectl -n dusk logs -f job/scale-report
```

For another run, give `SCALE_RUN` a new name - the load refuses one that
already has campaigns - delete both Jobs and apply the overlay again: it
creates them suspended.

## Baseline: the unmodified stack in one region

These are the numbers of the Dusk stack as it was before any scale-out work,
taken on 16 October 2026 at `services` revision `c0d3ed923`. Later changes are
measured the same way and compared with them. Every number below was read
during these runs, and the reading it came from is named beside it; the
one figure that is an extrapolation says so and gives its arithmetic. Times
are UTC.

### How it was measured

* **Host.** One machine with 14 cores and 30 GiB of RAM (`/proc/cpuinfo`,
  `free`). Every container of the stack ran on 10 of the cores at the lowest
  CPU weight (`cpuset 0-9`, `cpu_shares 2`), while other work ran on the same
  cores: 15 containers of other projects, among them another compose stack's
  Kafka and ClickHouse, a second Vector process, Rust builds and a virtual
  machine (`docker ps`, `ps`). The load average read 32 to 86 during `b1` (`uptime`,
  four readings) and 22 to 100 in the `uptime` lines recorded at the start and
  end of the later runs; the kernel's CPU pressure (`/proc/pressure/cpu`,
  `some`) read 44 to 56% at 18:43, before `b1`, and 70% during `c1`. The
  numbers are therefore those of a starved
  stack: read the counts, ratios and failure modes as the result, and the
  absolute rates as lower bounds.
* **Stack.** `compose.yaml` with `simulator.yaml`, release images built from
  that revision: one nightfall, one dawn, one Kafka broker, Postgres 17,
  ClickHouse, Vector, the Ceph demo, and two twilight instances - compose runs
  one; a second, `twilight-1`, was added to measure the leadership and to kill
  a leader. Its containers' memory peaked at 5.8 GiB together during `b1`,
  5.9 GiB during `b3` and 5.9 GiB during `c1` (`docker stats`, summed per
  sample within each run).
* **Population.** One simulator holding 20 000 links (`--links-per-second
  500`) and the 3 real nodes of compose's `node` service, which every campaign
  also selected. For `c1` and the fault runs the simulator was started again
  with 3 000 links, for the reason given under [Facts reads](#facts-reads).
* **Load.** `twilight-scale load` with the parameters in the table of runs.
  Each run was made once.
* **Readings.** The report and the load driver's output of each run; the
  campaign rows' timestamps; the ledger and the results in ClickHouse; the
  services' metrics in `dusk.otel_metrics` (scraped every 15 s) and, where
  those lagged, the services' own `/metrics`; `docker stats` and the active
  sessions' wait events in Postgres's `pg_stat_activity`, sampled about every
  8 s (one `docker stats` over every container, the wait query, then a 3 s
  pause) - 53 samples within `b1`, 48 within `b3` (from 19:05:46, the first
  sample of that run) and 61 within `c1`, from which every per-run average
  below is taken; the services' logs; `pg_test_fsync` in the Postgres volume;
  `kafka-producer-perf-test` against a scratch topic.

The reports these runs printed came from the report as it was then. It has
changed since: `running` and `ended` no longer count as an outcome, it checks
that each campaign reached the nodes it selected, it waits for rows on their
way, for nightfall's ledger queues and for ClickHouse's copies to catch up, and
it leaves out processes dawn turned away. The last column counts each run the
current way, from the results recorded during it. Its coverage figures count
every node a campaign's range selected, on the assumption that each was online
during the campaign - all of the simulator's links and the 3 real nodes were up
- because the presence the current check reads was not recorded then.

| Run | Links | Campaign | What happened | Report then | Counted the current way |
|-----|-------|----------|---------------|-------------|-------------------------|
| `b1` | 20 000 | 5 000 simulated nodes, 1000/s (more than the stack can do), node timeout 120 s | Aborted after 7 minutes: 1 178 rows, 325 succeeded, 567 unknown, 286 cancelled (its report) | FAIL: 656 of 1 046 started processes without the outcome of the dispatch that started them; 30 pids whose `Dusk.process` the ledger shows answered `ok` twice | FAIL: those, plus the processes behind its 65 `running` results; and it reached 892 of the 5 003 nodes it selected |
| `b2` | 20 000 | 1 000 simulated nodes whose facts were not read, 4/s, 120 s | Aborted after 225 s: none of the simulated nodes had joined (its load output) | PASS | FAIL: it reached the 3 real nodes of the 1 003 it selected |
| `b3` | 20 000 | 574 simulated nodes whose facts were read, 2/s, 60 s | 157 succeeded in the first 200 s; then nightfall refused the real nodes' processes as not intended, which paused the campaign; aborted (its report and load output) | FAIL: 2 of 165 started processes without an outcome | FAIL: those 2 and any process whose dispatch reported `running`, which was not counted at the time; and it reached 166 of the 577 nodes it selected |
| `c1` | 3 000 | the same 574 nodes, 2/s, 60 s | Completed: 575 succeeded, 2 unknown - their streams broke when the ledger did not commit a call in time, so dawn reported them `running` (its report; the results) | PASS | FAIL: 2 started processes without an outcome, the two `running` |

### What limits it first

#### nightfall's ledger

Every call nightfall forwards that is not a streaming call waits until the
ledger transaction holding its entry has committed
([The ledger](ledger.md#hashes-and-the-chain)), and each instance writes its
ledger as one chain of Kafka transactions, one after another.

* With only the 3 real nodes linked, before any load, the first 91
  transactions took 101.1 ms at the fastest and 101.8 ms at the median
  (`nightfall_ledger_commit_seconds` in `dusk.otel_metrics`, its samples at 18:25). So every forwarded call
  waits about 100 ms on an idle stack, and one instance, whose transactions
  follow one another, commits at most about ten a second - arithmetic from that
  floor. The cause of the floor was not established.
* During `b1`, the instance committed 1 796 transactions in 330 s, 5.4 a
  second and 170 ms on average; the median read 119 and 125 ms and the 99th
  percentile 853 and 995 ms at 18:50:07 and 18:50:22
  (`nightfall_ledger_commit_seconds`). It carried 155 calls a second, and 8 463
  of the 66 372 calls it answered in the run's 7 minutes failed with
  `overloaded` because their entry was not committed within 5 s (`dusk.ledger`).
* A campaign's process made 15.7 ledgered calls at its own pid during `c1`
  (`dusk.ledger`): `Dusk.process`, `Dusk.run`, two `Process.portal`,
  `ShPortal.functions`, `ShPortal.sh`, the script's `Stream.send` and
  `Stream.done`, and 7.7 `Process.pid` on average. The calls dawn makes in the
  node's default shell for the `ps` before and the kill after are not
  attributed to a campaign in the ledger and are not counted here. dawn makes
  these calls one after the other, so a dispatch waits for a dozen
  transactions or more.
* 43% of the calls nightfall ledgered during `b1` (66.8 of 154.9 a second) and
  39% during `c1` were `Process.pid` (`dusk.ledger`): the keepalive of the
  `sh` client dawn uses, which calls it every half round trip - every 50 ms to
  10 s, and every 50 ms after a failure (`base/sh/src/client/shell/mod.rs`) -
  for as long as dawn holds the session.

#### Facts reads

twilight evaluates a node against its campaigns only once it has read the
node's facts (`evaluateBatch` in
`services/twilight/internal/engine/evaluation.go`), whatever the selector says.

* twilight had read 958 nodes' facts 13 minutes after the 20 000 links came up,
  about 1.2 a second, against the configured `engine.facts_per_second` of 50
  (the inventory database).
* In the 5 minutes before 18:43 it finished 354 facts reads while nightfall
  recorded 23 647 calls (`dusk.ledger`), a smoke test's dispatches among them:
  about 70 calls a read, 23 of them `Process.pid`.
* At 1.2 reads a second, the 20 000 nodes would have needed about 4.6 hours -
  an extrapolation: 20 000 / 1.2 = 16 700 s.
* In `b2`, none of the 1 000 simulated nodes whose facts were not read joined
  in 225 s; twilight's sweep logged 1 000 candidates and 1 000 not evaluated.
  That is why `c1` and the fault runs used 3 000 links, twilight having read
  most of their facts, and the 574 contiguous nodes whose facts were all read.
  Over those nodes twilight created a campaign's 577 rows in 1.4 s (`b3`'s
  report).

#### What overload does

`b1` asked for 1000 processes a second.

* twilight dispatched 3 436 times for 1 178 rows, 2.9 times a row (twilight's
  `/metrics`; the report). The results (ClickHouse) say why so many sends came
  back: 1 259 times dawn found no connection to the node within 10 s, 984 times
  the ledger did not commit a call in time, and 60 times dawn refused because
  256 connections were already being set up.
* 567 rows ended `unknown` (the report); the run's results include 502
  `duplicate` and 65 `running`.
* For 656 of the 1 046 processes the ledger shows started, the dispatch that
  started them never reported an outcome: dawn had given up on the connection
  while nightfall was still holding its calls, and nightfall forwarded them
  afterwards. The ledger shows no `ShPortal.sh` forwarded for any of the 656:
  their scripts never ran, and as one-shot rows they are not sent again.
* For 30 pids the ledger shows two `Dusk.process` answered `ok`: the first
  dispatch's `Dusk.process` and `Dusk.run` were answered after dawn had given
  up, and a later dispatch, finding nothing at the pid in its `ps`, sent
  `Dusk.process` again. The report found no `ShPortal.sh` forwarded twice under
  any of them, and the one traced by hand had exactly one.
  Why the later `ps` did not show the first process was not established.

#### Kafka

With 20 000 links, a 2 000-record `kafka-producer-perf-test` at 200 records a
second with `acks=all` saw a median latency of 999 ms (18:42); with 3 000
links, a 200-record test saw 124 ms (19:28). The Kafka container waited for
CPU 10 to 17% of the time at 18:43, before `b1`, and 16 to 21% at 19:11, during
`b3` (its `cpu.pressure`, `full`). Its broker timed out its own heartbeat at 19:08 and
19:48 (its log), and dawn's producer saw `NotLeaderForPartition` (dawn's log).
What followed at 19:08 is under [Faults](#faults).

#### twilight's leader

All 3 436 dispatches of `b1` came from the instance holding the leadership
lock: only it exported `twilight_dispatch_processes_total` (both instances'
`/metrics`). Its CPU averaged 29.5% during `b1`, 14.8% during `b3` and 21.9%
during `c1`; the other instance's, 1.8%, 1.6% and 1.2% (`docker stats`).

#### Postgres

Not a limit at these rates. In three samples during `b3`, at most 1 of
twilight's 29 pooled connections was active (`pg_stat_activity`). In the 24 of
`b1`'s 53 samples that found an active session, active sessions were counted on
CPU 33 times, waiting on `WALWrite` 18 times and `WalSync` 9 times, and on row
locks (`transactionid`, `tuple`) 6 times. `pg_test_fsync` measured 1 008 `fdatasync` a second in the
Postgres volume.

#### Vector

It used 233% of a core on average during `b1`, 131% during `b3` and 317%
during `c1` (`docker stats`), and its copies into ClickHouse fell behind: at
19:08 the newest metric in `dusk.otel_metrics` was 16 minutes old, its two
ledger consumers were 1 464 and 5 953 records behind and its results consumer
1 164 (`kafka-consumer-groups`).

#### An alert that load raises

twilight's reconcile raised the critical `ledger_chain_broken` alert, with
break `unsigned_tail`, 210 times between 18:29 and 19:08 (twilight's alerts):
while the simulator's links came up and the ledger was saturated, entries went
more than twice the 1 s checkpoint interval without a checkpoint covering them.

#### Against the predictions

The design of the scale-out work predicted, from the code and from Postgres
measured alone, where one region would serialize. Against those predictions,
on this host:

| Predicted | Measured |
|-----------|----------|
| One twilight instance does all per-node work | Confirmed: every dispatch came from the leader |
| Sequential row creation at about 325 rows/s | 577 rows in 1.4 s for nodes whose facts were read (`b3`'s report); nodes whose facts were not read are not evaluated until they are |
| The campaign's own row holds one campaign to about 600 dispatches/s | Not reached: `b1` dispatched 3 436 times in 7 minutes, about 8 a second; the counter rose by 821 in its busiest minute |
| Results consumed only by the leader | Not a limit: the results consumer's `twilight_kafka_consumer_lag` stayed at 0 during `b1` |
| About 25 000 ledgered calls/s per nightfall instance (an estimate) | 155 calls/s during `b1` and 207 during `c1` (`dusk.ledger`), at 5.4 transactions/s and at least 100 ms each; the first limit reached |
| Postgres connections | Not reached |

### Faults

Each fault run was `c1` again - the same 574 nodes and the 3 real nodes, 2
processes a second, a 60 s node timeout, 3 000 links - with one fault injected
while the campaign dispatched: 50 s after it started in `f1`, 76 s in `f2` and
75 s in `f3` (the load driver's output; the fault script's log). A kill is
`docker kill --signal KILL`.

| Run | Fault | What happened | Report then | Counted the current way |
|-----|-------|---------------|-------------|-------------------------|
| `f1` | Kill the twilight instance holding the leadership; start it again 60 s later | The other instance took the lock 0.22 s after the kill (the leadership row's `acquired_at`). 109 rows succeeded in the minute of the kill and 111 in the next (the results). Completed: 574 succeeded, 3 unknown (its report). | FAIL: 2 started processes without the outcome of the dispatch that started them, and 1 pid whose `Dusk.process` was answered `ok` twice. All three came between 19:46:25 and 19:49:08, 4.5 minutes after the kill, while dispatch stalled - a stall during which Kafka's broker timed out its heartbeat, at 19:48 | FAIL: 3 started processes without an outcome - the 2 and one `running` - and the pid answered twice |
| `f2` | Kill nightfall; start it again 20 s later | Every session on it ended. 59 dispatches came back `unreachable` and 3 `running`, all between 19:53:22 and 19:54:02 (the results); 115 rows succeeded in the next minute. Completed: 572 succeeded, 5 unknown (its report). | FAIL: 2 processes whose shell nightfall had started - `Dusk.process` and `Dusk.run` answered `ok` a second before the kill - were reported `unreachable` with `delivered` false; the resend 4 minutes later found the pid and reported `duplicate`, so their rows became `unknown`. Their script never ran (no `ShPortal.sh` in the ledger), and nothing runs it again | FAIL: 5 started processes without an outcome - the 2 and the three `running` |
| `f3` | Restart Kafka (`docker restart`, 11 s) | 544 dispatches came back `unreachable` between 20:10:18 and 20:16:51, 58 of them before the restart; 355 of the 544 because the ledger did not commit a call within 5 s (the results). 12 to 77 rows succeeded a minute until 20:17, then 119. nightfall's log shows no failed ledger transaction. Completed: 570 succeeded, 7 unknown (its report). | FAIL: 5 started processes without their dispatch's outcome, as in `f2`, none with its script forwarded | FAIL: 7 started processes without an outcome - the 5 and two `running` |

**A stall nobody injected.** At 19:08, during `b3`, Kafka's broker timed out
its own heartbeat, starved of CPU. What followed is the most serious failure
these runs found:

* nightfall's ledger failed every transaction from 19:08:45 on - `Flush
  error: OperationTimedOut`, retried as abortable 29 times by 19:27:54 (its log)
  - while Kafka answered other clients again (the 124 ms test at 19:28). The
  calls waiting on it failed with `overloaded`; at about 19:27 its ledger queue
  held 31 783 entries (`nightfall_ledger_queue_depth`).
* Its table of intended processes read 0 (`nightfall_intended_processes`), so it
  refused every process and every default-shell command as not intended -
  3 274 `process_without_intent` and 2 043 `default_shell_without_intent`
  refusals by then (`nightfall_admission_refused_total`), among them every
  facts read - and it could not record those refusals, because the ledger
  queue was full (its log).
* Vector's Kafka consumers lost their group assignments at 19:08 and had not
  rejoined at 19:23, so nothing reached ClickHouse (its log;
  `kafka-consumer-groups`).
* Neither recovered by itself. Vector was restarted at 19:24 and nightfall
  killed and started at 19:30. Afterwards nightfall held 4 472 intended
  processes (its `/metrics` at 19:31), and facts reads succeeded again: 64 in
  40 s, before 19:33 (twilight's log).

The entries nightfall held were in neither Kafka nor ClickHouse, so even a
copy that had caught up with Kafka would have missed them. That is why the
report also waits for every nightfall instance to report an empty ledger queue,
and exits 2 when it does not see one in time.

### What the baseline says

No run passes the current checks. `b2` fails because its campaign reached
none of the simulated nodes it selected. Every other run left started
processes whose outcome nobody knows, in two ways: dawn gave up on a node
while nightfall still held its calls behind the ledger, or lost the link when
nightfall died, and the resend found the pid and reported `duplicate` (`b1`,
`b3`, `f1`, `f2`, `f3`); or the stream broke while the script ran and dawn
reported `running` (`b1`, `c1`, `f1`, `f2`, `f3`). In the first way the ledger shows the script
never forwarded - 656 of 656 in `b1`, 2 of 2 in `f2`, 5 of 5 in `f3` - and the
one-shot row is `unknown`, never sent again. Separately, for 30 pids in `b1`
and 1 in `f1`, two dispatches each had `Dusk.process` answered `ok`. No
command was forwarded twice under one pid in any run. What limits throughput
first is nightfall's ledger, not twilight's leader: at least 100 ms per
transaction, transactions one after another, every call of a dispatch waiting
for its own, and 39 to 43% of those calls keepalives. A node also cannot join
a campaign until twilight has read its facts, which on this host went at about
1.2 nodes a second.
