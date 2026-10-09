from __future__ import annotations

import asyncio
import hashlib
import os
import time

import pytest
from conftest import (
    CAMPAIGN_ID,
    CP_TYPE_ID,
    DEFAULT_SH_PID,
    DEVICE_ID,
    INSTALLATION_ID,
    PID,
    FakeFleet,
    FakeNode,
    RecordingProducer,
    Wait,
    dawn_settings,
    node_ref,
    work_spec,
)
from moto.server import ThreadedMotoServer

from dawn.config import KafkaTopics, S3Settings
from dawn.events import Events
from dawn.files import Files, UploadsExhausted, object_key, s3_client
from dawn.models import FileRequest
from dawn.nodes import Connection, Sessions
from dawn.work import Results, Runner

pytestmark = pytest.mark.anyio

BUCKET = "dusk-files"
MEBIBYTE = 1024 * 1024
PART_BYTES = 5 * MEBIBYTE


@pytest.fixture(scope="module")
def moto_endpoint():
    os.environ.setdefault("AWS_ACCESS_KEY_ID", "testing")
    os.environ.setdefault("AWS_SECRET_ACCESS_KEY", "testing")
    server = ThreadedMotoServer(ip_address="127.0.0.1", port=0, verbose=False)
    server.start()
    host, port = server.get_host_and_port()
    yield f"http://{host}:{port}"
    server.stop()


@pytest.fixture
async def s3(moto_endpoint, tmp_path):
    access = tmp_path / "access"
    secret = tmp_path / "secret"
    access.write_text("testing\n")
    secret.write_text("testing\n")
    settings = S3Settings(
        endpoint=moto_endpoint,
        bucket=BUCKET,
        access_key_file=access,
        secret_key_file=secret,
    )
    async with s3_client(settings) as client:
        try:
            await client.create_bucket(Bucket=BUCKET)
        except client.exceptions.BucketAlreadyOwnedByYou:
            pass
        yield client


UPLOADED: list[int] = []


@pytest.fixture
def uploads(tmp_path):
    directory = tmp_path / "uploads"
    directory.mkdir()
    return directory


def files(
    s3,
    producer: RecordingProducer,
    directory,
    max_file_bytes: int = 1 << 30,
    max_concurrent: int = 4,
    max_staged_bytes: int | None = None,
) -> Files:
    UPLOADED.clear()
    return Files(
        s3,
        BUCKET,
        Results(Events(producer, KafkaTopics(), "dawn-0"), b"k" * 32),
        max_file_bytes,
        max_concurrent,
        max_staged_bytes or max_concurrent * max_file_bytes,
        UPLOADED.append,
        part_bytes=PART_BYTES,
        directory=str(directory),
    )


async def collect(
    collector: Files, node: FakeNode, path: str, index: int = 0, seconds: float = 60
):
    connection = Connection(node.connect(PID), {})
    await collector.collect(
        connection,
        collector._results.attempt(node_ref(), PID, CAMPAIGN_ID, 1, "collect_file"),
        path,
        index,
        time.monotonic() + seconds,
    )


async def outstanding_uploads(s3) -> list:
    listed = await s3.list_multipart_uploads(Bucket=BUCKET)
    return listed.get("Uploads", [])


async def stored(s3, key: str) -> bytes:
    response = await s3.get_object(Bucket=BUCKET, Key=key)
    async with response["Body"] as body:
        return await body.read()


def the_key(path: str, index: int = 0) -> str:
    return (
        f"files/{DEVICE_ID}/{INSTALLATION_ID}/{PID}/{index}-{path.rsplit('/', 1)[-1]}"
    )


def copies(node: FakeNode) -> list[str]:
    return [call[2] for call in node.calls if call[0] == "sh"]


async def test_a_small_file_is_copied_stored_whole_and_announced(s3, producer, uploads):
    node = FakeNode()
    content = b"line one\nline two\n"
    node.files["/var/log/app.log"] = [content[:5], content[5:]]
    collector = files(s3, producer, uploads)

    await collect(collector, node, "/var/log/app.log")

    assert await stored(s3, the_key("/var/log/app.log")) == content
    (announced,) = producer.on("dusk.files")
    assert announced["object_key"] == the_key("/var/log/app.log")
    assert announced["bucket"] == BUCKET
    assert announced["node_path"] == "/var/log/app.log"
    assert announced["size_bytes"] == len(content)
    assert announced["sha256"] == hashlib.sha256(content).hexdigest()
    assert announced["content_type"] == "application/octet-stream"
    assert announced["campaign_id"] == CAMPAIGN_ID
    assert announced["pid"] == str(PID)
    (result,) = producer.results()
    assert result["action_kind"] == "collect_file"
    assert result["status"] == "succeeded"
    assert result["delivered"] is True
    assert result["pid"] == str(PID)
    assert result["attempt"] == 1
    (copy,) = copies(node)
    assert copy.startswith(f"cp ':/var/log/app.log' '{uploads}/dawn-")
    assert UPLOADED == [len(content)]
    assert list(uploads.iterdir()) == []


