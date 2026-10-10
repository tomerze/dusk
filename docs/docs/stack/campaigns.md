# Campaigns

This page is for the people who write campaigns: how to say which nodes a
campaign applies to, what it does to them and how fast and how carefully it
goes, what each node's state in a campaign means, and what to do when a
reconcile alert fires. Running twilight itself is covered in
[Running twilight](twilight.md).

A campaign is how you set what a fleet should be. It has three parts:

* a **selector**: which nodes it applies to, for example
  `country == "US" and os_name in ["debian", "ubuntu"]`;
* an **action**: what to do to each of them - run a script, bring a version or
  a configuration to a value, or quarantine the node;
* a **policy**: how - how many nodes per second, in which phases, with which
  health gates, timeouts and retries.

```json title="An ensure_version campaign"
{
  "name": "dusk 0.3.0 for US Debian and Ubuntu",
  "description": "Upgrades the agent, 1% first.",
  "selector": "country == \"US\" and os_name in [\"debian\", \"ubuntu\"] and dusk_version < \"0.3.0\"",
  "action": {
    "kind": "ensure_version",
    "version": "0.3.0",
    "script": "upgrade --to 0.3.0"
  },
  "policy": {
    "rate": {"per_second": 50, "burst": 100},
    "phases": [
      {"name": "canary", "percent": 1, "bake_seconds": 3600},
      {"name": "early", "percent": 10, "bake_seconds": 3600},
      {"name": "half", "percent": 50, "bake_seconds": 1800},
      {"name": "all", "percent": 100, "bake_seconds": 900}
    ],
    "gates": {"min_sample": 20, "max_failure_rate": 0.05, "max_silent_rate": 0.05, "silent_window_seconds": 600},
    "node_timeout_seconds": 900
  }
}
```

## Life of a campaign

A campaign is created as a **draft**, which can still be edited. Starting it
makes it **running**; from then on its definition cannot change.

| Status | Meaning |
|--------|---------|
| `draft` | Being written. Nothing is sent to any node. |
| `running` | Taking nodes and dispatching its work to them. |
| `paused` | Nothing new is sent - no first dispatch, no resend, no retry. Processes already on their way finish, their results are recorded and health gates are still judged. Bake time does not accrue. |
| `completed` | Finished, by itself or because an operator completed it. |
| `aborted` | Stopped by an operator. |
| `failed` | Stopped by its own policy: a health gate failed with `on_gate_failure: abort`. |
| `archived` | Removed: its per-node rows and events are dropped. Only a completed, aborted or failed campaign can be archived, and only an hour past its node timeout after its last dispatch. |

* **Start**: the selector is compiled once and frozen with the campaign, and
  the first phase opens. An `ensure_version` or `ensure_config` campaign whose
  selector overlaps a running campaign of the same kind (and, for
  `ensure_version`, the same `version_key`) is refused unless its policy sets
  `allow_overlap: true`; the refusal says how many nodes overlap, and
  `GET /api/v1/campaigns/{id}/overlap` answers the same for a draft before it
  is started.
* **Pause** and **resume**: a campaign paused by an operator resumes on request.
  One paused by a failing health gate resumes only with `override_gate: true`
  and a reason, which is recorded; until its phase passes, the gate then counts
  only the nodes dispatched after the resume. The gate is still judged while a
  campaign is paused: when it fails then, the campaign fails if
  `on_gate_failure` is `abort`, and otherwise stays paused and needs
  `override_gate` to resume, as if the gate had paused it. A campaign paused
  because nightfall denied one of its processes, or because dawn refused
  twilight's dispatch (`pause_kind: permission`), resumes on request; fix the
  role in nightfall's permissions, or twilight's principal in dawn's, first, or
  its next process is refused too.
* **Abort**: nothing new is sent; nodes that were waiting (`pending`, `backoff`,
  `excluded`, `conflict`) or being verified become `cancelled`; processes
  already on their way still have their results recorded.
* **Complete**: the same as abort, but the campaign counts as completed. Use it
  to end an `ensure_*` campaign whose fleet has converged.

A `run_script` or `quarantine` campaign completes by itself once its last phase
is open, twilight has swept every matching node of the open phases since that
phase opened, that phase has baked, its gate passes, and no node is waiting, on
its way or being verified. An `ensure_version` or `ensure_config` campaign never
completes by itself: after its last phase it keeps enforcing the desired state
on every matching node that connects - the UI shows it as converging - until
an operator completes or aborts it. A campaign with a `deadline` completes when
the deadline passes, and its waiting nodes become `cancelled`.

## Which nodes: the selector

