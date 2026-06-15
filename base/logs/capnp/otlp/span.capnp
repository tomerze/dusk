@0xa3f59c1e7b8d4602;

# OpenTelemetry / OTLP span (wire form). Mirrors the Span message from
# opentelemetry/proto/trace/v1 (and the Status it carries). The AnyValue /
# KeyValue family (opentelemetry/proto/common/v1) is shared with the other
# signals, so it is imported from common.capnp. The OTLP transport envelope
# (TracesData / ResourceSpans / ScopeSpans / Resource / InstrumentationScope) is
# intentionally not modelled here.
using Common = import "/capnp/common.capnp";

# A single span: one operation within a trace.
struct Span {
  traceId @0 :Data;
  spanId @1 :Data;
  traceState @2 :Text;
  parentSpanId @3 :Data;
  # W3C trace flags plus the remote-context bits.
  flags @4 :UInt32;
  name @5 :Text;
  kind @6 :SpanKind;
  startTimeUnixNano @7 :UInt64;
  endTimeUnixNano @8 :UInt64;
  attributes @9 :List(Common.KeyValue);
  droppedAttributesCount @10 :UInt32;
  events @11 :List(Event);
  droppedEventsCount @12 :UInt32;
  links @13 :List(Link);
  droppedLinksCount @14 :UInt32;
  status @15 :Status;

  # How the span relates to its parent/children.
  enum SpanKind {
    unspecified @0;
    internal @1;
    server @2;
    client @3;
    producer @4;
    consumer @5;
  }

  # A timestamped event that occurred during the span.
  struct Event {
    timeUnixNano @0 :UInt64;
    name @1 :Text;
    attributes @2 :List(Common.KeyValue);
    droppedAttributesCount @3 :UInt32;
  }

  # A pointer from this span to another span (possibly in another trace).
  struct Link {
    traceId @0 :Data;
    spanId @1 :Data;
    traceState @2 :Text;
    attributes @3 :List(Common.KeyValue);
    droppedAttributesCount @4 :UInt32;
    flags @5 :UInt32;
  }
}

# The span's completion status.
struct Status {
  message @0 :Text;
  code @1 :StatusCode;

  enum StatusCode {
    unset @0;
    ok @1;
    error @2;
  }
}
