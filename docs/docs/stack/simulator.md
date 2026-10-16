# Simulating node links

`nightfall_simulator` holds many fleet links in one process, each one a node to
nightfall and to the rest of the Dusk stack. It is how the stack is loaded with
more nodes than there are machines: a link costs the simulator a TLS session
and a Cap'n Proto RPC system, not a node.

Each link does what a node's `nightfall -c` does on the wire:

* it dials nightfall's fleet address over TCP and does a full TLS 1.3
  handshake with a client certificate of its own, carrying its device and
  installation ids - like a node, which builds a new TLS configuration for
  every attempt, it never resumes a session;
* it serves a `Dusk` bootstrap over the link, with a random namespace id for the
  life of the process;
* it answers nightfall's heartbeats (`Dusk.time`), closes the link when
  nightfall sends no heartbeat for 90 seconds, and links again after the node's
  full-jitter backoff: a random wait up to 1 second, doubling up to 300
  seconds, reset once a link was heard from for more than a minute.

Where it differs from a node on the wire: a node resolves nightfall's name on
its own host for every attempt, while the simulator resolves it once at start,
then every 30 seconds and, at most once a second, after a failed connection,
for all of its links together. It puts the DNS load of one client on the
stack, not that of every link; each link dials one of the addresses the name
resolved to.

The bootstrap answers everything nightfall calls on an idle node
(`namespaceId`, `programs`, `time`, `dusk`) and what dawn calls to run work on
one, so twilight can dispatch campaigns to every simulated node:

| Call | Answer |
|------|--------|
| `Dusk.programs` | `sh`, `kvs`, `ps`, `kill` and `logs`, each with its program id and the git revision its crate was built from, all with the simulator's version, so nightfall picks a schema bundle for a simulated node as it does for a real one. |
| `Dusk.process` | A shell server at its pid (fixed or not), or the kvs process the shell client starts to scan keys. A pid that a live process holds answers that process, so a duplicate runs nothing twice. Any other program is refused. |
| `Dusk.run`, `Process.run` | Run the process; `Process.run` answers once it exits. |
| `Dusk.ps` | The processes the link was asked for, with their states. |
| `Dusk.kill`, `Dusk.waitpid` | The namespace's rules: signal 15 stops a process, 8 reaps an exited one, 7 sweeps a suspended one; a stopped process answers `process has exited`. |
| `ShPortal.sh` | When every statement is `ps`, `kill`, `kvs get` or `logs`, the answers those programs give: the `ps` record, the kill applied, the facts below by name, and an `sh_exec` span for every shell that ran a script. Any other script streams one value, `simulated`, and ends. |
| `KvsPortal.scan` | The ids of the facts below. |

A simulated node's facts are `dusk.version` (the simulator's version),
`dusk.impl` (`simulated`), `dusk.hostname` (`simulated-<simulator>-<link>`),
`dusk.target.arch`, `dusk.target.os`, `dusk.target.bits` and `dusk.device.id`,
so `impl == "simulated"` selects them in a campaign.

A link holds at most 256 live processes and, apart from them, 4096 exited
processes nobody reaped. The exited ones are the markers that keep a pid from
running twice, and as on a node they hold no live slot. Past either bound
`Dusk.process` answers `overloaded`: a simulated node refuses new work once
4096 of its processes wait to be reaped, where a real node keeps accepting it.
The simulator does not model a node's own limit on processes running at once.

## Identities

A link's certificate is minted inside the simulator from a CA of the
simulator's own, so a million links never go through step-ca and never enroll.
nightfall admits them because that CA is in its `fleet.client_ca` bundle beside
step-ca's root, which only the compose file and the overlay below put there: a
nightfall without them refuses every simulated link. step-ca's fleet-client key
never leaves step-ca. The bundle lets nightfall renew a simulated certificate
into a real one too, so whoever holds the simulator's CA key can make nodes
that nightfall trusts; it belongs on test stacks only.

The certificate looks like one step-ca issues: an empty subject, the URIs
`urn:dusk:device:<device id>`, `urn:dusk:installation:<installation id>` and,
with `--tenant`, `urn:dusk:tenant:<tenant>`, `clientAuth`, valid for
`--certificate-lifetime-hours` (168 by default) from a minute ago. It is minted
again on a link's next attempt once it is within an hour of expiring.

A link's device id is the simulator's index (`--simulator`) and the link's
number, each as 16 hex digits; its installation id is the first 16 bytes of the
SHA-256 of its device id, in hex. A simulator restarted with the same index
links the same nodes again, with new namespace ids, as nodes that restarted.
Simulators with different indexes simulate different nodes.

## One address, many nodes

