from __future__ import annotations

from collections.abc import Callable
from typing import Any, cast

from .nodes import Node, NodeTarget


def connect(target: NodeTarget, sh_server_pid: int | None) -> Node:
    import dusk

    constructor = cast(Callable[..., Node], dusk.Dusk)
    return constructor(
        target.host,
        target.port,
        sh_server_pid,
        server_name=target.server_name,
        ca=str(target.ca),
        certificate=str(target.certificate),
        key=str(target.key),
    )


def programs() -> list[dict[str, Any]]:
    import dusk

    return dusk.Dusk.help()