A selector is an expression over the inventory's columns and the facts each
node reports. An empty selector matches no campaign: twilight refuses it, so a
campaign over the whole fleet says so, for example with `has(device_id)`.

### Fields

| Field | What it holds |
|-------|---------------|
| `device_id`, `installation_id` | The node's identity, 32 lowercase hex digits each. |
| `cert_fingerprint` | The SHA-256 of the node's current certificate. It changes on every renewal. |
| `lifecycle` | `enrolled`, `active`, `quarantined`, `retired` or `revoked`. |
| `country` | Two-letter country code, from the node's time zone. twilight does not look the node's address up in a GeoIP database. |
| `os_name` | `debian`, `ubuntu`, ... on Linux (the os-release id), the edition on Windows, the brand on Android, the system's name elsewhere. |
| `os_version` | The operating system's version. |
| `os_build` | The kernel release on Linux, `<build>.<revision>` on Windows. |
| `dusk_version` | The node's Dusk version. A version field (below). |
| `hardware_class` | `<arch>-<cores>c-<memory>g`, cores rounded down and memory in GiB rounded up to powers of two, e.g. `x86_64-8c-16g`. |
| `tenant` | The tenant the node's credential carried. |
| `locale` | The node's locale, e.g. `en_US.UTF-8`. |
| `hostname`, `impl`, `target_arch` | The node's host name, its Dusk impl (`nix`, `windows`, `std`) and its processor architecture. |
| `reported_version` | The version the node last reported for the version key a campaign watches. A version field. |
| `reported_config_hash` | The configuration hash the node last reported. |
| `facts["key"]` | Any fact the node reports, by its key, e.g. `facts["dusk.os.linux.os_release.version_id"]`. |

A node's facts are read when twilight first sees it, when it reconnects as a
new Dusk instance, and when they are more than a day old.

### Operators

| Form | Meaning |
|------|---------|
| `field == value`, `field != value` | Equal, not equal. |
| `field < value`, `<=`, `>`, `>=` | Ordering. |
| `field in [a, b, c]`, `field not in [a, b, c]` | One of, none of. |
| `has(field)` | The node has a value for the field, or reported the fact (whatever its value). |
| `semver(field) < "1.2.3"` | Compares the field as a semantic version. |
| `a and b`, `a or b`, `not a`, `( ... )` | Logic. `not` binds tighter than `and`, which binds tighter than `or`. |

Values are strings in double quotes with JSON escapes (`"US"`, `"café"`),
numbers (`4294967296`, `-1.5`, `2e9`; at most 100 characters, with an exponent
between -1000 and 1000) and `true`/`false`. A list holds values of
one type. Single quotes, `=` and `!` alone are refused with a message saying
what to write instead.

* **Columns are text.** They compare with strings only, byte by byte, so
  `os_build < "22631"` is a string comparison.
* **Facts keep their JSON type.** Numbers compare as numbers, exactly;
  strings compare byte by byte; booleans only with `==`, `!=`, `in` and
  `not in`. A fact of another type than the value it is compared with
  compares false, so `facts["dusk.device.memory_bytes"] >= 4294967296` is false
  on a node that reported the memory as a string.
* **Versions.** Ordering comparisons on `dusk_version` and `reported_version`,
  and any comparison wrapped in `semver(...)`, compare semantic versions:
  `1.10.0` is above `1.9.0`, a prerelease is below its release, every
  prerelease of one version compares equal to every other, and build metadata
  is ignored. A field that is not a semantic version compares false. `==` and
  `in` on a version field without `semver(...)` compare the exact string.

### Absent values

Logic is two-valued: any comparison with a field the node has no value for is
**false**, whatever the operator - `country != "US"` is false on a node whose
country is unknown, and so is `country == "US"`. So `not` selects the nodes
where a field is absent: `not country == "US"` matches every node outside the
US *and* every node whose country is unknown. To leave unknown values out, say
so: `has(country) and country != "US"`.

### Limits and errors

A selector is at most 16384 characters, 64 levels of nesting, 1024 comparisons
and 1024 values in one list. An invalid selector is refused with the position
of the mistake, counted in characters from 0, and what was expected there - for
example `unknown field "os"; use facts["os"] for a fact`.

### Who joins a campaign

A node joins a running campaign the first time twilight finds it matching the
selector while its phase bucket is in an open phase, either when the node
connects or when twilight sweeps the campaign. Membership is sticky: once a
node has a row in the campaign, changes to its facts never remove it. A waiting
node that stops matching becomes `excluded` and waits again if it matches
again; a node whose process is on its way, or that has finished, stays as it is.
Nodes that start matching after the campaign started join the phase of their
bucket.

