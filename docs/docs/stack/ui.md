# The twilight UI

This page is for **operators**: the people who roll changes out to a fleet of
dusk nodes, watch them land, and look after individual nodes. It walks through
every page of twilight's web UI and how to do each task there. It assumes
twilight is already deployed and that you have an account or an API token for
it.

twilight serves the UI and its API from the same address: the root of its HTTP
listener (port 8080 in the shipped configuration, usually behind your ingress).
The UI starts in the dark theme; the sun or moon button at the bottom of the
navigation switches it, and the browser remembers your choice.

## Signing in and what you may do

- **Single sign-on**: when twilight is configured for OIDC, the sign-in page
  offers *Sign in*, which takes you to your identity provider and back. Your
  role comes from your identity provider's groups. An account none of whose
  groups is mapped to a role is refused when the identity provider sends it
  back: the browser shows twilight's answer, an error saying that none of your
  groups maps to a twilight role, and no session starts.
- **API token**: an admin creates one on the twilight host with
  `twilight token create --name <name> --role <viewer|operator|admin>`. Paste it
  into *API token*. The UI keeps it in that browser tab only and forgets it when
  the tab closes.
- **Development mode**: on a twilight started with `TWILIGHT_DEV=1` on a
  loopback address, *Sign in* signs you in as an admin without asking for
  anything. Never use it anywhere else.

Your role decides what the UI offers:

| role | can |
|------|-----|
| viewer | see everything; every action is replaced by a *View only* badge |
| operator | also create, start, pause, resume, abort, complete and archive campaigns, retry and resolve nodes, quarantine nodes and release them, open sessions, stream logs, collect files, acknowledge and resolve alerts |
| admin | everything an operator can, and also retire and revoke nodes |

twilight checks every action on the server as well; the UI only hides what your
role cannot do.

## Around the UI

- The **navigation** on the left (behind the menu button on a phone) leads to
  Overview, Campaigns, Nodes and Alerts. The Alerts entry shows how many alerts
  are open, in red while any of them is critical.
- **Live** at the bottom of the navigation means counters, presence and alerts
  update as they change. *Reconnecting* means that stream is interrupted; every
  page then still refreshes on its own every 15 to 30 seconds.
- A red **critical alert** banner sits on top of every page while a critical
  alert waits for acknowledgement, with how many do. *View alerts* opens the
  alerts page; *Acknowledge* acknowledges the first one in place.
- When twilight is too busy to answer (it says *busy*, or that dawn or a
  dependency is busy), the page shows twilight's message and asks again after
  the wait twilight names.
- Times are relative ("7m ago"); hover one to see the exact time with its time
  zone. Long identifiers are shortened in the middle (`e044df54…92a08c8`); hover
  for the whole value, and use the copy button next to it to copy it.
- Every status is shown as a word with an icon, never as a colour alone.
- Filters, tabs and sort orders live in the address, so you can share the exact
  view you are looking at by copying the link.

### Keyboard

- In any table, focus a row with Tab, then move with the arrow keys (or `j` and
  `k`), jump with Home and End, and open the row with Enter or Space.
- In the selector and script editors, Ctrl+Space opens completion, Tab leaves
  the editor, and in the one-line selector on the Nodes page Enter applies it.
- Dialogs close with Escape; their confirm button stays disabled until
  everything they need is filled in.

## Overview

What the fleet looks like now and what is changing it:

- **Fleet**: how many nodes have a live session out of the whole inventory, and
  the inventory by lifecycle. Each lifecycle links to the nodes in it.
- **Active campaigns**: every running or paused campaign with its progress,
  phase and start. *All campaigns* opens the full list.
- **Open alerts**: how many alerts are open by severity, and the five newest.

When twilight's view of which nodes are online is incomplete (a nightfall
instance has not published its full connection table in time), the overview
says *The online view is degraded*. Online counts may then be stale, and every
campaign's health gates hold until the view recovers.

## Campaigns

The **Campaigns** page lists every campaign, newest first, with its status,
progress, current phase, selector, rate, start and owner. Search by name, owner,
selector or description, and filter by status or action. Search and the action
filter look through the 2,000 newest campaigns of the status you picked, and
say so when there are more; pick a status to search further back.

The progress bar is split by the state of each node's row (succeeded, failed,
in progress, and so on; hover a segment for its count), and the ticks mark where
each phase ends. While a campaign runs, the striped part is about how many
matched nodes it has not reached yet, estimated from the share of the nodes its
open phases cover; the campaign's own page counts them exactly. A row with a red
edge is a campaign a health gate has paused.

