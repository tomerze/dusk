@0x9e96cae8d383c911;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";
using StreamResult = import "/capnp/stream.capnp".StreamResult;

# OTel/OTLP log record schema (LogRecord, SeverityNumber, AnyValue family).
using LogRecord = import "/capnp/log_record.capnp";

const programId :UInt64 = 0xa508ae4405044e9d;

struct LogsArgs {
  struct Data {
    # The minimum severity to stream; unspecified streams everything.
    level @0 :LogRecord.SeverityNumber;
  }
  interface Server {
    # Streams log records to the client. Where they land is the client's
    # choice (its command line; the viewer by default) — the node doesn't
    # know. A failed send is retried.
    send @0 (entries :List(LogRecord.LogRecord)) -> StreamResult;
    # Long-poll, the client answers when its time to stop
    stop @1 () -> ();
  }
}

interface LogsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