A node's bucket comes from the SHA-256 of its device id, installation id and
the campaign's salt, drawn when the campaign is created. The same nodes are in
the canary however often the campaign is paused and resumed.

## What to do: the action

| Kind | Fields | What happens on a node |
|------|--------|------------------------|
| `run_script` | `script`; optional `collect_files` (up to 16 absolute paths) and `stream_logs` (`level`: `trace`, `debug`, `info`, `warn` or `error`; `duration_seconds` 1 to 86400) | The script runs once. Files are uploaded after it, and the node's logs are streamed to the collector for the duration. The node's state follows the script's result alone: a file that could not be collected does not fail a node whose script succeeded, and collected files do not make a failed script succeed. |
| `ensure_version` | `version`, `script`, `version_key` (default `dusk.version`) | If the node already reports `version` under `version_key` it succeeds without running anything; otherwise the script runs and the node must then report `version`. |
| `ensure_config` | `config_hash`, `script` | The script runs and the node must then report `config_hash` as `dusk.config.hash`. |
| `quarantine` | optional `script`, `require_script_success` | The script runs first if there is one and the node is online; then the node's lifecycle becomes `quarantined`, which nightfall enforces on its sessions. With `require_script_success` a failed script, or one whose result never came, leaves the node as it was; without it the node is quarantined either way. A quarantine reaches offline nodes too. A node that is `revoked` or `retired` keeps its lifecycle: its row succeeds with `last_status` `withdrawn` and nothing is sent to nightfall. |

