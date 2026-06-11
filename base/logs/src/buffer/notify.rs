//! The readers' wakeup: a version counter bumped on every write, plus the wakers
//! of the currently-parked readers. Unlike a fixed-capacity watch, any number of
//! readers can park at once.

use alloc::vec::Vec;
use core::cell::RefCell;
use core::future::poll_fn;
use core::sync::atomic::Ordering;
use core::task::{Poll, Waker};
use dusk_program::embassy_sync::blocking_mutex::Mutex;
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use portable_atomic::AtomicU64;

pub(in crate::buffer) struct Notify {
    version: AtomicU64,
    wakers: Mutex<CriticalSectionRawMutex, RefCell<Vec<Waker>>>,
}

impl Notify {
    pub(in crate::buffer) fn new() -> Self {
        Notify {
            version: AtomicU64::new(0),
            wakers: Mutex::new(RefCell::new(Vec::new())),
        }
    }

    /// Bump the version and wake every parked reader.
    pub(in crate::buffer) fn notify(&self) {
        self.version.fetch_add(1, Ordering::Release);
        let wakers = self
            .wakers
            .lock(|wakers| core::mem::take(&mut *wakers.borrow_mut()));
        for waker in wakers {
            waker.wake();
        }
    }

    /// The version to sample *before* scanning, so a write landing mid-scan
    /// makes the matching [`changed`](Self::changed) return immediately.
    pub(in crate::buffer) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Park until the version moves past `seen`.
    pub(in crate::buffer) async fn changed(&self, seen: u64) {
        poll_fn(|context| {
            if self.version.load(Ordering::Acquire) != seen {
                return Poll::Ready(());
            }
            self.wakers.lock(|wakers| {
                let mut wakers = wakers.borrow_mut();
                if !wakers.iter().any(|waker| waker.will_wake(context.waker())) {
                    wakers.push(context.waker().clone());
                }
            });
            // Re-check: a notify between the first check and the registration
            // would have drained the list without our waker in it.
            if self.version.load(Ordering::Acquire) != seen {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}
