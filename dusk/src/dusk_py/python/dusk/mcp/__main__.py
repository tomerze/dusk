"""Command-line entry point: ``python -m dusk.mcp <ip> <port>``.

Parses the bind address with click and hands it to :func:`serve`, which
blocks running the MCP gateway over streamable HTTP.
"""

from __future__ import annotations

import click

from . import serve


@click.command()
@click.argument("ip")
@click.argument("port", type=int)
def main(ip: str, port: int) -> None:
    """Run the dusk MCP gateway, binding to IP and PORT."""
    serve(ip, port)


if __name__ == "__main__":
    main()