Scripts are Dusk shell scripts, at most 256 KiB. Each attempt on a node is one
process, at a pid derived from the campaign, the node and the attempt, and dawn
starts it only when no process at that pid is in the node's process table. So
a node runs an attempt once however often twilight sends it: a resend of work
already delivered is reported `duplicate` and runs nothing. A node that
restarted has an empty process table, so a resend after a restart runs again.
twilight reaps an attempt's process only once no resend of it can come (see
[Running twilight](twilight.md#dispatch)).

`run_script` and `quarantine` are **one-shot**: a script that may have run is
never run again by itself. `ensure_version` and `ensure_config` are
**converging**: they judge a node by what it reports, not by how its script
ended, so a node that succeeds and later drifts away from the desired state is
brought back while it still matches.

## How: the policy

| Key | Default | Meaning |
|-----|---------|---------|
| `rate.per_second` | `10` | Processes this campaign sends per second, across the whole fleet. |
| `rate.burst` | `per_second`, rounded up | Processes that may be sent at once after a quiet period. The budget starts empty, also after a twilight failover. |
| `phases` | one phase, `all`, 100 percent | Up to 20 phases, each with a `name`, a cumulative `percent` (the last one 100, at most three decimals) and `bake_seconds`. The default phase bakes for the longer of `node_timeout_seconds` and `gates.silent_window_seconds`. |
| `gates.min_sample` | `1` | Results needed before the gate may pass, overall and per group; at least 1. While the open phases hold fewer nodes, the gate needs a result from each of them. |
| `gates.max_failure_rate` | `0.05` | The highest failure rate the gate passes. |
| `gates.max_silent_rate` | `0.05` | The highest silent rate the gate passes. |
| `gates.silent_window_seconds` | `600` | How long after its success a node may stay away before it counts as silent. |
| `gates.breakdown` | all four | Groups the gate is also judged in: any of `os_build`, `hardware_class`, `dusk_version`, `country`. `[]` judges only the whole campaign. |
| `abort.on_gate_failure` | `pause` | `pause` or `abort` when a gate fails. |
| `abort.max_total_failures` | none | Fail the gate once more nodes than this have failed. |
| `node_timeout_seconds` | `900` | How long one node's script may take. A `run_script` process that collects files or streams logs has the same time again for each file, plus `stream_logs.duration_seconds`, before its result is overdue. |
| `retry.max_attempts` | `3` for `ensure_*`, `1` for `run_script` and `quarantine` | Explicit failures a node may have before it is `failed`. |
| `retry.initial_backoff_seconds` | `60` | The wait after a node's first failure. |
| `retry.multiplier` | `2` | Each later wait is this many times the previous. |
| `retry.max_backoff_seconds` | `3600` | The longest wait. |
| `deadline` | none | An RFC 3339 time at which the campaign completes. |
| `allow_overlap` | `false` | Start an `ensure_*` campaign even though it overlaps a running one (see [Overlapping campaigns](#overlapping-campaigns)). |

A gate key you leave out takes its default; one you give, `0` included, is
kept, except that `min_sample` is at least 1. Every phase must bake for at least `node_timeout_seconds` and at least
`gates.silent_window_seconds`, so that a phase's results are in before it is
judged.

A wait after `n` failures is `initial_backoff_seconds × multiplier^(n-1)`, at
most `max_backoff_seconds`, and then a random point in its upper half, so nodes
that failed together are not retried together.

### Phases and health gates

A phase opens when the campaign starts or when the previous phase passes. A
node joins only while its bucket falls in an open phase, so the first phase is
the canary. A phase passes when all of these hold:

* it has baked for `bake_seconds`, counting from when it opened and leaving out
  the time the campaign was paused, so a pause or a twilight failover neither
  restarts nor skips a bake;
* enough results are in: `S + F` and `E` are each at least `min_sample`, or
  at least the number of nodes in the open phases when that is fewer, so a
  phase whose buckets hold no node passes on its bake alone and a phase with
  fewer nodes than `min_sample` needs a result from every node it has;
* the failure rate and the silent rate are at most their maximum, overall and in
  every breakdown group with at least `min_sample` nodes;
* twilight's online view is not degraded.

The gate counts every node in the open phases:

* **F**, failed: nodes whose latest attempt the node reported as `failed`,
  including those waiting to retry, plus nodes in `unknown` and nodes resolved
  as `failed`;
* **S**, succeeded, by their result or resolved as `succeeded`;
* **failure rate** = F / (S + F);
* **E**, eligible: nodes whose success is at least `silent_window_seconds` old;
* **Q**, silent: nodes in E that disconnected after their success and did not
  connect again within `silent_window_seconds`. A node that stayed connected is
  never silent;
* **silent rate** = Q / E.

A node twilight could not reach, a time-out before delivery, a denied process,
a dispatch dawn refused and a dawn `error` never count as failures: nothing is known to have failed on
the node. A dawn `error` after the process started still uses up one of the
node's `retry.max_attempts`.

Each group is a value of a breakdown dimension, copied from the node when it
was first dispatched, so a gate can say "failure rate 0.62 in
os_build=22631.4317, 41 of 66". The gate is judged every 15 seconds (twilight's
`engine.gate_interval_seconds`), and a failing gate pauses or fails the campaign before another process is sent. The
same transaction raises a high alert: `campaign_paused_by_gate` when it pauses
the campaign, which resolves when the campaign is resumed, aborted, completed or
fails, and `campaign_failed_by_policy` when it fails it; both carry the gate's
reason, the failing group, the tallies and the thresholds, and reach people
through twilight's [alert routes](twilight.md#alerts). A gate
that holds says why in the campaign's events: `waiting for sample 7 of 20`,
`waiting for silent-window sample 3 of 20`, or `the online view is degraded`.

### Overlapping campaigns

On one node, only one `ensure_version` campaign per `version_key`, and only one
`ensure_config` campaign, is in effect: the one that started first among those
whose selector matches the node. The node's rows in the others become
`conflict`, which counts in no rate, and a `campaign_conflict` alert is raised;
they wait again once nothing earlier claims the node.

## Each node's state

| State | Meaning | What to do |
|-------|---------|------------|
| `pending` | Waiting to be sent, or to be sent again, with the same pid, after something kept the process from reaching the node. | Nothing; it is sent when the node is online, the rate allows and any wait has passed. |
| `dispatching` | Recorded as an intended process, being handed to dawn. | Nothing. |
| `dispatched` | dawn accepted the process. | Nothing. |
| `delivered` | The process started on the node, or dawn found it already there (`duplicate`) or still running (`running`). | Nothing. |
| `verifying` | The script succeeded, but the node has not yet reported the desired state (`ensure_*`), or its quarantine is being recorded. An `ensure_*` node is judged on the facts read in its next session. | Nothing. A node that still differs then counts as a failed attempt. |
| `backoff` | The node reported a failure and waits to be retried with a new attempt. | Nothing, or look at `last_error`. |
| `succeeded` | Done. For `ensure_*`: the node reports the desired state. | Nothing. |
| `failed` | Failed `retry.max_attempts` times, or was denied by nightfall, or its dispatch was refused by dawn (`last_status` `denied` either way), or was resolved as failed. | Read `last_error` and `last_status`; retry it when the cause is fixed. |
| `unknown` | A one-shot process reached the node but no result came back before its time-out, or dawn saw its script end without learning how (`ended`). It may or may not have done its work, so twilight does not run it again. | Find out what happened on the node, then resolve it as succeeded or failed, or retry it. |
| `excluded` | The node no longer matches the selector. | Nothing; it waits again if it matches again. |
| `conflict` | An earlier `ensure_*` campaign owns this node. | Nothing, or complete the other campaign. |
| `cancelled` | The campaign was aborted, completed or reached its deadline before this node was done: its process was not sent, or not sent again, or the node was still being verified. | Nothing. |

A node that was never reached is sent again whenever it is next online, with no
limit, because nothing failed. A node that keeps failing stops after
`retry.max_attempts`.

**Retry** moves `failed`, `unknown` and `cancelled` nodes of a running or paused
campaign back to `pending` with a new attempt and a new pid. **Resolve**
closes `unknown` nodes as `succeeded` or `failed` by hand; a `quarantine` node resolved as `succeeded` is quarantined first. Both take a reason,
recorded in the campaign's events, and at most 10 000 nodes at a time.

## Reconcile alerts

nightfall writes every call it forwards to a node into its ledger, with the pid
of the process the call works under (`0` for a call under no process).
twilight's reconcile checks each one against the processes twilight intended -
the `intended_processes` table, written before every call to dawn - and raises
an alert when something reached a node that twilight did not ask for. These
are the most important alerts in the system: a process with no campaign or
operator behind it means a stolen client certificate, someone using the SDK
directly, or a compromised dawn.

Each alert's detail names the pid, the node, the principal - the dawn instance
or operator certificate the call came from - and the ledger instance,
partition and sequence of the entry, so you can find it in the ledger. One
alert stays open per fingerprint; repeats count in its `occurrences`.

| Alert | Severity | What happened | What to do |
|-------|----------|---------------|------------|
| `process_without_intent` | critical | A client created a process (`Dusk.process`) at a pid twilight never intended for that node. The node's default shell, where dawn reads, kills and reaps, is never one. | Treat the principal's certificate as compromised: deny it in nightfall's permissions, revoke it, and find who holds it. |
| `default_shell_without_intent` | critical | A shell command (`ShPortal.sh`) ran in the node's default shell while no process twilight intended for that node was open. dawn runs commands there only around work twilight asked for. | As above. |
| `target_mismatch` | critical | A call under a pid reached a different node than its intended process names. | As above; also check the node it reached. |
| `result_without_ledger` | critical | dawn reported a process started at a pid, but the ledger shows no `Dusk.process` for that pid within ten minutes. | Check the dawn instance named in the detail: it reported work the ledger does not show. Check also that reconcile is not lagging (`twilight_reconcile_lag_seconds`). |
| `ledger_chain_broken` | critical | The ledger's hash chain or checkpoint signatures do not verify: an entry was edited, removed, reordered or forged, a chain started without naming the previous one, or an entry went more than twice nightfall's checkpoint interval without a checkpoint after it. | Run `nightfall verify-ledger` on the partition named in the detail and compare it with the object-locked evidence copy. An `unknown_key` break after a nightfall key rotation means `reconcile.ledger_keys` lacks the new key. |
| `process_after_deadline` | high | A call under a pid arrived after its intended process expired. Killing and reaping it (`Dusk.kill`, `Dusk.waitpid`) is not counted. | Check the clocks of the nightfall instance and twilight, then the principal. |
| `pid_reused` | high | A process at one pid was created on more than one client session, and twilight had not sent it again since it was first created. | Check whether the dawn instance reconnected during the work; if not, treat the principal as compromised. |
| `process_after_result` | high | Calls arrived under a pid more than a minute after its final result, and twilight had not sent it again. | Check the dawn instance: it kept using a finished process. |
| `process_shape` | high | A process ran more shell commands (`ShPortal.sh`) in its own shell than it was intended to: its script, one `cp` per collected file and one `logs stream` for a campaign's process, `reconcile.commands_per_session` for an interactive session. Or the node's default shell ran more shell commands than the processes open at the time allow together (`reconcile.default_shell`). Each time twilight sends a campaign's process again, both allowances grow by one more run of it. | Read the pid's calls in the ledger, or the default shell's for the window in the detail; something ran more than the work. |
| `quarantine_override` | high | A principal allowed past quarantine sent a command to a quarantined node. | Confirm the incident response that needed it. |

A process a client created without naming its pid is attributed to no intended
process; twilight counts these per principal in
`twilight_unattributed_processes_total` instead of raising an alert, because
client-side argument builders start helper processes that way.

Acknowledge an alert when someone is looking at it and resolve it when it is
dealt with; the next occurrence after a resolve opens a new alert.

What reconcile cannot see: a compromised dawn that runs other content within
the shape of a process twilight intended; entries a compromised nightfall never writes about
its own sessions; and entries removed after the last checkpoint, at most one
checkpoint interval of them.
