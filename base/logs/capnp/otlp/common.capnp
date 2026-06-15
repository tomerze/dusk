@0x8f3a2b1c5d6e4790;

# OpenTelemetry / OTLP common types (opentelemetry/proto/common/v1/common.proto):
# the AnyValue / KeyValue family shared by every signal (logs, traces, metrics).
# The signal schemas (log_record.capnp, span.capnp) import these rather than each
# redefining them.

# OTLP AnyValue: the value of a log body, span/metric attribute, etc.
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
