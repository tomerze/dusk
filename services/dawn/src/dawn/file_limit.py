from __future__ import annotations

import logging
import resource

logger = logging.getLogger(__name__)

RESERVED_DESCRIPTORS = 1024


class FileLimitTooLow(RuntimeError):
    pass


def raise_file_limit(node_sessions: int) -> int:
    required = 2 * node_sessions + RESERVED_DESCRIPTORS
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    if hard != resource.RLIM_INFINITY and hard < required:
        raise FileLimitTooLow(
            f"the open-file hard limit is {hard}, below the {required} that "
            f"limits.max_node_sessions = {node_sessions} needs, two connections per "
            "session and 1024 more; raise the hard limit "
            "(ulimit -Hn, LimitNOFILE=, or the container's nofile ulimit) or lower "
            "limits.max_node_sessions"
        )
    target = hard if hard != resource.RLIM_INFINITY else max(required, soft)
    if soft != target:
        resource.setrlimit(resource.RLIMIT_NOFILE, (target, hard))
    logger.info(
        "raised the open-file limit",
        extra={"soft_before": soft, "limit": target, "required": required},
    )
    return target
