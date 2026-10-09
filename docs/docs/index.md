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

<div class="dusk-hero__horizon" markdown>
![](assets/landing/sun.svg){ .dusk-hero__sun }
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

Each of these devices is a node: it runs Dusk, and you can reach it from a
single place.

<div class="dusk-fleet__figure">
--8<-- "assets/landing/fleet.svg"
--8<-- "assets/landing/fleet-phone.svg"
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

<article class="dusk-layer" data-layer="nightfall" markdown>

### :dusk-nightfall: Nightfall

Security layer
{ .dusk-layer__role }

Every device connects through Nightfall. It checks who can do what on which
device, and records every action taken.

</article>

<article class="dusk-layer" data-layer="dawn" markdown>

### :dusk-dawn: Dawn

Client layer
{ .dusk-layer__role }

Dawn carries out the work on your devices: it runs commands, streams logs back
and collects files.

</article>

<article class="dusk-layer" data-layer="twilight" markdown>

### :dusk-twilight: Twilight

Orchestration layer
{ .dusk-layer__role }

Tell Twilight what your fleet should look like. It rolls changes out in
phases, watches their health, and stops when something goes wrong.

</article>

<article class="dusk-layer" data-layer="data" markdown>

### :dusk-data: Data

Inventory, events and files
{ .dusk-layer__role }

Everything your devices report, stored on your own infrastructure and ready
to send anywhere else you need it.

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

<section class="dusk-section dusk-start" markdown>

<div class="grid cards" markdown>

-   :material-puzzle-outline:{ .dusk-card-icon }

    **Embed Dusk**

    Drop the C library into an app you already ship - every device running it
    becomes a manageable node.

    [Embed it →](getting-started/guides/embed.md)

-   :material-chart-timeline-variant:{ .dusk-card-icon }

    **Analytics & diagnosis**

    Connect to your fleet to see what's running, read logs, and fix misbehaving
    devices.

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
