use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::sync::Arc;
use alloc::task::Wake;
use alloc::vec::Vec;
use core::cell::{RefCell, UnsafeCell};
use core::future::Future;
use core::mem;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

struct Notifier {
    locked: AtomicBool,
    woken: AtomicBool,
    wakers: UnsafeCell<Vec<Waker>>,
}

unsafe impl Sync for Notifier {}

impl Notifier {
    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::SeqCst);
    }

    fn register(&self, waker: &Waker) {
        while !self.try_lock() {
            core::hint::spin_loop();
        }
        let wakers = unsafe { &mut *self.wakers.get() };
        if !wakers.iter().any(|registered| registered.will_wake(waker)) {
            wakers.push(waker.clone());
        }
        self.unlock();
        if self.woken.load(Ordering::SeqCst) {
            self.wake_all();
        }
    }

    fn wake_all(&self) {
        self.woken.store(true, Ordering::SeqCst);
        loop {
            if !self.try_lock() {
                return;
            }
            if !self.woken.swap(false, Ordering::SeqCst) {
                self.unlock();
                if self.woken.load(Ordering::SeqCst) {
                    continue;
                }
                return;
            }
            let wakers = mem::take(unsafe { &mut *self.wakers.get() });
            self.unlock();
            for waker in wakers {
                waker.wake();
            }
        }
    }
}

impl Wake for Notifier {
    fn wake(self: Arc<Self>) {
        self.wake_all();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_all();
    }
}

enum State<F: Future> {
    Pending(Pin<Box<F>>),
    Polling,
    Done(F::Output),
}

struct Inner<F: Future> {
    state: RefCell<State<F>>,
    notifier: Arc<Notifier>,
}

pub struct Shared<F: Future> {
    inner: Rc<Inner<F>>,
}

impl<F: Future> Shared<F> {
    pub fn new(future: F) -> Self {
        Shared {
            inner: Rc::new(Inner {
                state: RefCell::new(State::Pending(Box::pin(future))),
                notifier: Arc::new(Notifier {
                    locked: AtomicBool::new(false),
                    woken: AtomicBool::new(false),
                    wakers: UnsafeCell::new(Vec::new()),
                }),
            }),
        }
    }
}

impl<F: Future> Clone for Shared<F> {
    fn clone(&self) -> Self {
        Shared {
            inner: self.inner.clone(),
        }
    }
}

impl<F: Future> Unpin for Shared<F> {}

impl<F: Future> Future for Shared<F>
where
    F::Output: Clone,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let inner = &self.inner;
        let mut future = {
            let mut state = inner.state.borrow_mut();
            match &*state {
                State::Done(output) => return Poll::Ready(output.clone()),
                State::Polling => {
                    drop(state);
                    inner.notifier.register(context.waker());
                    return Poll::Pending;
                }
                State::Pending(_) => {}
            }
            match mem::replace(&mut *state, State::Polling) {
                State::Pending(future) => future,
                State::Polling | State::Done(_) => unreachable!(),
            }
        };
        inner.notifier.register(context.waker());
        let waker = Waker::from(inner.notifier.clone());
        let mut shared_context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut shared_context) {
            Poll::Ready(output) => {
                *inner.state.borrow_mut() = State::Done(output.clone());
                inner.notifier.wake_all();
                Poll::Ready(output)
            }
            Poll::Pending => {
                *inner.state.borrow_mut() = State::Pending(future);
                Poll::Pending
            }
        }
    }
}

pub trait SharedExt: Future + Sized {
    fn into_shared(self) -> Shared<Self> {
        Shared::new(self)
    }
}

impl<F: Future> SharedExt for F {}
