use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::RefCell;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use futures_core::Stream;

struct Channel<T> {
    queue: VecDeque<T>,
    waker: Option<Waker>,
    senders: usize,
    receiver_alive: bool,
}

pub struct UnboundedSender<T> {
    shared: Rc<RefCell<Channel<T>>>,
}

pub struct UnboundedReceiver<T> {
    shared: Rc<RefCell<Channel<T>>>,
}

pub struct SendError<T>(pub T);

pub fn unbounded<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let shared = Rc::new(RefCell::new(Channel {
        queue: VecDeque::new(),
        waker: None,
        senders: 1,
        receiver_alive: true,
    }));
    (
        UnboundedSender {
            shared: shared.clone(),
        },
        UnboundedReceiver { shared },
    )
}

impl<T> UnboundedSender<T> {
    pub fn unbounded_send(&self, item: T) -> Result<(), SendError<T>> {
        let waker = {
            let mut shared = self.shared.borrow_mut();
            if !shared.receiver_alive {
                return Err(SendError(item));
            }
            shared.queue.push_back(item);
            shared.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        !self.shared.borrow().receiver_alive
    }
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        self.shared.borrow_mut().senders += 1;
        UnboundedSender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for UnboundedSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut shared = self.shared.borrow_mut();
            shared.senders -= 1;
            if shared.senders == 0 {
                shared.waker.take()
            } else {
                None
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        let abandoned = {
            let mut shared = self.shared.borrow_mut();
            shared.receiver_alive = false;
            mem::take(&mut shared.queue)
        };
        drop(abandoned);
    }
}

impl<T> Unpin for UnboundedReceiver<T> {}

impl<T> Stream for UnboundedReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let mut shared = self.shared.borrow_mut();
        if let Some(item) = shared.queue.pop_front() {
            return Poll::Ready(Some(item));
        }
        if shared.senders == 0 {
            return Poll::Ready(None);
        }
        shared.waker = Some(context.waker().clone());
        Poll::Pending
    }
}
