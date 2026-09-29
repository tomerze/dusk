use super::layer::BufferLayer;
use alloc::collections::BTreeMap;
use core::cell::RefCell;
use dusk_program::embassy_sync::blocking_mutex::Mutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use portable_atomic_util::Arc;
use tracing::subscriber::Interest;
use tracing::{Dispatch, Event, Metadata, Subscriber, span};

struct Registry {
    installed: bool,
    layers: BTreeMap<u64, Arc<BufferLayer>>,
}

static REGISTRY: Mutex<CriticalSectionRawMutex, RefCell<Registry>> =
    Mutex::new(RefCell::new(Registry {
        installed: false,
        layers: BTreeMap::new(),
    }));

const UNROUTED_SPAN: u64 = u64::MAX;

struct BufferRouter;

pub(crate) fn register(tid: u64, layer: Arc<BufferLayer>) -> anyhow::Result<()> {
    REGISTRY.lock(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.layers.contains_key(&tid) {
            anyhow::bail!("a logs launcher is already registered on thread {tid}");
        }
        if !registry.installed {
            tracing::dispatcher::set_global_default(Dispatch::new(BufferRouter)).map_err(
                |error| anyhow::anyhow!("couldn't install the logs subscriber: {error}"),
            )?;
            registry.installed = true;
        }
        registry.layers.insert(tid, layer);
        Ok(())
    })
}

pub(crate) fn unregister(tid: u64, layer: &Arc<BufferLayer>) {
    REGISTRY.lock(|registry| {
        let mut registry = registry.borrow_mut();
        if registry
            .layers
            .get(&tid)
            .is_some_and(|registered| Arc::ptr_eq(registered, layer))
        {
            registry.layers.remove(&tid);
        }
    });
}

fn current() -> Option<Arc<BufferLayer>> {
    let tid = dusk_core::driver::tid();
    REGISTRY.lock(|registry| registry.borrow().layers.get(&tid).cloned())
}

impl Subscriber for BufferRouter {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        current().is_some_and(|layer| layer.enabled(metadata))
    }

    fn new_span(&self, attributes: &span::Attributes<'_>) -> span::Id {
        match current() {
            Some(layer) => layer.new_span(attributes),
            None => span::Id::from_u64(UNROUTED_SPAN),
        }
    }

    fn record(&self, id: &span::Id, values: &span::Record<'_>) {
        if let Some(layer) = current() {
            layer.record(id, values);
        }
    }

    fn record_follows_from(&self, id: &span::Id, follows: &span::Id) {
        if let Some(layer) = current() {
            layer.record_follows_from(id, follows);
        }
    }

    fn event(&self, event: &Event<'_>) {
        if let Some(layer) = current() {
            layer.event(event);
        }
    }

    fn enter(&self, id: &span::Id) {
        if let Some(layer) = current() {
            layer.enter(id);
        }
    }

    fn exit(&self, id: &span::Id) {
        if let Some(layer) = current() {
            layer.exit(id);
        }
    }

    fn clone_span(&self, id: &span::Id) -> span::Id {
        match current() {
            Some(layer) => layer.clone_span(id),
            None => id.clone(),
        }
    }

    fn try_close(&self, id: span::Id) -> bool {
        match current() {
            Some(layer) => layer.try_close(id),
            None => false,
        }
    }

    fn current_span(&self) -> tracing_core::span::Current {
        match current() {
            Some(layer) => layer.current_span(),
            None => tracing_core::span::Current::none(),
        }
    }
}
