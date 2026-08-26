use std::cell::RefCell;
use std::rc::Rc;
use std::thread_local;
use std::vec::Vec;
use tokio::sync::Notify;

thread_local! {
    static STOP_SIGNALS: RefCell<Vec<Rc<Notify>>> = const { RefCell::new(Vec::new()) };
}

pub struct StopScope {
    _private: (),
}

impl StopScope {
    pub fn enter(stop_signal: Rc<Notify>) -> Self {
        STOP_SIGNALS.with_borrow_mut(|stop_signals| stop_signals.push(stop_signal));
        StopScope { _private: () }
    }
}

impl Drop for StopScope {
    fn drop(&mut self) {
        STOP_SIGNALS.with_borrow_mut(|stop_signals| stop_signals.pop());
    }
}

pub fn stop_innermost() -> bool {
    STOP_SIGNALS.with_borrow(|stop_signals| match stop_signals.last() {
        Some(stop_signal) => {
            stop_signal.notify_waiters();
            true
        }
        None => false,
    })
}
