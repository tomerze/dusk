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

/// A stack of span field-sets — a span's effective scope.
type Scope = Vec<SpanFields>;

struct SpanRecord {
    fields: SpanFields,
    references: usize,
    metadata: &'static Metadata<'static>,
    start_milliseconds: u64,
    root: bool,
    ancestors: Vec<u64>,
}

struct ClosedSpan {
    name: &'static str,
    level: Level,
    start_milliseconds: u64,
    parent: Option<u64>,
    trace: Option<u64>,
    root: bool,
    scope: Scope,
}

/// Everything the subscriber mutates, behind one critical-section mutex.
#[derive(Default)]
struct State {
    /// Live spans, keyed by id.
    spans: BTreeMap<u64, SpanRecord>,
    /// The entered-span stack as `(span id, task root span id)`. One stack — the
    /// executor runs one task at a time — but each entry is tagged with the id of
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

    fn current_scope(&self) -> (Scope, Option<(u64, u64)>) {
        self.state.lock(|state| {
            let state = state.borrow();
            let current = state.entered.last().copied();
            let current_task_root = current.map(|(_, task_root)| task_root).unwrap_or(0);
            let scope = state
                .entered
                .iter()
                .filter(|&&(_, task_root)| task_root == current_task_root)
                .filter_map(|&(span_id, _)| {
                    state
                        .spans
                        .get(&span_id)
                        .map(|record| record.fields.clone())
                })
                .collect();
            (scope, current)
        })
    }

    /// Write a closing span as a `Signal` span: its identity, kind, start/end, and
    /// attributes (the span's deduplicated effective scope, innermost-first). The
    /// trace id is the span's task root id (each task is a trace); the innermost
    /// ancestor is the parent. Written to its level's lane.
    fn write_span_close(&self, id: u64, span: ClosedSpan) {
        let end_milliseconds = Instant::now().as_millis();

        // Deduplicate the effective scope (innermost-first, first value wins) into
        // the span's attributes.
        let mut attributes: Vec<(&'static str, &FieldValue)> = Vec::new();
        for fields in span.scope.iter().rev() {
            for (name, value) in fields.iter() {
                if !attributes.iter().any(|(seen, _)| *seen == *name) {
                    attributes.push((name, value));
                }
            }
        }

        let mut message = build_span(
            id,
            span.trace,
            span.parent,
            span.name,
            span.root,
            span.start_milliseconds,
            end_milliseconds,
            &attributes,
        );
        let result = self.buffer.writer().write(span.level, &mut message);
        debug_assert!(result.is_ok(), "writing a span signal to the buffer failed");
    }
}

impl Subscriber for BufferLayer {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        // Track every span and see every event; the buffer's lane routing — not
        // a global filter — decides what is kept (see `event`).
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
        // Stamp the start outside the state lock — `Instant::now()` takes the time
        // driver's mutex, which must not nest inside the critical section.
        let start_milliseconds = if routed {
            Instant::now().as_millis()
        } else {
            0
        };
        let own = Arc::new(collector.fields);

        let id = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.last_id += 1;
            let id = state.last_id;

            // Stash what the span close needs: the ids of the task's
            // currently-entered spans — its ancestors, root-first, the innermost
            // being the parent. The close reads their *live* fields. A task's root
            // span heads its own trace, so it has no ancestors. Only gathered when
            // the trace lane keeps the signal.
            let current_task_root = state
                .entered
                .last()
                .map(|&(_, task_root)| task_root)
                .unwrap_or(0);
            let ancestors = if !routed || root {
                Vec::new()
            } else {
                state
                    .entered
                    .iter()
                    .filter(|&&(_, task_root)| task_root == current_task_root)
                    .map(|&(span_id, _)| span_id)
                    .collect()
            };

            state.spans.insert(
                id,
                SpanRecord {
                    fields: own,
                    references: 1,
                    metadata,
                    start_milliseconds,
                    root,
                    ancestors,
                },
            );
            id
        });
        span::Id::from_u64(id)
    }

    fn record(&self, id: &span::Id, values: &span::Record<'_>) {
        // Collect outside the lock — visiting runs caller formatting code.
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

        // Collect the event's own fields once — visiting runs caller formatting
        // code, so it must not run twice.
        let mut event_fields = FieldCollector::default();
        event.record(&mut event_fields);

        let (scope, current) = self.current_scope();
        // A log's span is the innermost it's within; its trace is its task — the
        // id of that task's root span.
        let span_id = current.map(|(span_id, _)| span_id);
        let trace_id = current.map(|(_, task_root)| task_root);

        let mut message = build_log_record(&event_fields.fields, &scope, span_id, trace_id);
        let result = self.buffer.writer().write(*metadata.level(), &mut message);
        // Can't log from inside event (it would recurse into this subscriber);
        // write() counted the failure in drop_counts().write_failures.
        debug_assert!(result.is_ok(), "writing a log signal to the buffer failed");

        #[cfg(feature = "console")]
        super::console::print(metadata, &event_fields.fields, &scope);
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
            // Pull the closing span's own data out first — ending its `&mut`
            // borrow — then read its ancestors' *current* fields from the same
            // table, so a late `record()` on the span or an ancestor is reflected.
            let span = if self.routed[level_index(level)] {
                let name = record.metadata.name();
                let start_milliseconds = record.start_milliseconds;
                let root = record.root;
                let own = record.fields.clone();
                let ancestors = core::mem::take(&mut record.ancestors);
                let mut scope: Scope = ancestors
                    .iter()
                    .filter_map(|ancestor| state.spans.get(ancestor).map(|r| r.fields.clone()))
                    .collect();
                let parent = ancestors.last().copied();
                // The trace is the task root: this span's own id if it is the root,
                // else the root-first first ancestor (the task root span).
                let trace = if root {
                    Some(id.into_u64())
                } else {
                    ancestors.first().copied()
                };
                scope.push(own);
                Some(ClosedSpan {
                    name,
                    level,
                    start_milliseconds,
                    parent,
                    trace,
                    root,
                    scope,
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
