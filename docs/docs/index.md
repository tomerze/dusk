---
template: home.html
hide:
  - toc
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

<div class="grid cards" markdown>

-   **Embed Dusk**

    ---

    Drop the C library into an app you already ship - every device running it
    becomes a manageable node.

    [Embed it →](getting-started/guides/embed.md)

-   **Analytics & diagnosis**

    ---

    Connect to your fleet to see what's running, read logs, and fix misbehaving
    devices.

    [See your fleet →](getting-started/guides/connect-a-client.md)

-   **Drive it over HTTP**

    ---

    Run the API gateway and reach any node from curl, a script, a dashboard or a
    browser. AI agents get an MCP server on the same port.

    [Open the gateway →](features/gateway.md)

-   **Customize & extend**

    ---

    Dusk is modular: swap the platform backend or add your own programs when the
    built-ins aren't enough.

    [Extend Dusk →](getting-started/guides/first-program.md)

-   **Reference**

    ---

    The crate map and the Cap'n Proto wire schemas.

    [Reference →](sdk-reference/crates.md)

</div>