async def test_a_path_holding_a_quote_is_quoted_with_the_other(s3, producer, uploads):
    node = FakeNode()
    node.files["/var/log/it's here.log"] = [b"x"]

    await collect(files(s3, producer, uploads), node, "/var/log/it's here.log")

    assert copies(node)[0].startswith('cp ":/var/log/it\'s here.log" ')
    assert producer.results()[0]["status"] == "succeeded"


async def test_an_empty_file_is_stored(s3, producer, uploads):
    node = FakeNode()
    node.files["/etc/empty"] = []

    await collect(files(s3, producer, uploads), node, "/etc/empty")

    assert await stored(s3, the_key("/etc/empty")) == b""
    assert producer.on("dusk.files")[0]["size_bytes"] == 0


async def test_a_large_file_streams_through_a_multipart_upload(s3, producer, uploads):
    node = FakeNode()
    chunks = [bytes([index]) * MEBIBYTE for index in range(11)] + [b"tail"]
    node.files["/var/lib/dump.bin"] = chunks

    await collect(files(s3, producer, uploads), node, "/var/lib/dump.bin")

    content = b"".join(chunks)
    assert await stored(s3, the_key("/var/lib/dump.bin")) == content
    head = await s3.head_object(Bucket=BUCKET, Key=the_key("/var/lib/dump.bin"))
    assert head["ETag"].strip('"').endswith("-3")
    assert producer.on("dusk.files")[0]["sha256"] == hashlib.sha256(content).hexdigest()
    assert UPLOADED == [PART_BYTES, PART_BYTES, len(content) - 2 * PART_BYTES]
    assert await outstanding_uploads(s3) == []
    assert list(uploads.iterdir()) == []


async def test_a_copy_that_fails_partway_stores_nothing(s3, producer, uploads):
    node = FakeNode()
    node.files["/var/lib/half.bin"] = [
        b"x" * MEBIBYTE,
        RuntimeError("Failed: couldn't read `/var/lib/half.bin` at 1048576"),
    ]

    await collect(files(s3, producer, uploads), node, "/var/lib/half.bin")

    with pytest.raises(s3.exceptions.NoSuchKey):
        await s3.get_object(Bucket=BUCKET, Key=the_key("/var/lib/half.bin"))
    assert producer.on("dusk.files") == []
    (result,) = producer.results()
    assert result["status"] == "error"
    assert result["delivered"] is True
    assert "couldn't read" in result["error"]
    assert list(uploads.iterdir()) == []


async def test_a_file_the_node_does_not_have_was_tried_on_the_node(
    s3, producer, uploads
):
    await collect(files(s3, producer, uploads), FakeNode(), "/etc/absent")

    (result,) = producer.results()
    assert result["status"] == "error"
    assert result["delivered"] is True
    assert "couldn't open" in result["error"]


async def test_a_collection_dawn_stops_partway_stops_the_copy_and_is_reported(
    s3, producer, uploads
):
    node = FakeNode()
    node.files["/var/lib/slow.bin"] = [b"x" * 100, Wait(30)]
    collecting = asyncio.create_task(
        collect(files(s3, producer, uploads), node, "/var/lib/slow.bin")
    )
    await until(
        lambda: any(uploads.iterdir()) and next(uploads.iterdir()).stat().st_size
    )

    collecting.cancel()
    with pytest.raises(asyncio.CancelledError):
        await collecting

    assert node.connections[-1].disconnected
    assert producer.on("dusk.files") == []
    (result,) = producer.results()
    assert result["action_kind"] == "collect_file"
    assert result["status"] == "error"
    assert result["error"] == "dawn stopped before the file was collected"
    assert list(uploads.iterdir()) == []


def mismatched(connection, words):
    import pathlib

    pathlib.Path(words[2]).write_bytes(b"y" * (6 * MEBIBYTE))
    return [
        {
            CP_TYPE_ID: {
                "source": words[1],
                "destination": words[2],
                "length": 6 * MEBIBYTE,
                "resumed": 0,
                "sha256": "00" * 32,
            }
        }
    ]


async def test_a_file_whose_digest_disagrees_is_not_stored(s3, producer, uploads):
    node = FakeNode()
    node.commands["cp"] = mismatched

    await collect(files(s3, producer, uploads), node, "/var/lib/odd.bin")

    assert await outstanding_uploads(s3) == []
    with pytest.raises(s3.exceptions.NoSuchKey):
        await s3.get_object(Bucket=BUCKET, Key=the_key("/var/lib/odd.bin"))
    assert producer.on("dusk.files") == []
    assert "SHA-256" in producer.results()[0]["error"]


