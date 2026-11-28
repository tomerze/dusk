from __future__ import annotations

from conftest import (
    BASE_URL,
    PROGRAMS,
    SERVED_AT,
    FakeConnection,
    FakeConnectionFactory,
)
from fastapi import FastAPI, Request
from pydantic import BaseModel
from starlette.testclient import TestClient
from test_mcp import ACCEPT_BOTH, PROTOCOL_VERSION, call, sole_message


class TicketRequest(BaseModel):
    ticket: str


def test_the_connect_route_is_replaced_by_the_one_supplied():
    from dusk.gw import ConnectionRegistry
    from dusk.gw.rest import build_application

    registry = ConnectionRegistry(FakeConnectionFactory())
    opened: list[str] = []

    def connect_by_ticket(api: FastAPI, registry, owner) -> None:
        @api.post("/connect")
        async def connect(body: TicketRequest, request: Request) -> dict:
            opened.append(body.ticket)
            descriptor, _ = registry.register(
                owner(request), FakeConnection(body.ticket, 1)
            )
            return {"descriptor": descriptor}

    api = build_application(registry, PROGRAMS, connect=connect_by_ticket)
    with TestClient(api) as client:
        rejected = client.post("/connect", json={"host": "10.0.0.1", "port": 9090})
        accepted = client.post("/connect", json={"ticket": "node-a"})
        ran = client.post(
            "/sh", json={"descriptor": accepted.json()["descriptor"], "command": "ps"}
        )
        specification = client.get("/openapi.json").json()

    assert rejected.status_code == 400
    assert accepted.status_code == 200, accepted.text
    assert opened == ["node-a"]
    assert ran.json() == {"output": [{"ran": "ps", "on": "node-a:1"}]}
    body = specification["paths"]["/connect"]["post"]["requestBody"]
    assert body["content"]["application/json"]["schema"]["$ref"].endswith(
        "/TicketRequest"
    )


def test_the_mcp_connect_tool_and_instructions_are_replaced_by_the_ones_supplied():
    from dusk.gw import ConnectionRegistry
    from dusk.gw.mcp import build_application
    from mcp.server.fastmcp import FastMCP

    def connect_by_ticket(server: FastMCP, registry) -> None:
        async def connect(ticket: str) -> str:
            return ticket

        server.add_tool(connect, name="connect", description="by ticket")

    application = build_application(
        ConnectionRegistry(FakeConnectionFactory()),
        PROGRAMS,
        SERVED_AT,
        connect=connect_by_ticket,
        instructions="only tickets",
    )
    with TestClient(application, base_url=BASE_URL) as client:
        initialized = call(
            client,
            "initialize",
            protocolVersion=PROTOCOL_VERSION,
            capabilities={},
            clientInfo={"name": "dusk-gateway-tests", "version": "0"},
        )
        session_id = initialized.headers["mcp-session-id"]
        client.post(
            "/mcp",
            headers={
                "accept": ACCEPT_BOTH,
                "content-type": "application/json",
                "mcp-session-id": session_id,
            },
            json={"jsonrpc": "2.0", "method": "notifications/initialized"},
        )
        tools = sole_message(call(client, "tools/list", session_id))["result"]["tools"]

    assert sole_message(initialized)["result"]["instructions"] == "only tickets"
    connect = next(tool for tool in tools if tool["name"] == "connect")
    assert connect["description"] == "by ticket"
    assert list(connect["inputSchema"]["properties"]) == ["ticket"]
    assert {tool["name"] for tool in tools} == {"connect", "disconnect", "ps", "sleep"}
