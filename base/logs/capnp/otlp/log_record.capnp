@0xd06f74de2a9b6c32;

# OpenTelemetry / OTLP log record schema (wire form). Produced by the
# enrichment stage that reads a `LogRecord` out of the signal buffer, fills in the
# severity (implied by which lane held it) and normalises the timestamps, then
# converts them to this representation.
#
# Mirrors opentelemetry/proto/logs/v1 (LogRecord). The AnyValue / KeyValue family
# (opentelemetry/proto/common/v1) is shared with the other signals, so it is
# imported from common.capnp. The OTLP transport envelope (LogsData /
# ResourceLogs / ScopeLogs / Resource / InstrumentationScope) is intentionally
# not modelled here.
using Common = import "/capnp/common.capnp";

struct LogRecord {
  timeUnixNano @0 :UInt64;
  observedTimeUnixNano @1 :UInt64;
  severityNumber @2 :SeverityNumber;
  severityText @3 :Text;
  body @4 :Common.AnyValue;
  attributes @5 :List(Common.KeyValue);
  droppedAttributesCount @6 :UInt32;
  flags @7 :UInt32;
  traceId @8 :Data;
  spanId @9 :Data;
  eventName @10 :Text;
}

# OTLP SeverityNumber (1-24 grouped by level; 0 = unspecified).
enum SeverityNumber {
  unspecified @0;
  trace @1;
  trace2 @2;
  trace3 @3;
  trace4 @4;
  debug @5;
  debug2 @6;
  debug3 @7;
  debug4 @8;
  info @9;
  info2 @10;
  info3 @11;
  info4 @12;
  warn @13;
  warn2 @14;
  warn3 @15;
  warn4 @16;
  error @17;
  error2 @18;
  error3 @19;
  error4 @20;
  fatal @21;
  fatal2 @22;
  fatal3 @23;
  fatal4 @24;
}