async def test_a_file_larger_than_allowed_is_not_stored(s3, producer, uploads):
    node = FakeNode()
    node.files["/var/lib/big.bin"] = [b"z" * 600, b"z" * 600]

    await collect(
        files(s3, producer, uploads, max_file_bytes=1000), node, "/var/lib/big.bin"
    )

    assert producer.on("dusk.files") == []
    assert "limits.max_file_bytes" in producer.results()[0]["error"]
    assert list(uploads.iterdir()) == []


async def test_a_copy_that_grows_past_the_limit_is_stopped_while_it_runs(
    s3, producer, uploads
):
    node = FakeNode()
    node.files["/var/lib/endless.bin"] = [b"z" * 2000, Wait(30), b"z"]

    await collect(
        files(s3, producer, uploads, max_file_bytes=1000), node, "/var/lib/endless.bin"
    )

    assert node.connections[-1].disconnected
    assert "limits.max_file_bytes" in producer.results()[0]["error"]
    assert list(uploads.iterdir()) == []


async def test_a_copy_nightfall_refuses_is_denied(s3, producer, uploads):
    node = FakeNode()
    node.files["/etc/shadow"] = RuntimeError(
        "Unimplemented: remote exception: not permitted: CpPortal.cp"
    )

    await collect(files(s3, producer, uploads), node, "/etc/shadow")

    (result,) = producer.results()
    assert result["status"] == "denied"
    assert result["delivered"] is True


async def test_a_file_the_node_may_not_read_is_an_error_not_a_denial(
    s3, producer, uploads
):
    node = FakeNode()
    node.files["/root/secret"] = RuntimeError(
        "Failed: remote exception: remote exception: couldn't open `/root/secret`: "
        "Permission denied (os error 13)"
    )

    await collect(files(s3, producer, uploads), node, "/root/secret")

    (result,) = producer.results()
    assert result["status"] == "error"


async def test_a_collection_past_its_deadline_times_out(s3, producer, uploads):
    node = FakeNode()
    node.files["/etc/hosts"] = [b"x"]

    await collect(files(s3, producer, uploads), node, "/etc/hosts", seconds=-1)

    assert producer.results()[0]["status"] == "timed_out"


async def test_work_collects_each_file_under_its_position_in_its_shell(
    s3, producer, uploads
):
    fleet = FakeFleet()
    node = fleet.add(FakeNode())
    node.files["/a/first.log"] = [b"1"]
    node.files["/b/second.log"] = [b"2"]
    runner = Runner(
        dawn_settings(),
        fleet,
        Results(Events(producer, KafkaTopics(), "dawn-0"), b"k" * 32),
        files(s3, producer, uploads),
    )

    await runner.run(
        node_ref(), work_spec(collect_files=["/a/first.log", "/b/second.log"])
    )

    keys = [message["object_key"] for message in producer.on("dusk.files")]
    assert keys == [the_key("/a/first.log", 0), the_key("/b/second.log", 1)]
    statuses = [
        (message["action_kind"], message["status"]) for message in producer.results()
    ]
    assert statuses == [
        ("run_script", "started"),
        ("collect_file", "succeeded"),
        ("collect_file", "succeeded"),
        ("run_script", "succeeded"),
    ]
    assert all(
        message["pid"] == str(PID) and message["attempt"] == 1
        for message in producer.results()
    )
    copied = [call for call in node.calls if call[0] == "sh" and call[2][:3] == "cp "]
    assert [call[1] for call in copied] == [PID, PID]


async def test_a_failed_scripts_result_comes_after_its_files_results(
    s3, producer, uploads
):
    fleet = FakeFleet()
    node = fleet.add(FakeNode())
    node.files["/a/first.log"] = [b"1"]
    node.scripts["false"] = [RuntimeError("Failed: program exited with error")]
    runner = Runner(
        dawn_settings(),
        fleet,
        Results(Events(producer, KafkaTopics(), "dawn-0"), b"k" * 32),
        files(s3, producer, uploads),
    )

    await runner.run(
        node_ref(), work_spec(script="false", collect_files=["/a/first.log"])
    )

    statuses = [
        (message["action_kind"], message["status"]) for message in producer.results()
    ]
    assert statuses == [
        ("run_script", "started"),
        ("collect_file", "succeeded"),
        ("run_script", "failed"),
    ]


def test_the_object_key_is_the_contracts():
    key = object_key(node_ref(), PID, 15, "C:\\Logs\\setup.log")

    assert key == f"files/{DEVICE_ID}/{INSTALLATION_ID}/{PID}/15-setup.log"


