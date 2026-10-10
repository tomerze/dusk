import hashlib
import uuid
from collections.abc import Callable
from typing import Any

import pytest
from botocore.exceptions import ClientError

EVIDENCE = "dusk-ledger-evidence"
LAKE = "dusk-lake"
FILES = "dusk-files"
SIGNOZ = "signoz-cold"


def refused(call: Callable[[], Any]) -> str:
    with pytest.raises(ClientError) as failure:
        call()
    return failure.value.response["Error"]["Code"]


def test_every_bucket_exists(object_store: Callable[[str], Any]) -> None:
    names = {
        bucket["Name"] for bucket in object_store("admin").list_buckets()["Buckets"]
    }
    assert {EVIDENCE, LAKE, FILES, SIGNOZ} <= names


def test_the_evidence_bucket_locks_every_object_by_default(
    object_store: Callable[[str], Any],
) -> None:
    configuration = object_store("admin").get_object_lock_configuration(
        Bucket=EVIDENCE
    )["ObjectLockConfiguration"]
    assert configuration["ObjectLockEnabled"] == "Enabled"
    assert configuration["Rule"]["DefaultRetention"] == {
        "Mode": "GOVERNANCE",
        "Days": 1,
    }


def test_the_ledger_writer_may_only_put_evidence(
    object_store: Callable[[str], Any],
) -> None:
    writer = object_store("ledger-writer")
    key = f"verify/{uuid.uuid4()}.jsonl"
    body = b'{"kind":"checkpoint"}\n'
    writer.put_object(Bucket=EVIDENCE, Key=key, Body=body)
    assert (
        refused(lambda: writer.get_object(Bucket=EVIDENCE, Key=key)) == "AccessDenied"
    )
    assert refused(lambda: writer.list_objects_v2(Bucket=EVIDENCE)) == "AccessDenied"
    assert (
        refused(lambda: writer.delete_object(Bucket=EVIDENCE, Key=key))
        == "AccessDenied"
    )
    assert (
        refused(lambda: writer.put_object(Bucket=LAKE, Key=key, Body=body))
        == "AccessDenied"
    )
    admin = object_store("admin")
    assert admin.get_object(Bucket=EVIDENCE, Key=key)["Body"].read() == body
    version = admin.head_object(Bucket=EVIDENCE, Key=key)
    assert version["ObjectLockMode"] == "GOVERNANCE"
    assert refused(
        lambda: admin.delete_object(
            Bucket=EVIDENCE, Key=key, VersionId=version["VersionId"]
        )
    ) == ("AccessDenied")


def test_nobody_else_writes_evidence(object_store: Callable[[str], Any]) -> None:
    for user in ["vector", "dawn", "reader", "signoz"]:
        client = object_store(user)
        assert (
            refused(
                lambda: client.put_object(
                    Bucket=EVIDENCE, Key="forged.jsonl", Body=b"{}"
                )
            )
            == "AccessDenied"
        )


def test_vector_writes_the_lake_and_the_reader_only_reads_it(
    object_store: Callable[[str], Any],
) -> None:
    key = f"verify/dt=2026-10-07/{uuid.uuid4()}.parquet"
    object_store("vector").put_object(Bucket=LAKE, Key=key, Body=b"PAR1")
    reader = object_store("reader")
    assert reader.get_object(Bucket=LAKE, Key=key)["Body"].read() == b"PAR1"
    assert any(
        item["Key"] == key
        for item in reader.list_objects_v2(Bucket=LAKE, Prefix="verify/")["Contents"]
    )
    assert (
        refused(lambda: reader.put_object(Bucket=LAKE, Key=key, Body=b"x"))
        == "AccessDenied"
    )
    assert (
        refused(
            lambda: object_store("dawn").put_object(Bucket=LAKE, Key=key, Body=b"x")
        )
        == "AccessDenied"
    )
    object_store("admin").delete_object(Bucket=LAKE, Key=key)


def test_dawn_uploads_files_it_cannot_read_back(
    object_store: Callable[[str], Any],
) -> None:
    dawn = object_store("dawn")
    body = b"collected file"
    key = f"files/verify/{uuid.uuid4()}/0-syslog"
    upload = dawn.create_multipart_upload(Bucket=FILES, Key=key)
    part = dawn.upload_part(
        Bucket=FILES, Key=key, UploadId=upload["UploadId"], PartNumber=1, Body=body
    )
    dawn.complete_multipart_upload(
        Bucket=FILES,
        Key=key,
        UploadId=upload["UploadId"],
        MultipartUpload={"Parts": [{"ETag": part["ETag"], "PartNumber": 1}]},
    )
    assert refused(lambda: dawn.get_object(Bucket=FILES, Key=key)) == "AccessDenied"
    stored = object_store("admin").get_object(Bucket=FILES, Key=key)["Body"].read()
    assert hashlib.sha256(stored).digest() == hashlib.sha256(body).digest()
    object_store("admin").delete_object(Bucket=FILES, Key=key)


def test_incomplete_uploads_of_collected_files_are_aborted_after_a_day(
    object_store: Callable[[str], Any],
) -> None:
    rules = object_store("admin").get_bucket_lifecycle_configuration(Bucket=FILES)[
        "Rules"
    ]
    assert [
        rule["AbortIncompleteMultipartUpload"]["DaysAfterInitiation"] for rule in rules
    ] == [1]
