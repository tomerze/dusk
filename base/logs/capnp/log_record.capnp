@0xd06f74de2a9b6c32;

# OpenTelemetry / OTLP log record schema (wire form). Produced by the
# enrichment stage that reads `LogRecord`s out of the log buffers, fills in the
# severity (implied by which buffer held the entry) and normalises the
# timestamps, then converts them to this representation.
#
# Mirrors opentelemetry/proto/logs/v1 (LogRecord) and the AnyValue family from
# opentelemetry/proto/common/v1. The OTLP transport envelope (LogsData /
# ResourceLogs / ScopeLogs / Resource / InstrumentationScope) is intentionally
# not modelled here.

struct LogRecord {
  timeUnixNano @0 :UInt64;
  observedTimeUnixNano @1 :UInt64;
  severityNumber @2 :SeverityNumber;
  severityText @3 :Text;
  body @4 :AnyValue;
  attributes @5 :List(KeyValue);
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

# OTLP AnyValue: the value of a log body or an attribute.
struct AnyValue {
  union {
    stringValue @0 :Text;
    boolValue @1 :Bool;
    intValue @2 :Int64;
    doubleValue @3 :Float64;
    bytesValue @4 :Data;
    arrayValue @5 :ArrayValue;
    kvlistValue @6 :KeyValueList;
  }
}

struct ArrayValue {
  values @0 :List(AnyValue);
}

struct KeyValueList {
  values @0 :List(KeyValue);
}

# OTLP KeyValue: one attribute.
struct KeyValue {
  key @0 :Text;
  value @1 :AnyValue;
}