def file_request(**fields) -> FileRequest:
    values = {
        "node": node_ref(),
        "pid": str(PID),
        "path": "/var/log/app.log",
        "campaign_id": None,
    }
    values.update(fields)
    return FileRequest(**values)


async def settled(collector: Files) -> None:
    await collector.drain(10)


async def test_a_standalone_collection_collects_in_the_shell_at_its_pid_and_reaps_it(
    s3, producer, fleet: FakeFleet, uploads
):
    node = fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"hello"]
    sessions = Sessions(10)
    collector = files(s3, producer, uploads)

    upload_id = collector.start(file_request(), dawn_settings(), fleet, sessions)
    assert sessions.open == 1
    await settled(collector)

    assert len(upload_id) == 36
    assert await stored(s3, the_key("/var/log/app.log")) == b"hello"
    assert producer.results()[0]["status"] == "succeeded"
    assert producer.results()[0]["campaign_id"] is None
    assert producer.results()[0]["attempt"] is None
    assert node.calls[0] == ("connect", PID)
    assert all(connection.disconnected for connection in node.connections)
    assert node.commands_in(DEFAULT_SH_PID) == [
        f"kill {PID:#x}; ps",
        f"kill --signal 8 {PID:#x}; ps",
    ]
    assert PID not in node.processes
    assert sessions.open == 0


async def test_a_standalone_collection_from_an_unreachable_node_reports_it(
    s3, producer, fleet, uploads
):
    sessions = Sessions(10)
    collector = files(s3, producer, uploads)

    collector.start(file_request(), dawn_settings(), fleet, sessions)
    await settled(collector)

    (result,) = producer.results()
    assert result["status"] == "unreachable"
    assert result["delivered"] is False
    assert sessions.open == 0


async def test_a_standalone_collection_resent_while_it_runs_is_the_same_one(
    s3, producer, fleet, uploads
):
    node = fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"x"]
    fleet.delay = 0.2
    collector = files(s3, producer, uploads)
    sessions = Sessions(10)

    first = collector.start(file_request(), dawn_settings(), fleet, sessions)
    again = collector.start(file_request(), dawn_settings(), fleet, sessions)
    await settled(collector)

    assert again == first
    assert len(producer.results()) == 1


async def test_another_path_under_the_same_pid_is_another_collection(
    s3, producer, fleet, uploads
):
    node = fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"x"]
    node.files["/var/log/other.log"] = [b"y"]
    fleet.delay = 0.2
    collector = files(s3, producer, uploads)
    sessions = Sessions(10)

    first = collector.start(file_request(), dawn_settings(), fleet, sessions)
    other = collector.start(
        file_request(path="/var/log/other.log"), dawn_settings(), fleet, sessions
    )
    await settled(collector)

    assert other != first
    assert len(producer.on("dusk.files")) == 2


async def test_a_standalone_collection_stopped_while_connecting_is_reported(
    s3, producer, fleet, uploads
):
    fleet.add(FakeNode())
    fleet.delay = 0.5
    collector = files(s3, producer, uploads)
    sessions = Sessions(10)

    collector.start(file_request(), dawn_settings(), fleet, sessions)
    await asyncio.sleep(0.05)
    await collector.drain(0)

    (result,) = producer.results()
    assert result["status"] == "error"
    assert result["error"] == "dawn stopped before the file was collected"
    assert sessions.open == 0


async def test_standalone_collections_are_bounded(s3, producer, fleet, uploads):
    node = fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"x"]
    fleet.delay = 0.2
    collector = files(s3, producer, uploads, max_concurrent=1)
    sessions = Sessions(10)

    collector.start(file_request(), dawn_settings(), fleet, sessions)
    with pytest.raises(UploadsExhausted):
        collector.start(
            file_request(pid=str(PID + 1)),
            dawn_settings(),
            fleet,
            sessions,
        )
    await settled(collector)
    await asyncio.sleep(0)

    assert sessions.open == 0


async def test_staging_space_bounds_the_collections_at_once(
    s3, producer, fleet, uploads
):
    node = fleet.add(FakeNode())
    node.files["/var/log/app.log"] = [b"x"]
    fleet.delay = 0.2
    collector = files(s3, producer, uploads, max_file_bytes=1000, max_staged_bytes=2500)
    sessions = Sessions(10)

    for offset in range(2):
        collector.start(
            file_request(pid=str(PID + offset)), dawn_settings(), fleet, sessions
        )
    with pytest.raises(UploadsExhausted, match="limits.max_staged_bytes"):
        collector.start(
            file_request(pid=str(PID + 2)), dawn_settings(), fleet, sessions
        )
    await settled(collector)


async def until(condition, seconds: float = 5.0) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + seconds
    while not condition():
        assert loop.time() < deadline, "the condition never held"
        await asyncio.sleep(0.01)