Every link of one simulator comes from the same address, which nightfall's
per-address limits count as one: `handshake_failures_per_ip_per_minute` (30)
failed or timed-out TLS handshakes from it - handshakes that time out while
nightfall is saturated, say - put the whole simulator in the penalty box for
`penalty_seconds`, and its links back off. For a load run, list the
simulators' network in nightfall's `limits.per_ip_exempt_cidrs`, as for a large
NAT. Keep the ramp of all simulators together below
`limits.handshakes_per_second` (2000): connections beyond it are closed before
TLS, and those links back off too.

## On the local stack

`infra/compose/simulator.yaml`, given after `compose.yaml`, adds two services
and changes one. `simulator-ca` creates the simulator's CA once, in the
`simulator-ca` volume, and writes a fleet-client bundle of step-ca's root and
that CA; `nightfall` reads the bundle as its `fleet.client_ca`; and
`nightfall-simulator` holds the links. With the stack running:

```bash
cd infra/compose
docker compose -f compose.yaml -f simulator.yaml build nightfall-simulator
DUSK_SIMULATED_LINKS=5000 DUSK_SIMULATED_LINKS_PER_SECOND=500 \
  docker compose -f compose.yaml -f simulator.yaml up -d nightfall-simulator
curl -s http://127.0.0.1:9103/metrics | grep nightfall_simulator_links
```

The `up` recreates nightfall with the bundle, which closes the stack's node
sessions once; they link again. `docker compose up -d nightfall`, without
`simulator.yaml`, recreates it with step-ca's root alone.

| Variable | Default | What it sets |
|----------|---------|--------------|
| `DUSK_SIMULATED_LINKS` | `1000` | How many links the simulator holds. |
| `DUSK_SIMULATED_LINKS_PER_SECOND` | `200` | How many links start each second. |
| `DUSK_SIMULATOR_METRICS_PORT` | `9103` | Host port for the simulator's metrics. Set it empty for a random port. |

The simulated nodes take the tenant `dev`, the tenant of the local stack's fleet
token. `docker compose -f compose.yaml -f simulator.yaml stop
nightfall-simulator` closes every link. The CA's key stays in the
`simulator-ca` volume until `docker compose -f compose.yaml -f simulator.yaml
down --volumes` removes it.

## On Kubernetes

`infra/k8s/overlays/scale` is the dev overlay plus what a scale run needs; the
prod overlay never includes any of it.

* The `nightfall-simulator-ca` Job creates the simulator's CA once as the
  Secret `nightfall-simulator-ca`, and writes the ConfigMap
  `nightfall-simulator-trust`: step-ca's root from `dusk-trust-anchors`
  followed by that CA. nightfall reads the ConfigMap as its `fleet.client_ca`,
  and waits for it before it starts.
* The `nightfall-simulator` Job is indexed: each pod holds `--links` links as
  simulator `<its completion index>`, so raising `completions` and
  `parallelism` together adds pods of distinct nodes. A pod that fails is
  retried three times for its own index (`backoffLimitPerIndex`); the other
  pods keep their links. The pods spread across the cluster's nodes, one per
  node before two share one, and their NetworkPolicy lets them reach
  nightfall's 8443 and nothing else.

```bash
docker build -f services/nightfall/Dockerfile --target simulator --build-arg GIT_REV=$(git rev-parse HEAD) -t dusk-nightfall-simulator:dev .
kind load docker-image --name dusk dusk-nightfall-simulator:dev
kubectl apply -k infra/k8s/overlays/scale
kubectl -n dusk port-forward "$(kubectl -n dusk get pod -l app.kubernetes.io/name=nightfall-simulator -o name | head -n 1)" 9103:9103
```

The Job never completes: its pods hold their links until it is deleted
(`kubectl -n dusk delete job nightfall-simulator`). Each pod widens its
ephemeral port range to 1024-65535 with the `net.ipv4.ip_local_port_range`
sysctl, which Kubernetes allows without configuration.

## Flags

| Flag | Default | What it sets |
|------|---------|--------------|
| `--fleet` | required | nightfall's fleet address, `host:port`, resolved at start, every 30 seconds and after a failed connection. |
| `--server-name` | `fleet.dusk.test` | The name nightfall's fleet certificate is checked against, sent as SNI. |
| `--fleet-server-ca` | required | The CA nightfall's fleet certificate chains to. |
| `--issuer-certificate`, `--issuer-key` | required | The CA the links' certificates are signed with, and its unencrypted PKCS#8 key; nightfall must trust that CA as a fleet client CA. |
| `--tenant` | none | The tenant in every link's certificate. |
| `--certificate-lifetime-hours` | `168` | How long a link's certificate is valid. |
| `--links` | `1000` | How many links the process holds. |
| `--simulator` | `0` | The process's index, the first half of every device id. |
| `--links-per-second` | `100` | The ramp: link `n` starts `n / rate` seconds after the start. |
| `--threads` | one per available CPU | Threads holding the links, each a single-threaded runtime. |
| `--heartbeat-timeout-seconds` | `90` | Seconds without a heartbeat from nightfall after which a link is closed. |
| `--metrics-listen` | `0.0.0.0:9103` | Where Prometheus metrics are served. |

