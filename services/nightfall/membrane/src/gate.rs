use futures::channel::oneshot;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

pub(crate) struct Gate {
    capacity: usize,
    used: Cell<usize>,
    waiters: RefCell<VecDeque<oneshot::Sender<GateTicket>>>,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new(1)
    }
}

pub(crate) struct GateTicket {
    gate: Option<Rc<Gate>>,
}

impl Gate {
    pub(crate) fn new(capacity: usize) -> Gate {
        Gate {
            capacity,
            used: Cell::new(0),
            waiters: RefCell::new(VecDeque::new()),
        }
    }

    pub(crate) fn used(&self) -> usize {
        self.used.get()
    }

    pub(crate) fn try_enter(self: &Rc<Gate>) -> Option<GateTicket> {
        if self.used.get() >= self.capacity {
            return None;
        }
        self.used.set(self.used.get() + 1);
        Some(GateTicket {
            gate: Some(self.clone()),
        })
    }

    pub(crate) fn enter(self: &Rc<Gate>) -> impl Future<Output = GateTicket> + 'static {
        let place = match self.try_enter() {
            Some(ticket) => Ok(ticket),
            None => {
                let (sender, receiver) = oneshot::channel();
                self.waiters.borrow_mut().push_back(sender);
                Err(receiver)
            }
        };
        async move {
            match place {
                Ok(ticket) => ticket,
                Err(receiver) => match receiver.await {
                    Ok(ticket) => ticket,
                    Err(oneshot::Canceled) => GateTicket { gate: None },
                },
            }
        }
    }
}

impl Drop for GateTicket {
    fn drop(&mut self) {
        let Some(gate) = self.gate.take() else {
            return;
        };
        loop {
            let next = gate.waiters.borrow_mut().pop_front();
            let Some(sender) = next else {
                gate.used.set(gate.used.get().saturating_sub(1));
                return;
            };
            match sender.send(GateTicket {
                gate: Some(gate.clone()),
            }) {
                Ok(()) => return,
                Err(mut returned) => returned.gate = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[test]
    fn admits_in_arrival_order_and_skips_abandoned_waiters() {
        let gate = Rc::new(Gate::default());
        let first = gate.try_enter().unwrap();
        assert!(gate.try_enter().is_none());
        let mut second = Box::pin(gate.clone().enter());
        let abandoned = Box::pin(gate.clone().enter());
        let mut third = Box::pin(gate.clone().enter());
        assert!(second.as_mut().now_or_never().is_none());
        assert!(third.as_mut().now_or_never().is_none());
        let mut abandoned = abandoned;
        assert!(abandoned.as_mut().now_or_never().is_none());
        drop(abandoned);
        drop(first);
        assert!(third.as_mut().now_or_never().is_none());
        let second = second.now_or_never().unwrap();
        assert!(gate.try_enter().is_none());
        drop(second);
        let third = third.now_or_never().unwrap();
        drop(third);
        assert!(gate.try_enter().is_some());
    }

    #[test]
    fn admits_up_to_its_capacity_and_hands_each_released_place_to_the_oldest_waiter() {
        let gate = Rc::new(Gate::new(2));
        let first = gate.try_enter().unwrap();
        let second = gate.try_enter().unwrap();
        assert!(gate.try_enter().is_none());
        let mut third = Box::pin(gate.clone().enter());
        let mut fourth = Box::pin(gate.clone().enter());
        assert!(third.as_mut().now_or_never().is_none());
        assert!(fourth.as_mut().now_or_never().is_none());
        drop(second);
        assert_eq!(gate.used(), 2);
        assert!(fourth.as_mut().now_or_never().is_none());
        let third = third.now_or_never().unwrap();
        drop(first);
        let fourth = fourth.now_or_never().unwrap();
        drop(third);
        drop(fourth);
        assert_eq!(gate.used(), 0);
        let closed = Rc::new(Gate::new(0));
        assert!(closed.try_enter().is_none());
    }

    #[test]
    fn passes_a_ticket_dropped_in_the_channel_to_the_next_waiter() {
        let gate = Rc::new(Gate::default());
        let first = gate.try_enter().unwrap();
        let mut second = Box::pin(gate.clone().enter());
        let mut third = Box::pin(gate.clone().enter());
        assert!(second.as_mut().now_or_never().is_none());
        assert!(third.as_mut().now_or_never().is_none());
        drop(first);
        drop(second);
        let third = third.now_or_never().unwrap();
        assert!(gate.try_enter().is_none());
        drop(third);
        assert!(gate.try_enter().is_some());
    }
}