### Creating a campaign

*New campaign* opens the form. Nothing is sent to any node until the campaign
starts, so you can fill it in, review it and save it as a draft as often as you
like.

1. **Campaign**: a name (required) and a description that explains why the
   campaign exists.
2. **Nodes**: the selector. As you type, twilight checks it and shows how many
   nodes match now with a sample of them, or underlines exactly where the
   selector is wrong and says what is wrong. A campaign needs a selector; to
   target every node, write `has(device_id)`. The language:
    - fields such as `country`, `os_name`, `os_version`, `os_build`,
      `dusk_version`, `hardware_class`, `tenant`, `locale`, `hostname`,
      `lifecycle`, and any fact as `facts["dusk.device.memory_bytes"]`;
    - comparisons `==`, `!=`, `<`, `<=`, `>`, `>=`, `in [...]` and `not in [...]`,
      `has(field)` and `has(facts["key"])`, combined with `and`, `or`, `not` and
      parentheses;
    - `dusk_version` and `reported_version` compare as versions
      (`dusk_version < "0.2.0"`), and `semver(...)` makes any field or fact
      compare that way;
    - a comparison with a missing value is false, so
      `not country == "US"` also selects nodes whose country is unknown.

    A node joins the campaign the first time it matches while its phase is
    open. If it stops matching before it is done, its row is set aside as
    excluded, and it comes back if it matches again.
3. **Action**: what the nodes do.
    - *Run script* runs a dusk shell script once on each node. It can also
      collect up to 16 files from each node (absolute paths) and stream the
      node's logs at a level of your choice for a while. A node that never
      reports back is shown to you as unknown and is not run again on its own.
    - *Ensure version* brings each node to a version: the node must report it
      under the key in *Read from* (`dusk.version` by default) once the script
      has run. The script runs only on nodes that report something else, up to
      the *Attempts* under *Timeouts and retries*; a node that still reports
      something else after its last attempt is failed. A node that reached the
      version and drifts away from it later gets the script again, also after
      the rollout is over.
    - *Ensure config* does the same for a config hash reported under
      `dusk.config.hash`.
    - *Quarantine* moves nodes to quarantine. An optional script runs first on
      nodes that are online; turn on *Quarantine only the nodes where the script
      succeeds* to make it a condition.
4. **Rollout**: the phases and the rate. Each phase *reaches* a cumulative
   share of the matched nodes: 1%, 10%, 50%, 100% means the second phase adds
   9% to the first one's 1%. The last phase must reach 100%, and each bake must
   be at least as long as the silent window and the node timeout. Each row says
   what the phase adds and about how many nodes that is. The presets set common
   shapes. Which phase a node falls into is fixed when the campaign is created,
   so pausing and resuming never reshuffles the canary. *Nodes per second* caps
   dispatch across every twilight instance together.
5. **Health gates**: a phase opens the next one only once it has baked, the
   gate has *Minimum sample* results (at least 1; a new campaign starts at
   10) or, in a phase with fewer nodes, a result from every one of them (so a
   phase with no node needs none), and both rates are under their limits:
    - the **failure rate** is the share of reported results that failed (nodes
      left unknown count as failed);
    - the **silent rate** is the share of nodes that reported success and then
      disconnected without coming back within the *Silent window*.

    With *Also judge each group of* ticked, every group of an inventory fact
    (each OS build, each hardware class, each dusk version, each country) with
    enough results is judged on its own, so a problem confined to one OS build
    fails the gate before it moves the overall rate. *When a gate fails* pauses
    the campaign or aborts it; *Most failures allowed* is an optional cap that
    fails the gate once more nodes than that have failed.
6. **Timeouts and retries**: the *Node timeout* after delivery, how many
   *Attempts* a node that reports a failure gets and how they back off, an
   optional *Deadline* (in your time zone) at which the campaign completes
   and nodes that are not in flight are cancelled, and for ensure actions whether the campaign may *overlap* a
   running one for the same key.

The **Summary** beside the form (below it on a phone) reads the campaign back in
one sentence as you edit, and lists what is left to fix, each linking to its
section. *Review* shows the whole campaign in plain English, for example:

> Bring 3,644 nodes to dusk.version 0.2.2 in 4 phases (1% → 10% → 50% → 100%),
> at most 50 nodes/s, pausing if more than 5% fail in any OS build, hardware
> class, dusk version or country.

