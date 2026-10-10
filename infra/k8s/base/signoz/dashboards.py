import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

SCHEMA_VERSION = "v6"
PAGE_SIZE = 200
READY_SECONDS = 600


class SigNozError(Exception):
    pass


def call(method: str, url: str, body: Any = None, token: str | None = None) -> Any:
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers=headers,
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            content = response.read()
    except urllib.error.HTTPError as failure:
        raise SigNozError(
            f"{method} {urllib.parse.urlsplit(url).path}: {failure.code} {failure.read().decode(errors='replace')}"
        ) from failure
    return json.loads(content) if content else None


def log_in(url: str, email: str, password: str) -> str:
    context = call(
        "GET",
        f"{url}/api/v2/sessions/context?"
        + urllib.parse.urlencode({"email": email, "ref": url}),
    )
    organizations = context["data"]["orgs"] or []
    if len(organizations) != 1:
        raise SigNozError(
            f"{email} belongs to {len(organizations)} organizations, expected one"
        )
    session = call(
        "POST",
        f"{url}/api/v2/sessions/email_password",
        {"email": email, "password": password, "orgId": organizations[0]["id"]},
    )
    return session["data"]["accessToken"]


def log_in_when_ready(url: str, email: str, password: str) -> str:
    deadline = time.monotonic() + READY_SECONDS
    delay = 1.0
    while True:
        try:
            return log_in(url, email, password)
        except (SigNozError, OSError) as failure:
            if time.monotonic() > deadline:
                raise SigNozError(
                    f"SigNoz at {url} did not accept the login within {READY_SECONDS} seconds: {failure}"
                ) from failure
            print(f"waiting for SigNoz: {failure}", file=sys.stderr, flush=True)
            time.sleep(delay)
            delay = min(delay * 2, 15.0)


def existing_dashboards(url: str, token: str) -> dict[str, str]:
    found: dict[str, str] = {}
    offset = 0
    while True:
        page = call(
            "GET",
            f"{url}/api/v2/dashboards?"
            + urllib.parse.urlencode({"limit": PAGE_SIZE, "offset": offset}),
            token=token,
        )["data"]
        for dashboard in page["dashboards"]:
            found[dashboard["name"]] = dashboard["id"]
        offset += len(page["dashboards"])
        if not page["dashboards"] or offset >= page["total"]:
            return found


def provision(url: str, email: str, password: str, directory: Path) -> None:
    files = sorted(directory.glob("*.json"))
    if not files:
        raise SigNozError(f"no dashboards in {directory}")
    token = log_in_when_ready(url, email, password)
    existing = existing_dashboards(url, token)
    for path in files:
        exported = json.loads(path.read_text())
        name = f"dusk-{path.stem}"
        body = {
            "schemaVersion": SCHEMA_VERSION,
            "name": name,
            "image": exported["image"],
            "tags": exported["tags"],
            "spec": exported["spec"],
        }
        if name in existing:
            call("PUT", f"{url}/api/v2/dashboards/{existing[name]}", body, token)
            print(f"dashboard {name} updated", flush=True)
        else:
            call("POST", f"{url}/api/v2/dashboards", body, token)
            print(f"dashboard {name} created", flush=True)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: dashboards.py <directory of exported dashboards>")
    try:
        provision(
            os.environ["SIGNOZ_URL"].rstrip("/"),
            os.environ["SIGNOZ_EMAIL"],
            Path(os.environ["SIGNOZ_PASSWORD_FILE"]).read_text().strip(),
            Path(sys.argv[1]),
        )
    except SigNozError as failure:
        raise SystemExit(str(failure)) from failure


if __name__ == "__main__":
    main()
