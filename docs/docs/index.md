---
template: home.html
hide:
  - toc
  - footer
---

<section class="dusk-hero" markdown>

<div class="dusk-hero__floats" aria-hidden="true" markdown>
:dusk-nightfall:{ .dusk-float data-layer="nightfall" }
:dusk-dawn:{ .dusk-float data-layer="dawn" }
:dusk-data:{ .dusk-float data-layer="data" }
:dusk-phone:{ .dusk-float }
:dusk-twilight:{ .dusk-float data-layer="twilight" }
:dusk-observability:{ .dusk-float data-layer="observability" }
:dusk-microcontroller:{ .dusk-float }
:dusk-server:{ .dusk-float }
</div>

<div class="dusk-hero__inner" markdown>

# ![](assets/landing/logo-d.svg){ .dusk-hero__d }usk { #dusk .dusk-hero__wordmark aria-label="Dusk" }

The one solution for every fleet.<br>
Seamlessly drop it in at any scale.
{ .dusk-hero__lead }

Dusk can monitor and manage fleets deployed on everything from
microcontrollers to supercomputers. Run it inside applications you already
ship.
{ .dusk-hero__pitch }

[Get started](getting-started/index.md){ .md-button .md-button--primary }
[View on :fontawesome-brands-github:](https://github.com/tomerze/dusk){ .md-button .dusk-button-ghost title="View on GitHub" }

</div>

<div class="dusk-hero__horizon">
--8<-- "assets/landing/sun.svg"
</div>

</section>

<section class="dusk-platforms" markdown>

## Runs where your software runs.

<div class="dusk-platforms__grid" markdown>

- :simple-linux: Linux
- :fontawesome-brands-windows: Windows
- :simple-apple: macOS
- :simple-freebsd: FreeBSD
- :simple-android: Android
- :simple-apple: iOS
- :simple-webassembly: WASM
- :simple-espressif: ESP-IDF
- :material-weather-windy: Zephyr
- :material-chip: Bare metal

</div>

[See all platforms](embedding/node-artifacts.md#platforms)
[Add your own](getting-started/guides/custom-impl.md)

</section>

<section class="dusk-section dusk-fleet" markdown>

## Fleet

Place a Dusk Node in everything you deploy, that's your Dusk Fleet

<div class="dusk-fleet__figure">
--8<-- "assets/landing/fleet.svg"
</div>

</section>

<section class="dusk-section dusk-stack" markdown>

## Stack

Five layers connect your fleet to the people who run it.

<div class="dusk-stack__body" markdown>

<nav class="dusk-stack__menu" aria-label="Stack layers" markdown>
[Nightfall](#nightfall){ data-layer="nightfall" }
[Dawn](#dawn){ data-layer="dawn" }
[Twilight](#twilight){ data-layer="twilight" }
[Data](#data){ data-layer="data" }
[Observability](#observability){ data-layer="observability" }
</nav>

<div class="dusk-stack__layers" markdown>

<div class="dusk-stack__head" aria-hidden="true"></div>

<article class="dusk-layer" data-layer="nightfall" markdown>

### :dusk-nightfall: Nightfall

Security layer
{ .dusk-layer__role }

Every node connects through Nightfall. It checks who can do what on which
node, and records every action taken.

</article>

<article class="dusk-layer" data-layer="dawn" markdown>

### :dusk-dawn: Dawn

Client layer
{ .dusk-layer__role }

Dawn carries out the work on your nodes: it runs commands, streams logs back
and collects files.

</article>

<article class="dusk-layer" data-layer="twilight" markdown>

### :dusk-twilight: Twilight

Orchestration layer
{ .dusk-layer__role }

Twilight is all about transition of state. Tell Twilight what your fleet
should look like, and it will take care of it, your way.

</article>

<article class="dusk-layer" data-layer="data" markdown>

### :dusk-data: Data

Inventory, events and objects
{ .dusk-layer__role }

Everything your fleet reports, stored on your own infrastructure and ready to
send anywhere else you need it.

</article>

<article class="dusk-layer" data-layer="observability" markdown>

### :dusk-observability: Observability

Dashboards and alerts
{ .dusk-layer__role }

See the whole fleet at a glance. Use Grafana and SigNoz out of the box, or
connect the tools you already have.

</article>

</div>

<div class="dusk-stack__figure">
--8<-- "assets/landing/stack.svg"
</div>

</div>

</section>

<section class="dusk-os" markdown>

<div class="dusk-os__tour" markdown>

<div class="dusk-os__stage" markdown>

## Node { .dusk-os__title }

Feels like an OS inside. Somehow you already know how to drive it.
{ .dusk-os__lead }

<div class="dusk-os__terminals">
<div class="dusk-os__dot" aria-hidden="true"></div>
<div class="dusk-os__terminal dusk-os__terminal--ps">
<div class="dusk-os__bar" aria-hidden="true"><span></span><span></span><span></span></div>
<pre class="dusk-os__session"><span class="dusk-os__gray">$</span> <span class="dusk-os__green dusk-os__bold">dusk</span> <span class="dusk-os__bright-cyan dusk-os__namespace">7c3f1a9e2b5d4086</span><span class="dusk-os__blue">.connect.nightfall</span>
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan">pc1</span>
<span class="dusk-os__bright-cyan">●</span> <span class="dusk-os__green">❯</span> <span class="dusk-os__cyan dusk-os__bold">ps</span>
╭───┬─────────────────────────┬─────────┬────────────────────┬────────────────────┬─────╮
│ <span class="dusk-os__gray">#</span> │          <span class="dusk-os__yellow dusk-os__bold">Name</span>           │ <span class="dusk-os__yellow dusk-os__bold">Version</span> │     <span class="dusk-os__yellow dusk-os__bold">Program ID</span>     │        <span class="dusk-os__yellow dusk-os__bold">PID</span>         │ ... │
├───┼─────────────────────────┼─────────┼────────────────────┼────────────────────┼─────┤
│ <span class="dusk-os__gray">0</span> │ sh[server]              │ 0.1.0   │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0x8d0e0504ec994ea4</span></span> │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xf2efce60e8c425d0</span></span> │ ... │
│ <span class="dusk-os__gray">1</span> │ nightfall[listen :9090] │ 0.1.0   │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xd089c575e560637e</span></span> │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0x7a7d909c3bb3ac79</span></span> │ ... │
│ <span class="dusk-os__gray">2</span> │ init                    │ 0.1.0   │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xd77c7f8193a1856c</span></span> │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xc079da09ecb0cd0a</span></span> │ ... │
│ <span class="dusk-os__gray">3</span> │ ps                      │ 0.1.0   │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xd111e8c31818511d</span></span> │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0xe67bdd2de87a69eb</span></span> │ ... │
│ <span class="dusk-os__gray">4</span> │ sh[prompt <span class="dusk-os__cell">⟷</span> pc1]        │ 0.1.0   │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0x8d0e0504ec994ea4</span></span> │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0x565e93ec9fe33165</span></span> │ ... │
╰───┴─────────────────────────┴─────────┴────────────────────┴────────────────────┴─────╯
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan">pc1</span><span class="dusk-os__gray"> ⇄ <span class="dusk-os__latency">66</span>ms</span>
<span class="dusk-os__gray">○</span> <span class="dusk-os__green">❯</span> </pre>
</div>
<div class="dusk-os__terminal dusk-os__terminal--cp">
<div class="dusk-os__bar" aria-hidden="true"><span></span><span></span><span></span></div>
<pre class="dusk-os__session dusk-os__session--narrow"><span class="dusk-os__gray">$</span> <span class="dusk-os__green dusk-os__bold">dusk</span> <span class="dusk-os__bright-cyan dusk-os__namespace">7c3f1a9e2b5d4086</span><span class="dusk-os__blue">.connect.nightfall</span>
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan dusk-os__instance">instance-20261010</span>
<span class="dusk-os__bright-cyan">●</span> <span class="dusk-os__green">❯</span> <span class="dusk-os__cyan dusk-os__bold">cp</span> <span class="dusk-os__white">report.txt</span> <span class="dusk-os__white">:/tmp/report.txt</span>
╭─────────────┬────────────────────────╮
│ <span class="dusk-os__yellow dusk-os__bold">source</span>      │ report.txt             │
│ <span class="dusk-os__yellow dusk-os__bold">destination</span> │ :/tmp/report.txt       │
│ <span class="dusk-os__yellow dusk-os__bold">length</span>      │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">2819</span></span>                   │
│ <span class="dusk-os__yellow dusk-os__bold">resumed</span>     │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0</span></span>                      │
│ <span class="dusk-os__yellow dusk-os__bold">sha256</span>      │ 3114865161b206080e637a │
│             │ 48c1caaaf8df4950c6ecce │
│             │ 76a1b7a30cbba34be6d0   │
╰─────────────┴────────────────────────╯
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan dusk-os__instance">instance-20261010</span><span class="dusk-os__gray"> ⇄ <span class="dusk-os__latency">17</span>ms</span>
<span class="dusk-os__gray">○</span> <span class="dusk-os__green">❯</span> </pre>
<pre class="dusk-os__session dusk-os__session--wide"><span class="dusk-os__gray">$</span> <span class="dusk-os__green dusk-os__bold">dusk</span> <span class="dusk-os__bright-cyan dusk-os__namespace">7c3f1a9e2b5d4086</span><span class="dusk-os__blue">.connect.nightfall</span>
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan dusk-os__instance">instance-20261010</span>
<span class="dusk-os__bright-cyan">●</span> <span class="dusk-os__green">❯</span> <span class="dusk-os__cyan dusk-os__bold">cp</span> <span class="dusk-os__white">report.txt</span> <span class="dusk-os__white">:/tmp/report.txt</span>
╭─────────────┬──────────────────────────────────╮
│ <span class="dusk-os__yellow dusk-os__bold">source</span>      │ report.txt                       │
│ <span class="dusk-os__yellow dusk-os__bold">destination</span> │ :/tmp/report.txt                 │
│ <span class="dusk-os__yellow dusk-os__bold">length</span>      │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">2819</span></span>                             │
│ <span class="dusk-os__yellow dusk-os__bold">resumed</span>     │ <span class="dusk-os__bright-cyan"><span class="dusk-os__bold">0</span></span>                                │
│ <span class="dusk-os__yellow dusk-os__bold">sha256</span>      │ 3114865161b206080e637a48c1caaaf8 │
│             │ df4950c6ecce76a1b7a30cbba34be6d0 │
╰─────────────┴──────────────────────────────────╯
<span class="dusk-os__yellow dusk-os__bold">dusk </span><span class="dusk-os__cyan">0.1.0</span><span class="dusk-os__yellow dusk-os__bold"> @</span><span class="dusk-os__cyan dusk-os__instance">instance-20261010</span><span class="dusk-os__gray"> ⇄ <span class="dusk-os__latency">17</span>ms</span>
<span class="dusk-os__gray">○</span> <span class="dusk-os__green">❯</span> </pre>
</div>
</div>

</div>

</div>

</section>

<section class="dusk-section dusk-start" markdown>

<div class="grid cards" markdown>

-   :material-puzzle-outline:{ .dusk-card-icon }

    **Embed Dusk**

    Drop the C library into an app you already ship - every running copy of it
    becomes a manageable node.

    [Embed it →](getting-started/guides/embed.md)

-   :material-chart-timeline-variant:{ .dusk-card-icon }

    **Analytics & diagnosis**

    Connect to your fleet to see what's running, read logs, and fix misbehaving
    nodes.

    [See your fleet →](getting-started/guides/connect-a-client.md)

-   :material-web:{ .dusk-card-icon }

    **Drive it over HTTP**

    Run the API gateway and reach any node from curl, a script, a dashboard or a
    browser. AI agents get an MCP server on the same port.

    [Open the gateway →](features/gateway.md)

-   :material-tune-variant:{ .dusk-card-icon }

    **Customize & extend**

    Dusk is modular: swap the platform backend or add your own programs when the
    built-ins aren't enough.

    [Extend Dusk →](getting-started/guides/first-program.md)

-   :material-book-open-variant:{ .dusk-card-icon }

    **Reference**

    The crate map and the Cap'n Proto wire schemas.

    [Reference →](sdk-reference/crates.md)

</div>

</section>
