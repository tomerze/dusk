# The nightfall ledger

This page is for security auditors: people who check, after the fact, what was
done to the nodes of a fleet and whether the record of it has been altered.
It describes what the ledger records, how each entry is hashed into a chain, how
checkpoints are signed, how to verify all of it, and what the ledger can and
cannot reveal.

nightfall is the service every Dusk node in a fleet connects to and every client
(dawn) reaches nodes through. Because every call to a node crosses nightfall,
nightfall is where the ledger is written.

## What is recorded

One entry for each of these:

* every call a client makes on a node through nightfall, and every call a node
  makes back to a client;
* the result of every forwarded call;
* every call nightfall refuses (`denied`) or holds back by a rate limit
  (`rate_limited`) - one entry, with no result entry after it, because the call
  never reached the node. A call refused because no process twilight intended
  allows it names the [admission rule](membrane.md#admission) in its
  `event_detail`;
* the calls nightfall itself makes on a node while it sets up a session
  (`setup_call` events, principal `nightfall`);
* node sessions opening, closing and being refused, a client's access being
  dropped, an RPC message being rejected, a role using a quarantine override, and
  a role exempt from admission making a call no intended process allows
  (`admission_override`);
* the chain's own events (`chain_link`, `chain_resumed`) and signed checkpoints.

No entry holds a parameter value, a result value or command output. A call entry
holds the names of the parameter fields, which of them were redacted, and
`param_hash`, an HMAC of the parameters (see [Parameter hashes](#parameter-hashes)).
Heartbeats are not recorded.

## Where it is

nightfall writes the ledger to the Kafka topic `dusk.ledger`. Each nightfall
instance writes to its own partition (by default the number at the end of its
instance name, the StatefulSet ordinal) with the instance name as the record key,
in Kafka transactions. Read the topic with `isolation.level=read_committed`;
otherwise you also see records of transactions that were aborted and never became
part of the ledger. Kafka keeps the topic for 30 days.

Every record is one entry as JSON. nightfall writes it in its canonical form (the
next section), but any copy that keeps the values - a ClickHouse table, an object
store holding the records as JSON lines - verifies the same way, because the hash
is taken over the canonical form, not over the bytes a copy happens to store.
Verification needs the entries of each partition in their original order.

## The entry

Every entry has all of these fields; a field with no value is `null`, never
absent. `services/contracts/kafka/dusk.ledger.schema.json` in the Dusk repository
is the authoritative definition, including which fields each kind of entry
carries.

| Field | Meaning |
|-------|---------|
| `schema` | `dusk.ledger/v1` |
| `id` | A UUID v7 naming this entry. |
| `time` | When nightfall created the entry, RFC 3339 UTC with nine fractional digits. |
| `kind` | `call`, `result`, `event` or `checkpoint`. |
| `instance`, `partition` | The nightfall instance and the Kafka partition it writes. Together they name the chain. |
| `sequence` | The entry's position in its chain, from 0. |
| `previous_hash` | `hash` of the previous entry of the chain; 64 zeros on the first entry of a chain. |
| `hash` | SHA-256, as 64 lowercase hex digits, of the entry's canonical bytes with `hash` left out. |
| `device_id`, `installation_id` | The node's identity from its certificate (32 lowercase hex digits each). |
| `namespace_id` | The node's running Dusk instance, 16 lowercase hex digits. |
| `epoch` | The session's epoch. |
| `principal` | Who made the call: the client's certificate principal, or `nightfall`. |
| `pid` | The process the call is about, as a decimal string: the fixed pid a `Dusk.process` named, the pid a `Dusk.kill` or `Dusk.waitpid` names, or the pid of the process every capability of the call descends from. `"0"` when the call is about no process. A process is the unit of work on a node, so this ties every call to the work it belongs to. |
| `intent_campaign_id`, `intent_principal`, `intent_subject` | The campaign, the principal who asked and the subject (`campaign:<id>` or the operator) of the process twilight intended at `pid`, as nightfall held it when it saw the call. `null` when it held none - on pid `"0"`, in the default shell, on checkpoints and on calls refused for want of an intended process. twilight keeps its own record of an intended process for weeks; these fields keep the answer to who asked in the ledger, and in its evidence copy, for as long as they are kept. |
| `session_id` | The client connection the call arrived on. |
| `call_id` | Shared by a call and its result. |
| `cap_id`, `parent_cap_id` | The capability the call was made on and the one it was derived from, so the entries of a session form a tree. |
| `direction` | `client_to_node`, `node_to_client` or `nightfall_to_node`. |
| `action`, `interface_id`, `method_id` | The method called, by name (`ShPortal.sh`) and by number. A call on an interface nightfall does not know is `unknown:<interface id>.<method id>` and always `denied`. |
| `param_fields` | The parameter fields by name, and whether each was redacted. |
| `param_cap_ids` | The capabilities passed as parameters. |
| `param_hash` | The parameter HMAC on calls; `""` on results, events and checkpoints. |
| `result_code` | `ok`, `error:failed`, `error:overloaded`, `error:disconnected`, `error:unimplemented` or `revoked` on results; `denied` or `rate_limited` on refused calls; `null` on calls that were forwarded. |
| `result_cap_ids` | The capabilities a result returned. |
| `event`, `event_detail` | The event an `event` entry records, and what it carries. A `denied` call refused by admission carries `{"rule": ...}` in `event_detail`; every other non-event entry carries `null`. |
| `key_id`, `signature` | Checkpoints only: which key signed and the signature. |

These three records are a call, its result and the checkpoint after them, exactly
as nightfall writes them (one JSON object per line). Save them as
`example.jsonl`; the checks below verify them.

```json
{"action":"ShPortal.sh","call_id":"0192f3a4-a001-7b2c-9d3e-4f5061728394","cap_id":7,"device_id":"3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13","direction":"client_to_node","epoch":1791278043512408,"event":null,"event_detail":null,"hash":"6e00fa298c5c31816638e88df022081a764dd5ed2e9a137e19b24d6c7aadbd93","id":"01a112fe-d1ba-7684-ae83-5239bd173f3d","installation_id":"a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70","instance":"nightfall-0","intent_campaign_id":"0192f3a4-5b6c-7d8e-9f01-23456789abcd","intent_principal":"token:0192f3a4-1111-7d8e-9f01-23456789abcd","intent_subject":"campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd","interface_id":"e1c5b0f3a7d29c48","key_id":null,"kind":"call","method_id":0,"namespace_id":"5d2e9a1c7b3f8e04","param_cap_ids":[8],"param_fields":[{"name":"script","redacted":false}],"param_hash":"1f3e5d7c9b0a2f4e6d8c1b3a5f7e9d0c2b4a6f8e1d3c5b7a9f0e2d4c6b8a1f3e","parent_cap_id":3,"partition":0,"pid":"11259529557207498630","previous_hash":"0000000000000000000000000000000000000000000000000000000000000000","principal":"dawn-0","result_cap_ids":[],"result_code":null,"schema":"dusk.ledger/v1","sequence":0,"session_id":"0192f3a4-9e8d-7c6b-8a59-483726150f1e","signature":null,"time":"2026-10-06T09:15:40.512408217Z"}
{"action":"ShPortal.sh","call_id":"0192f3a4-a001-7b2c-9d3e-4f5061728394","cap_id":7,"device_id":"3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13","direction":"client_to_node","epoch":1791278043512408,"event":null,"event_detail":null,"hash":"57c1d2cfe0453f2c832ed5741488974c3e869fb5ffc5784116fd97ffe746d67c","id":"01a112fe-d1c4-7a0e-9b31-0c6f2d8e4a57","installation_id":"a41e6c2f9b0d4e7a8c3f5b1d2e9a6c70","instance":"nightfall-0","intent_campaign_id":"0192f3a4-5b6c-7d8e-9f01-23456789abcd","intent_principal":"token:0192f3a4-1111-7d8e-9f01-23456789abcd","intent_subject":"campaign:0192f3a4-5b6c-7d8e-9f01-23456789abcd","interface_id":"e1c5b0f3a7d29c48","key_id":null,"kind":"result","method_id":0,"namespace_id":"5d2e9a1c7b3f8e04","param_cap_ids":[],"param_fields":[],"param_hash":"","parent_cap_id":3,"partition":0,"pid":"11259529557207498630","previous_hash":"6e00fa298c5c31816638e88df022081a764dd5ed2e9a137e19b24d6c7aadbd93","principal":"dawn-0","result_cap_ids":[9],"result_code":"ok","schema":"dusk.ledger/v1","sequence":1,"session_id":"0192f3a4-9e8d-7c6b-8a59-483726150f1e","signature":null,"time":"2026-10-06T09:15:40.531870044Z"}
{"action":null,"call_id":null,"cap_id":null,"device_id":null,"direction":null,"epoch":null,"event":null,"event_detail":null,"hash":"695914db1efc572f2e5cecde804ab15892521a28e87b01f4a7c9e60df1b8fb7a","id":"01a112fe-d2a9-7f13-8e64-91b0c3d5e7f2","installation_id":null,"instance":"nightfall-0","intent_campaign_id":null,"intent_principal":null,"intent_subject":null,"interface_id":null,"key_id":"HGdXo-ksSpjj5w_-LHDCgukLPgv6Byy8KLG2iR_eOsg","kind":"checkpoint","method_id":null,"namespace_id":null,"param_cap_ids":[],"param_fields":[],"param_hash":"","parent_cap_id":null,"partition":0,"pid":"0","previous_hash":"57c1d2cfe0453f2c832ed5741488974c3e869fb5ffc5784116fd97ffe746d67c","principal":"nightfall","result_cap_ids":[],"result_code":null,"schema":"dusk.ledger/v1","sequence":2,"session_id":null,"signature":"uEFhweyyaouKhPReSXKJGFLUF+5S1Mx+uHLA++L/1ZFR69tu5xbk2nLiWuAjUciBhGrRVOKwHeRtEFkVPhjeAw==","time":"2026-10-06T09:15:41.512901336Z"}
```

## Hashes and the chain

An entry's canonical bytes are its JSON in the form RFC 8785 (the JSON
Canonicalization Scheme) defines: members sorted by name, no whitespace, strings
escaped minimally, numbers written as ECMAScript writes them. `hash` is the
SHA-256 of the canonical bytes of the entry without its `hash` member.

Each nightfall instance keeps one chain per partition it writes. The first entry
has `sequence` 0 and `previous_hash` of 64 zeros; every later entry has the next
sequence number and the previous entry's `hash`. So changing any entry changes its
hash, and the next entry no longer names it.

How a chain continues across the life of an instance:

* **Restart.** When an instance starts, it reads the last entry of its partition.
  If that entry is its own (same `instance`), it continues the chain from it.
* **Another instance takes the partition.** If the last entry belongs to another
  instance name, the new instance starts a new chain at sequence 0, and that first
  entry is a `chain_link` event whose `event_detail` names the entry it follows:
  `{"instance": ..., "sequence": ..., "hash": ...}`. A chain start that follows an
  earlier entry on the partition without such a link is a break.
* **Producer failure.** When Kafka reports a fatal error, nightfall stops
  forwarding calls, creates a new producer, reads the partition's last entries,
  keeps the queued entries Kafka already holds (it matches them by `id`) and writes
  a `chain_resumed` event naming the last entry Kafka holds,
  `{"sequence": ..., "hash": ...}`, before writing the rest. Those entries keep
  their `id` and `time` and get new sequence numbers.

A Kafka consumer can see a record twice. An entry identical to one already seen
(same chain, `sequence` and `hash`) is a duplicate, not a break.

nightfall writes the entries of one transaction within 5 ms or 500 entries,
whichever comes first. A client's call on a node that is not a streaming call is
forwarded only after the transaction holding its `call` entry has committed; if
that takes more than 5 seconds the call fails with `overloaded` and is not
forwarded. Results, calls from nodes to clients and streaming calls are recorded
without holding the call back.

## Checkpoints

nightfall signs its chain with an Ed25519 key that never leaves the hosts nightfall
runs on. A checkpoint is an entry of kind `checkpoint` whose `signature` is the
base64 Ed25519 signature over the canonical bytes of
`{"hash": ..., "instance": ..., "partition": ..., "sequence": ...}` of the entry
before it, and whose `key_id` is the RFC 7638 thumbprint of the signing key's
public JWK. The checkpoint is itself part of the chain. nightfall writes one every
`checkpoint_interval_ms` (1000 by default) when entries were written since the
last one, and one when it shuts down cleanly.

To verify checkpoints you need the public keys as a JWKS, one OKP Ed25519 key per
signing key with its thumbprint as `kid`. The operator who holds the signing key
file (`ledger.signing_key_file`, a PKCS#8 PEM) derives it with OpenSSL 3 and
Python 3 without the private key leaving their machine:

```sh
openssl pkey -in ledger-signing.key -pubout -outform DER | python3 -c '
import base64, hashlib, json, sys
x = base64.urlsafe_b64encode(sys.stdin.buffer.read()[-32:]).rstrip(b"=").decode()
members = json.dumps({"crv": "Ed25519", "kty": "OKP", "x": x}, separators=(",", ":"), sort_keys=True)
kid = base64.urlsafe_b64encode(hashlib.sha256(members.encode()).digest()).rstrip(b"=").decode()
print(json.dumps({"keys": [{"kty": "OKP", "crv": "Ed25519", "x": x, "kid": kid}]}))' > ledger-public-keys.json
```

The key that signed the example above has this JWKS:

```json
{"keys": [{"kty": "OKP", "crv": "Ed25519", "x": "UvDGaapsy13txwCGfG7Fb68E8AjhkThX2TZL3ZCpzKE", "kid": "HGdXo-ksSpjj5w_-LHDCgukLPgv6Byy8KLG2iR_eOsg"}]}
```

Keep the public keys of retired signing keys: the checkpoints they signed stay in
the ledger.

## Verifying

### With nightfall

`nightfall verify-ledger` reads entries as JSON lines from a file or standard
input, or a range of a `dusk.ledger` partition from Kafka, verifies every chain and
checkpoint against the public keys you give it, and exits with a non-zero status
when it finds a break. `nightfall verify-ledger --help` lists its options. Its
report starts with one of these lines:

```text
ledger verified: 3 entries in 1 chains, 1 checkpoints, 0 duplicates
ledger verification failed: 1 breaks in 3 records
```

A failure is followed by the first break and every further one (up to 10 000), as
`first break: record <n>: <kind> instance <instance> partition <partition>
sequence <sequence>: <detail>`, where `record` counts input records from 1. Then
one line per chain gives its sequence range, its number of entries, its last
checkpoint and how many entries follow that checkpoint.

| Break | What it means |
|-------|---------------|
| `unparseable` | The record is not JSON or not a ledger entry. |
| `hash_mismatch` | The entry's content does not hash to its `hash`: it was changed. |
| `previous_hash_mismatch` | The entry does not name the entry before it: an entry was changed and rehashed, or replaced. |
| `sequence_gap` | Sequence numbers are missing: entries were removed. |
| `out_of_order` | An entry arrived after a later one. |
| `conflicting_entry` | Two different entries claim the same sequence number. |
| `stale_entry` | An entry older than the 65 536 most recent of its chain arrived again, so it cannot be told apart from a conflict. |
| `invalid_chain_start` | An entry with sequence 0 does not have 64 zeros as `previous_hash`. |
| `unlinked_chain_start` | A chain starts after another chain on the partition without a `chain_link`. |
| `chain_link_mismatch` | A `chain_link` names an entry other than the one before it on the partition. |
| `chain_resumed_mismatch` | A `chain_resumed` names an entry other than the one before it. |
| `unknown_key` | A checkpoint names a `key_id` none of your public keys has. |
| `forged_checkpoint` | A checkpoint's signature does not verify, or it carries none. |

### Independently

You do not have to trust nightfall's verifier. This script needs only Python 3 and
OpenSSL 3. It checks one chain read in order - hashes, sequence numbers,
`previous_hash` and checkpoint signatures - and stops at the first problem. It
canonicalizes with Python's `json` module, which produces the RFC 8785 bytes for
any entry whose member names are ASCII and whose numbers are integers no larger
than 2^53; nightfall writes only such entries. Save it as `check_ledger.py`:

```python
import base64
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def verify_signature(public_key, message, signature):
    spki = bytes.fromhex("302a300506032b6570032100") + public_key
    with tempfile.TemporaryDirectory() as directory:
        folder = pathlib.Path(directory)
        pem = "-----BEGIN PUBLIC KEY-----\n" + base64.b64encode(spki).decode() + "\n-----END PUBLIC KEY-----\n"
        (folder / "key.pem").write_text(pem)
        (folder / "message").write_bytes(message)
        (folder / "signature").write_bytes(signature)
        command = ["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", folder / "key.pem", "-rawin",
                   "-in", folder / "message", "-sigfile", folder / "signature"]
        return subprocess.run(command, capture_output=True).returncode == 0


records, jwks = sys.argv[1], sys.argv[2]
keys = {key["kid"]: base64.urlsafe_b64decode(key["x"] + "==") for key in json.load(open(jwks))["keys"]}
previous = None
number = 0
for number, line in enumerate(open(records, encoding="utf-8"), start=1):
    entry = json.loads(line)
    stored = entry.pop("hash")
    if hashlib.sha256(canonical(entry)).hexdigest() != stored:
        sys.exit(f"record {number}: hash mismatch")
    if previous is not None and entry["sequence"] != previous["sequence"] + 1:
        sys.exit(f"record {number}: sequence {entry['sequence']} follows {previous['sequence']}")
    if previous is not None and entry["previous_hash"] != previous["hash"]:
        sys.exit(f"record {number}: previous_hash mismatch")
    if entry["kind"] == "checkpoint":
        if previous is None or entry["key_id"] not in keys:
            sys.exit(f"record {number}: checkpoint without a preceding entry or with an unknown key_id")
        signed = {name: previous[name] for name in ("instance", "partition", "sequence", "hash")}
        signature = base64.b64decode(entry["signature"])
        if not verify_signature(keys[entry["key_id"]], canonical(signed), signature):
            sys.exit(f"record {number}: checkpoint signature does not verify")
    entry["hash"] = stored
    previous = entry
print(f"{number} records verified")
```

```sh
python3 check_ledger.py example.jsonl ledger-public-keys.json
```

prints `3 records verified`. Change `"principal":"dawn-0"` to `"dawn-1"` in the
first record and it prints `record 1: hash mismatch`; recompute every hash after
that change and the checkpoint stops it with `record 3: checkpoint signature does
not verify`.

## Parameter hashes

`param_hash` is HMAC-SHA256, keyed with nightfall's param key, over the canonical
Cap'n Proto encoding of the call's parameters after every field marked sensitive
in the schema and every capability has been cleared. Without the key the hash
reveals nothing about the parameters except whether two calls had the same ones:
equal parameters give equal hashes. An investigator who holds the key and has a candidate set of parameters
recomputes the hash with `nightfall ledger-hash --params <file>` and compares.

## What the ledger can and cannot show

The chain and its checkpoints reveal, in any copy of the ledger:

* an entry that was changed, whether or not its hash was recomputed;
* an entry removed from the middle of a chain;
* entries put in a different order, or an entry inserted;
* a chain rewritten from some entry on, at the first checkpoint after that entry,
  unless whoever rewrote it holds the signing key;
* a chain started on a partition without naming the entry before it.

They cannot reveal, by design:

* **Entries never written.** nightfall writes the ledger. A compromised nightfall
  can leave out entries for the sessions it serves. twilight's reconciler, which
  compares every ledger call with the process twilight intended at its pid, and
  the node's own logs are where that shows.
* **The newest entries removed.** Entries after the last checkpoint can be cut off
  without a trace. While nightfall runs that is at most `checkpoint_interval_ms` of
  entries (one second by default); the next checkpoint covers them.
* **A tail removed together with its checkpoints.** What remains is a valid,
  shorter chain. Only a record of where the chain had already reached shows it:
  twilight's reconciler keeps the head of every chain it has read, and the
  evidence copy of the ledger is kept in an object-locked bucket that cannot be
  rewritten.
* **Forgery by a holder of the signing key.** Whoever holds a nightfall
  instance's signing key can sign a chain of their choosing. Checkpoints prove
  that the key holder wrote the chain, not that it is complete.
* **Parameters and results.** The ledger records that a call was made, by whom, on
  what and with what outcome; what was passed and what came back stay out of it.
