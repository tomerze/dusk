---
template: home.html
hide:
  - toc
  - footer
---

<section class="dusk-hero" markdown>

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
{ .dusk-hero__actions }

:dusk-nightfall:{ .dusk-float data-layer="nightfall" style="--dusk-pace: 11s" }
:dusk-phone:{ .dusk-float style="--dusk-pace: 8s; --dusk-turn: reverse" }
:dusk-dawn:{ .dusk-float data-layer="dawn" style="--dusk-pace: 13s" }
:dusk-microcontroller:{ .dusk-float style="--dusk-pace: 9s; --dusk-turn: reverse" }
{ .dusk-hero__floats data-side="start" aria-hidden="true" }

:dusk-twilight:{ .dusk-float data-layer="twilight" style="--dusk-pace: 10s; --dusk-turn: reverse" }
:dusk-server:{ .dusk-float style="--dusk-pace: 12s" }
:dusk-data:{ .dusk-float data-layer="data" style="--dusk-pace: 7s; --dusk-turn: reverse" }
:dusk-observability:{ .dusk-float data-layer="observability" style="--dusk-pace: 14s" }
{ .dusk-hero__floats data-side="end" aria-hidden="true" }

</div>

<div class="dusk-hero__horizon">
--8<-- "assets/landing/sun.svg"
</div>

</section>

<section class="dusk-platforms" markdown>

## Runs where your software runs. { .dusk-platforms__title }

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
{ .dusk-platforms__links }

</section>

<section class="dusk-section dusk-fleet" markdown>

## Fleet { .dusk-section__title }

Place a Dusk Node in everything you deploy, that's your Dusk Fleet
{ .dusk-section__lead }

<div class="dusk-fleet__figure">
--8<-- "assets/landing/fleet-narrow.svg"
--8<-- "assets/landing/fleet.svg"
</div>

</section>

<section class="dusk-section dusk-stack" markdown>

## Stack { .dusk-section__title }

Five layers connect your fleet to the people who run it.
{ .dusk-section__lead }

<div class="dusk-stack__body" markdown>

<nav class="dusk-stack__menu" aria-label="Stack layers" markdown>
[Nightfall](#nightfall){ data-layer="nightfall" }
[Dawn](#dawn){ data-layer="dawn" }
[Twilight](#twilight){ data-layer="twilight" }
[Data](#data){ data-layer="data" }
[Observability](#observability){ data-layer="observability" }
{ .dusk-stack__links }
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

<section class="dusk-section dusk-start" markdown>

<div class="grid cards" markdown>

-   :material-puzzle-outline:{ .dusk-card-icon data-tone="gold" }

    **Embed Dusk**

    Drop the C library into an app you already ship - every running copy of it
    becomes a manageable node.

    [Embed it →](getting-started/guides/embed.md){ .dusk-card-link }

-   :material-chart-timeline-variant:{ .dusk-card-icon data-tone="nightfall" }

    **Analytics & diagnosis**

    Connect to your fleet to see what's running, read logs, and fix misbehaving
    nodes.

    [See your fleet →](getting-started/guides/connect-a-client.md){ .dusk-card-link }

-   :material-web:{ .dusk-card-icon data-tone="dawn" }

    **Drive it over HTTP**

    Run the API gateway and reach any node from curl, a script, a dashboard or a
    browser. AI agents get an MCP server on the same port.

    [Open the gateway →](features/gateway.md){ .dusk-card-link }

-   :material-tune-variant:{ .dusk-card-icon data-tone="twilight" }

    **Customize & extend**

    Dusk is modular: swap the platform backend or add your own programs when the
    built-ins aren't enough.

    [Extend Dusk →](getting-started/guides/first-program.md){ .dusk-card-link }

-   :material-book-open-variant:{ .dusk-card-icon data-tone="data" }

    **Reference**

    The crate map and the Cap'n Proto wire schemas.

    [Reference →](sdk-reference/crates.md){ .dusk-card-link }

</div>

</section>