followed by the details and the definition exactly as it will be saved. From
there:

- *Save as draft* keeps it for later. A draft can be edited (*Edit* on its page)
  and started at any time, or discarded (*More → Discard draft*, with a reason),
  which aborts it so that it can be archived.
- *Create and start* saves it and starts dispatching to phase 1 at once. If
  the start is refused, the draft stays and a message says why.

An ensure campaign whose nodes overlap a running or paused campaign for the same
key is flagged under the selector and on the review, with how many nodes both
match. Only the campaign that started first
acts on those nodes; this one's rows there become conflicts. Starting such a
campaign needs *Allow overlap*; saving it as a draft does not.

To run a variation of any campaign, open it and choose **More → Duplicate as a
new draft**: the form opens with a copy of its definition.

### Watching a campaign

A campaign's page shows, from the top:

- **The header**: its status and the actions that status allows, each behind a
  confirmation that says what will happen.
    - *Start* (draft): dispatch begins to phase 1. The definition is fixed from
      then on. For an ensure campaign the dialog says how many of its nodes
      running or paused campaigns of the same kind (and key) already target;
      while that is above 0 twilight refuses the start unless the policy allows
      overlap, and the dialog does not offer it.
    - *Pause* (running): nothing new is dispatched, including retries;
      processes already on nodes finish. Gates are still evaluated; bake time
      stops until it resumes. If the gate fails while it is paused, resuming
      needs a gate override.
    - *Resume* (paused): dispatch continues and bake time accrues again.
    - *Override and resume* (paused by a health gate, or its gate failed
      while it was paused): needs a reason, which is
      recorded in the history. For the current phase the gate then counts only
      the nodes dispatched after the resume, so the same group fails it again
      if the problem is still there.
    - *Abort* (running or paused): needs a reason. Nodes that are not in
      flight are cancelled; processes already on nodes finish. An aborted campaign cannot be
      resumed.
    - *More → Complete* (running or paused): ends the campaign. An ensure
      campaign stops correcting nodes that drift.
    - *More → Archive* (completed, aborted or failed): drops the per-node rows
      and the history, keeping the campaign, its definition and its counters.
      twilight refuses until an hour after the last dispatch plus the longest
      a node's process may take (the node timeout, once more for each file a
      *Run script* collects, plus how long it streams logs), so no result can
      still be arriving; the dialog says from when.
- **Why it stopped**: a paused campaign says whether its health gate, an
  operator or a dispatch nightfall denied paused it, and why; an aborted or failed
  campaign says why it stopped, with the failing group when a gate stopped it. An ensure campaign whose phases have
  all passed shows **Converging**: it keeps sending its action to matching nodes
  that report anything other than the desired state, until you complete or
  abort it.
- **Progress** against every node the selector matches now, with a legend; each
  state links to the nodes in it, and *Not reached yet* counts the matching
  nodes that have no row yet (in later phases, or not seen online since their
  phase opened).
- **Phases**: one step per phase with its share, its nodes and its state:

| state | means |
|-------|-------|
| Passed | the phase baked and its gate passed |
| Completed here | the campaign was completed while this phase was open, by an operator, at its deadline or on its own |
| Baking, 1h 20m left | the gate is satisfied; the bake time is not yet over |
| Holding: waiting for sample 23 of 50 | too few results (or too few nodes past the silent window) to judge the phase yet |
| Holding: online view degraded | the gate holds until twilight's online view is complete again |
| Gate failing | a rate is over its limit; the campaign pauses or aborts on the next check |
| Paused, Paused by its gate | the campaign is paused in this phase |
| Converging | every phase passed and the campaign keeps enforcing its desired state |
| Not open yet, Planned | later phases (Planned while the campaign is a draft) |
| Stopped here, Stopped by its policy, Never opened | where an aborted or a failed campaign stopped, and the phases after it |

Below them, four tabs:

- **Gates**: the verdict (Passing, Holding, Failing, Degraded view, Enforcing
  for a converging campaign, Not evaluated once it stopped) with twilight's
  reason, the failure and silent rates against their limits, the sample and the
  bake against what they need, and the **breakdown**: one table per inventory
  fact, opened on the group that tripped the gate (marked *Tripped the gate*),
  failing groups first. Groups below the minimum sample are labelled *Below
  sample*; they cannot fail the gate yet. Unreachable nodes, replaced sessions,
  timeouts before delivery and denials never count as failures; they show on the
  Nodes tab. twilight evaluates the gates of a running or paused campaign
  every few seconds (`engine.gate_interval_seconds`, 15 by default).
