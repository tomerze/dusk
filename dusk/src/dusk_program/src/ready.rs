use alloc::rc::Rc;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};

pub type Ready = Rc<Watch<CriticalSectionRawMutex, bool, 16>>;
