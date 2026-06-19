@0x9e96cae8d383c911;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";
using StreamResult = import "/capnp/stream.capnp".StreamResult;

# OTel/OTLP log record schema (LogRecord, SeverityNumber, AnyValue family).
using LogRecord = import "/capnp/log_record.capnp";
# OTLP span schema (Span, Status).
using Span = import "/capnp/span.capnp";

const programId :UInt64 = 0xa508ae4405044e9d;

const batchTypeId :UInt64 = 0xb3d9f4a05c7e2186;

const flagReplay :UInt8 = 1;
const flagFollow :UInt8 = 2;
const flagDump :UInt8 = 4;

# One streamed signal: an OTLP log record or an OTLP span.
struct Signal {
  severityNumber @0 :LogRecord.SeverityNumber;
  globalSequence @3 :UInt64;
  union {
    logRecord @1 :LogRecord.LogRecord;
    span @2 :Span.Span;
  }
}

struct SignalBatch {
    interface Ack {
        ack @0 () -> ();
    }
    signals @0 :List(Signal);
    ack @1 :Ack;
}

struct LogsArgs {
  struct Data {
    level @0 :LogRecord.SeverityNumber;
    flags @1 :UInt8;
  }
  interface Server {
    # Streams signals (log records and spans) to the client. Where they land is
    # the client's choice (its command line; the viewer by default) — the node
    # doesn't know. A failed send is retried.
    send @0 (signal_batch :SignalBatch) -> StreamResult;
    # Long-poll, the client answers when its time to stop
    stop @1 () -> ();
  }
}

interface LogsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