- **Nodes**: every node with a row, filtered by state and phase, with its
  attempt, pid (and when it was reaped), last error and when it last changed;
  under its state, a row shows the last result dawn reported and how many
  dispatches of its attempt did not reach the node. The pid is the process the attempt runs as on the node:
  twilight derives it from the campaign, the node and the attempt, so sending
  one attempt twice finds the process already there and never runs it again.
  While the campaign is running or paused, *Retry*
  gives failed and unknown nodes (all of them, those in the state and phase you
  filtered on, or the ones you tick) a new attempt at a new pid, at most 10,000
  per retry; with a filter on, the dialog counts the nodes and retries exactly
  those. For a one-shot action an unknown node may already have run it.
  *Resolve unknown* closes ticked unknown rows as succeeded or failed when you
  know what happened on them, for example from their logs; nothing is sent to
  the nodes. A resolved row counts in the gate like a reported result: as a
  success, or as a failure toward the failure rate. Both need a reason, which
  goes into the campaign history.
- **History**: the campaign's events, newest or oldest first, filtered to gates,
  operators, phases and status, or nodes. Click an event to see its fields. A
  campaign with more than 2,000 events shows its newest 2,000 and says so.
- **Definition**: the campaign in plain English, *Copy as JSON*, and the
  selector, script and policy it runs with.

The states a node's row can be in:

| state | means |
|-------|-------|
| Pending | waiting for a token and an online session |
| Dispatching, Dispatched | its pid is recorded as intended; dawn is being called, or accepted the dispatch |
| Delivered | the process started on the node |
| Verifying | an ensure process succeeded; waiting for the node to report the desired state from its next session |
| Backoff | it failed; another attempt follows after a backoff |
| Succeeded | done |
| Failed | it failed on every attempt allowed |
| Unknown | it was delivered but no result arrived; resolve it by hand or retry it |
| Excluded | it no longer matches the selector; it returns if it matches again |
| Conflict | an earlier campaign owns this key on the node |
| Cancelled | stopped before it finished: the campaign was aborted, failed or completed, or passed its deadline |

## Nodes

