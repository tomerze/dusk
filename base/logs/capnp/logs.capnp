@0x9e96cae8d383c911;

using Dusk = import "/capnp/dusk.capnp";
using Sh = import "/capnp/sh.capnp";

# OTel/OTLP log record schema (LogRecord, SeverityNumber, AnyValue family).
using LogRecord = import "/capnp/log_record.capnp";

const programId :UInt64 = 0xa508ae4405044e9d;

struct LogsArgs {
  struct Data {}
  interface Server {}
}

interface LogsPortal extends(Dusk.Portal, Sh.OutputPortal) {}
