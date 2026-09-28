use super::collect::{FieldCollector, FieldValue};
use super::convert::{build_log_record, build_span};
use crate::buffer::{LEVELS, SignalBuffer, level_index};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;
use dusk_program::embassy_sync::blocking_mutex::Mutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_time::Instant;
use portable_atomic_util::Arc;
use tracing::{Event, Level, Metadata, Subscriber, span};

pub(crate) const MESSAGE_FIELD: &str = "message";

const TASK_FIELD: &str = "task_id";

const NEW_TASK_FIELD: &str = "__new_task_id__";

pub(crate) const HEX_ID_FIELDS: &[&str] = &["pid", "program_id", "namespace_id", "task_id"];

/// A span's collected fields, shared cheaply across the records that reference it.
type SpanFields = Arc<Vec<(&'static str, FieldValue)>>;

/// A stack of span field-sets - a span's effective scope.
#[cfg(feature = "console")]
type Scope = Vec<SpanFields>;

struct SpanRecord {
    fields: SpanFields,
    references: usize,
    metadata: &'static Metadata<'static>,
    start_milliseconds: u64,
    root: bool,
    parent: Option<u64>,
    trace: Option<u64>,
}

struct ClosedSpan {
    name: &'static str,
    level: Level,
    start_milliseconds: u64,
    parent: Option<u64>,
    trace: Option<u64>,
    root: bool,
    fields: SpanFields,
}

/// Everything the subscriber mutates, behind one critical-section mutex.
#[derive(Default)]
struct State {
    /// Live spans, keyed by id.
    spans: BTreeMap<u64, SpanRecord>,
    /// The entered-span stack as `(span id, task root span id)`. One stack - the
    /// executor runs one task at a time - but each entry is tagged with the id of
    /// the task root span it belongs to, so a span left entered across another
    /// task's `.await` never widens that task's scope. The tag is a span id (from
    /// `last_id`), not the reused embassy task id, so it is unique for the life of
    /// the node.
    entered: Vec<(u64, u64)>,
    /// The last assigned span id (ids start at 1; 0 is never valid).
    last_id: u64,
}

pub struct BufferLayer {
    buffer: SignalBuffer,
    /// Whether `LEVELS[index]` routes to a lane.
    routed: [bool; 5],
    state: Mutex<CriticalSectionRawMutex, RefCell<State>>,
}

impl BufferLayer {
    pub fn new(buffer: SignalBuffer) -> Self {
        let routed = core::array::from_fn(|index| buffer.routes(LEVELS[index]));
        BufferLayer {
            buffer,
            routed,
            state: Mutex::new(RefCell::new(State::default())),
        }
    }

    /// The entered spans' field-sets for the current task, root-first - the
    /// console line's scope. Stored records don't carry it; a log reaches its
    /// spans' fields through its span and trace ids.
    #[cfg(feature = "console")]
    fn current_scope(&self) -> Scope {
        self.state.lock(|state| {
            let state = state.borrow();
            let current_task_root = state
                .entered
                .last()
                .map(|&(_, task_root)| task_root)
                .unwrap_or(0);
            state
                .entered
                .iter()
                .filter(|&&(_, task_root)| task_root == current_task_root)
                .filter_map(|&(span_id, _)| {
                    state
                        .spans
                        .get(&span_id)
                        .map(|record| record.fields.clone())
                })
                .collect()
        })
    }

    /// Write a closing span as a `Signal` span: its identity, kind, start/end,
    /// and its own fields as attributes. The trace id is the span's task root
    /// id (each task is a trace); the innermost ancestor is the parent. Written
    /// to its level's lane.
    fn write_span_close(&self, id: u64, span: ClosedSpan) {
        let end_milliseconds = Instant::now().as_millis();
        let mut message = build_span(
            id,
            span.trace,
            span.parent,
            span.name,
            span.root,
            span.start_milliseconds,
            Some(end_milliseconds),
            &span.fields,
        );
        // Can't log from inside the subscriber (it would recurse); write()
        // counted the failure in drop_counts().write_failures.
        let _ = self.buffer.writer().write(span.level, &mut message);
    }

    #[cfg(feature = "c_api")]
    pub(crate) fn write_log(
        &self,
        level: Level,
        fields: &[(&str, FieldValue)],
    ) -> capnp::Result<()> {
        if !self.routed[level_index(level)] {
            return Ok(());
        }
        let mut message = build_log_record(fields, None, None);
        self.buffer.writer().write(level, &mut message)
    }
}

impl Subscriber for BufferLayer {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        // Track every span and see every event; the buffer's lane routing - not
        // a global filter - decides what is kept (see `event`).
        true
    }

    fn new_span(&self, attributes: &span::Attributes<'_>) -> span::Id {
        let mut collector = FieldCollector::default();
        attributes.record(&mut collector);
        // A task root declares itself with `__new_task_id__`; detect it by the
        // field's presence (its value, the reused embassy id, can't identify a
        // task), then translate the field to the public `task_id` so the embassy
        // id still surfaces as an ordinary attribute.
        let root = collector
            .fields
            .iter()
            .any(|(name, _)| *name == NEW_TASK_FIELD);
        for (name, _) in collector.fields.iter_mut() {
            if *name == NEW_TASK_FIELD {
                *name = TASK_FIELD;
            }
        }
        let metadata = attributes.metadata();
        // The span rides its own level's lane (`info_span!`/`debug_span!`/…); only
        // record it when that lane is kept.
        let routed = self.routed[level_index(*metadata.level())];
        // Stamp the start outside the state lock - `Instant::now()` takes the time
        // driver's mutex, which must not nest inside the critical section.
        let start_milliseconds = if routed {
            Instant::now().as_millis()
        } else {
            0
        };
        let own = Arc::new(collector.fields);

        let (id, parent, trace) = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.last_id += 1;
            let id = state.last_id;

            // Derive the span's identity in its trace: the parent is the task's
            // innermost currently-entered span, the trace is the task root
            // (root-first first entered) - the span's own id if it is the root.
            // Both are written at open and again at close. Only derived when the
            // span's lane keeps the signal.
            let current_task_root = state
                .entered
                .last()
                .map(|&(_, task_root)| task_root)
                .unwrap_or(0);
            let (parent, trace) = if !routed {
                (None, None)
            } else if root {
                (None, Some(id))
            } else {
                let mut parent = None;
                let mut trace = None;
                for &(span_id, task_root) in state.entered.iter() {
                    if task_root != current_task_root {
                        continue;
                    }
                    if trace.is_none() {
                        trace = Some(span_id);
                    }
                    parent = Some(span_id);
                }
                (parent, trace)
            };

            state.spans.insert(
                id,
                SpanRecord {
                    fields: own.clone(),
                    references: 1,
                    metadata,
                    start_milliseconds,
                    root,
                    parent,
                    trace,
                },
            );
            (id, parent, trace)
        });

        if routed {
            let mut message = build_span(
                id,
                trace,
                parent,
                metadata.name(),
                root,
                start_milliseconds,
                None,
                &own,
            );
            let _ = self.buffer.writer().write(*metadata.level(), &mut message);
        }
        span::Id::from_u64(id)
    }

    fn record(&self, id: &span::Id, values: &span::Record<'_>) {
        // Collect outside the lock - visiting runs caller formatting code.
        let existing = self.state.lock(|state| {
            state
                .borrow()
                .spans
                .get(&id.into_u64())
                .map(|record| (*record.fields).clone())
        });
        let Some(fields) = existing else { return };
        let mut collector = FieldCollector { fields };
        values.record(&mut collector);
        self.state.lock(|state| {
            if let Some(record) = state.borrow_mut().spans.get_mut(&id.into_u64()) {
                record.fields = Arc::new(collector.fields);
            }
        });
    }

    fn record_follows_from(&self, _id: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        if !self.routed[level_index(*metadata.level())] {
            return;
        }

        // Collect the event's own fields once - visiting runs caller formatting
        // code, so it must not run twice.
        let mut event_fields = FieldCollector::default();
        event.record(&mut event_fields);

        // A log's span is the innermost it's within; its trace is its task - the
        // id of that task's root span.
        let current = self
            .state
            .lock(|state| state.borrow().entered.last().copied());
        let span_id = current.map(|(span_id, _)| span_id);
        let trace_id = current.map(|(_, task_root)| task_root);

        let mut message = build_log_record(&event_fields.fields, span_id, trace_id);
        // Can't log from inside event (it would recurse into this subscriber);
        // write() counted the failure in drop_counts().write_failures.
        let _ = self.buffer.writer().write(*metadata.level(), &mut message);

        #[cfg(feature = "console")]
        super::console::print(metadata, &event_fields.fields, &self.current_scope());
    }

    fn enter(&self, id: &span::Id) {
        let span_id = id.into_u64();
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            // A task's root span heads its own trace, keyed by its own (unique)
            // span id; a span entered within one inherits that key; before any
            // task root it is 0.
            let roots_task = state
                .spans
                .get(&span_id)
                .map(|record| record.root)
                .unwrap_or(false);
            let task_root = if roots_task {
                span_id
            } else {
                state
                    .entered
                    .last()
                    .map(|&(_, task_root)| task_root)
                    .unwrap_or(0)
            };
            state.entered.push((span_id, task_root));
        });
    }

    fn exit(&self, id: &span::Id) {
        let span_id = id.into_u64();
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            // Normally the top, but pop the matching id to stay correct if an
            // exit arrives out of order.
            if let Some(position) = state
                .entered
                .iter()
                .rposition(|&(entered, _)| entered == span_id)
            {
                state.entered.remove(position);
            }
        });
    }

    fn clone_span(&self, id: &span::Id) -> span::Id {
        self.state.lock(|state| {
            if let Some(record) = state.borrow_mut().spans.get_mut(&id.into_u64()) {
                record.references += 1;
            }
        });
        id.clone()
    }

    fn try_close(&self, id: span::Id) -> bool {
        let closed = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let record = state.spans.get_mut(&id.into_u64())?;
            record.references -= 1;
            if record.references != 0 {
                return None;
            }
            // The span rides its own level's lane; only record it when kept.
            let level = *record.metadata.level();
            let span = if self.routed[level_index(level)] {
                Some(ClosedSpan {
                    name: record.metadata.name(),
                    level,
                    start_milliseconds: record.start_milliseconds,
                    parent: record.parent,
                    trace: record.trace,
                    root: record.root,
                    fields: record.fields.clone(),
                })
            } else {
                None
            };
            state.spans.remove(&id.into_u64());
            Some(span)
        });
        match closed {
            None => false,
            Some(span) => {
                if let Some(span) = span {
                    self.write_span_close(id.into_u64(), span);
                }
                true
            }
        }
    }

    fn current_span(&self) -> tracing_core::span::Current {
        use tracing_core::span::Current;
        self.state.lock(|state| {
            let state = state.borrow();
            match state.entered.last() {
                Some(&(span_id, _)) => match state.spans.get(&span_id) {
                    Some(record) => Current::new(span::Id::from_u64(span_id), record.metadata),
                    None => Current::none(),
                },
                None => Current::none(),
            }
        })
    }
}
