"""Command-line entry point: ``python -m dusk.gw <ip> <port>``, or ``dusk_gw``.

Parses the bind address with click and hands it to :func:`serve`, which blocks
running the dusk API gateway - the ``/v1`` REST API and the ``/mcp`` MCP server.
"""

from __future__ import annotations

import click

from . import serve


@click.command()
@click.argument("ip")
@click.argument("port", type=int)
def main(ip: str, port: int) -> None:
    """Run the dusk API gateway, binding to IP and PORT."""
    serve(ip, port)


if __name__ == "__main__":
    main()