`NIGHTFALL_SIMULATOR_LOG` sets the log filter (`info` by default; `debug` logs
every link going up and down, and why).

## Metrics

| Metric | Labels | What it counts |
|--------|--------|----------------|
| `nightfall_simulator_links_configured` | | `--links`. |
| `nightfall_simulator_links_up` | | Links whose TLS handshake completed and that have not gone down. |
| `nightfall_simulator_handshakes_total` | `outcome` | TLS handshakes, `completed` or `failed` (refused, timed out or reset). |
| `nightfall_simulator_connect_failures_total` | | TCP connections that failed or timed out before TLS, and attempts made before nightfall's name resolved. |
| `nightfall_simulator_links_lost_total` | `reason` | Links that went down: `closed` by nightfall, `failed` with an error, or `silent` past the heartbeat timeout. |
| `nightfall_simulator_calls_total` | `method` | RPC calls the simulated nodes answered, by method (`Dusk.time`, `ShPortal.sh`, ...). |

## What a link costs

Measured on the local stack on 16 October 2026: release images, one nightfall
instance and one simulator with 10 threads, both containers on 10 of a
14-core machine's cores at the lowest CPU share while other builds ran on the
machine. Memory is the process's resident set (`VmRSS`) once every link was
up, less the process with no links; CPU is the process's user and system time
(`/proc/1/stat`) over the stated span.

| 20 000 links, `--links-per-second 1000`, load average 42 to 55 | |
|---|---|
| Ramp | every link up within 22 s |
| Simulator memory | 361 MiB, 18.5 KiB a link (7.3 MiB with none) |
| Simulator CPU to bring the links up | 22.6 s, 1.1 ms a link |
| Simulator CPU holding the links (heartbeats every 30 s) | 0.31 cores over 66 s |
| nightfall memory | 530 MiB, 27.2 KiB a session (32.9 MiB with none) |
| nightfall CPU to set the sessions up | 27.6 s, 1.4 ms a session |
| nightfall CPU holding the sessions | 0.45 cores over 66 s |
| Open files | one a link, on both sides |

CPU time grows with the load on the machine: an earlier run of 20 000 links
at a load average of 9 to 18 held them with 0.11 cores of the simulator and
0.16 of nightfall. That run's build still kept TLS session tickets, which cost
19.9 KiB a link; it also held 50 000 links, all up within 51 s, at 19.8 KiB a
simulator link and 25.0 KiB a nightfall session, and its simulator spent 1.6 ms
of system time on each of the last links, against 0.5 ms at 20 000, which fits
the kernel searching a fuller ephemeral port range for each new connection,
though it was not profiled.

A second try of 50 000 links at a load average of 50 to 63 shows what [one
address, many nodes](#one-address-many-nodes) warns of: starved of CPU,
nightfall timed out 2 890 handshakes and 4 436 session setups, put the
simulator's address in the penalty box and refused 91 165 connections from it
while the links backed off.

What bounds one simulator process, from those numbers:

* **Ephemeral ports.** Every link is a connection from the simulator's one
  address to the same nightfall address, so the port range bounds the links:
  64 512 with `net.ipv4.ip_local_port_range` set to `1024 65535`, as both the
  compose file and the scale overlay set it. Of these, it is the one a
  simulator reaches first. More links from one host take more addresses:
  more pods, or more containers.
* **Memory.** At 18.5 KiB a link, 64 512 links take about 1.1 GiB, an
  extrapolation from the runs above.
* **Open files.** One a link; the simulator raises its soft limit to the hard
  limit, 1 048 576 in the compose file.
* **Connection tracking.** Where the simulator reaches nightfall through a
  Kubernetes Service or Docker's network, each link is also an entry in the
  connection-tracking table of the host the simulator runs on, so that table
  (`net.netfilter.nf_conntrack_max`, 262 144 on the machine measured here)
  bounds the links of every simulator on one host together. The scale
  overlay spreads its simulator pods one per node for that reason.
* **CPU.** 1.1 to 2 ms to bring a link up, so one core brings up 500 to 900
  links a second; holding links took 5 to 16 thousandths of a core per
  thousand links, depending on the load.

So 50 000 links a simulator pod, as the scale overlay runs, sits below the
port bound, and a million links are 20 such pods, about 18 GiB of simulator
memory - extrapolated, not measured. On the nightfall side, the 25 to 27 KiB
an idle session cost here puts `fleet.max_sessions`' 100 000 sessions at about
2.5 GiB an instance, also an extrapolation: sessions with clients on them cost
more.
