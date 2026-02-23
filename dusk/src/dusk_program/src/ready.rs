use alloc::rc::Rc;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

pub type Ready = Rc<Signal<CriticalSectionRawMutex, ()>>;
