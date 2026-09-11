use std::cell::RefCell;
use std::rc::Rc;
use std::thread_local;
use std::vec::Vec;
use tokio::sync::Notify;

thread_local! {
    static STOP_SIGNALS: RefCell<Vec<Rc<Notify>>> = const { RefCell::new(Vec::new()) };
}

pub struct StopSignal(Rc<Notify>);

impl StopSignal {
    pub fn new() -> Self {
        let signal = Rc::new(Notify::new());
        STOP_SIGNALS.with_borrow_mut(|signals| signals.push(signal.clone()));
        Self(signal)
    }

    pub fn signal(&self) -> Rc<Notify> {
        self.0.clone()
    }
}

impl Default for StopSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for StopSignal {
    fn drop(&mut self) {
        STOP_SIGNALS.with_borrow_mut(|signals| {
            if let Some(position) = signals
                .iter()
                .rposition(|signal| Rc::ptr_eq(signal, &self.0))
            {
                signals.remove(position);
            }
        });
    }
}

pub fn stop_innermost() {
    STOP_SIGNALS.with_borrow(|signals| {
        if let Some(signal) = signals.last() {
            signal.notify_waiters();
        }
    });
}