The **Nodes** page is the inventory, one row per node. Type a selector into the
bar at the top (the same language as a campaign's, checked as you type) and
press Enter or *Apply selector* to filter the table; *Any*, *Online* and
*Offline* filter by presence. Click a column heading with arrows to sort by it.
*Columns* adds or removes columns (OS build, locale, impl, architecture,
reported version, installation id, first seen, enrolled); the browser remembers
your choice.

## A node

Open a node from any table to see:

- **Identity**: device id (the machine, stable across reinstalls), installation
  id (this install of dusk), certificate fingerprint (changes on every renewal),
  tenant, enrollment.
- **Presence**: its live sessions, each with its namespace id, epoch, the
  nightfall instance holding it, since when and when it was last heard; or, when
  it is offline, when it was last seen.
- **Reported state**: the version, config hash and running services it last
  reported, with its OS and hardware class.
- **Facts**: everything it reported, filterable by key or value, each copyable.
- **Processes**: its newest 100 campaign rows, each with the pid of its
  current attempt (*not sent* before the first dispatch), the campaign with
  its action and attempt, the row's state, the status dawn last reported for
  that process, the last error, when it was sent and when the pid was reaped.
  Among the statuses, *Duplicate* means the pid was already in the node's
  process table, so nothing ran again; *Running* means it is still running or
  dawn could not learn how it ended; *Ended* means the script finished but
  whether it succeeded could not be learned, which is not the same as failed;
  *No result yet* means dawn has not reported on it. A pid is reaped (killed
  and taken out of the node's process table) only once no resend of its
  attempt can come.

A quarantined, retired or revoked node says so at the top, with the reason
recorded when it changed. A node whose device is retired or revoked says that
too, with the reason, when and by whom.

### Acting on a node

These need the operator role, and retiring or revoking a node needs the admin
role. The first three need a connected, active node; when they are disabled,
hover them to see why.

- **Open session**: say why and for how long (15 minutes, 1 hour or 4 hours).
  twilight picks a pid, records it with you as intended for this node, writes
  your reason to its log with the pid, and shows the pid and a request body holding the pid and the
  node. Send that body to dawn's `POST /v1/connect` with your own token, then
  use dawn's shell endpoints. dawn starts a shell at that pid; every call it
  makes there is ledgered under the pid, and calls under it after the time you
  picked raise an alert.
- **Stream logs**: pick a level and how long (1 second to 24 hours). The node
  sends its log records at that level and above to the OpenTelemetry collector;
  the confirmation names the stream dawn started.
- **Collect file**: give an absolute path on the node. dawn uploads the file to
  the Dusk stack's object store under `files/<device>/<installation>/<pid>/`
  and a `dusk.files` event records its size and SHA-256; the confirmation names
  the upload dawn started.
- **Lifecycle**, each with a reason, kept as the installation's (or the
  device's) lifecycle reason:

| action | what nightfall does |
|--------|---------------------|
| Quarantine | keeps the node connected, drops every client session on it, and from then on lets a caller do only what both its role and the quarantine policy allow (a role with the quarantine override keeps its access, and each such call raises an alert). twilight still dispatches to it for campaigns that target it; nightfall may deny those dispatches, which pauses the campaign. |
| Release from quarantine | makes the node active again |
| Retire | drops its connection and refuses this installation at every handshake; the node stays in the inventory |
| Revoke | the same, while it is revoked. The UI offers no way to make the installation active again. You confirm by typing the node's hostname (its device id when it has none). |
| Retire the device, Revoke the device | blocks the whole machine: drops every installation of it that is connected and refuses all of them at every handshake, including installations made later by reinstalling dusk. Admin only, confirmed by typing the node's hostname. |
| Lift the device block | offered instead while the device is retired or revoked, admin only: stops refusing the machine's installations because of the device. Each installation keeps its own lifecycle, so one retired or revoked by itself stays refused. |

## Alerts

The **Alerts** page lists open alerts (or *All* of them) newest first, filtered
by severity and kind (those two filters look through the 2,000 newest alerts).
Each row says what happened, which call and principal it concerns when the alert
names them, when it was last seen and how many times. Critical and high open
alerts have a coloured edge; resolved ones are dimmed. *Acknowledge* says someone is on it (and removes a
critical alert from the banner); *Resolve* closes it. Click an alert to read
what its kind means and see its fields, with links to the node and campaigns it
names.

| kind | means |
|------|-------|
| Process nobody intended | a process was created on a node at a pid twilight never recorded as intended for that node: treat it as a stolen client certificate, someone using the SDK directly, or a compromised dawn until shown otherwise |
| Default shell used without intent | a command ran in a node's default shell while no process twilight intended for that node was open: treat it as a stolen client certificate or a compromised dawn until shown otherwise |
| Process on the wrong node | an intended pid was used on a node other than the one twilight recorded for it |
| Calls after the deadline | calls kept arriving under a pid after the time twilight recorded for it, other than killing it or waiting for it |
| Pid reused | a process at one pid was created from more than one client session |
| Calls after the result | calls kept arriving under a pid more than a minute after dawn reported its final result, other than the reap |
| Unexpected process shape | more scripts ran under a pid than twilight intended for it |
| Result without a ledger entry | dawn reported a process it delivered, but nightfall ledgered no process at that pid within 10 minutes |
| Ledger chain broken | an entry in the nightfall ledger is missing, changed or out of order, or a checkpoint signature does not verify |
| Quarantine override | a principal used its override to call a quarantined node |
| Revocation not enforced | a revoked or retired node still appeared in a nightfall census, or never got its revoked disconnect |
| Enrollment spike | more nodes enrolled in a minute than the threshold; a leaked fleet token looks like this, and so does a large rollout |
| Campaign conflict | two running campaigns set the same key on the same nodes |

A process created without a pid, so that the node chose one, raises no alert;
twilight counts it in the `twilight_unattributed_processes_total` metric.

## Trying the UI without a fleet

To learn the UI, or to show it to someone, run it against its built-in mock of
twilight's API: a generated fleet of 12,000 nodes with campaigns in every
status, a health gate failing in one OS build, and open alerts. From a checkout
of this repository, with Node.js 22.22 or newer:

```bash
cd services/twilight/web
npm ci
npm run dev
```

and open the address it prints. Changes you make there live in that browser tab
until you reload it. Add `?scenario=degraded`, `empty`, `errors`, `viewer`,
`operator`, `signed-out` or `slow` to the address to see those situations; the
choice lasts for the tab.
