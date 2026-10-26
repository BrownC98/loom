use std::sync::{Arc, Mutex};

use crate::graph::NodeId;

#[derive(Clone)]
pub struct Caller {
    pub node: NodeId,
    arrival: Arrival,
}

#[derive(Clone)]
pub struct Arrival(Arc<Mutex<Option<Ticket>>>);

type Ticket = Arc<AllArrived>;

struct AllArrived(Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for AllArrived {
    fn drop(&mut self) {
        if let Some(then) = self.0.take() {
            then();
        }
    }
}

impl std::fmt::Debug for Caller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.node.fmt(f)
    }
}

impl Caller {
    pub fn arrival(&self) -> Arrival {
        self.arrival.clone()
    }
}

impl Arrival {
    pub fn arrived(&self) {
        self.0.lock().unwrap().take();
    }
}

pub fn callers(
    node: NodeId,
    held: u32,
    then: impl FnOnce() + Send + Sync + 'static,
) -> impl FnMut() -> Caller + Send + 'static {
    let mut left = held;
    let mut last = Some(Arc::new(AllArrived(Some(Box::new(then)))));
    if held == 0 {
        last = None;
    }
    move || {
        let ticket = match left {
            0 => None,
            1 => {
                left = 0;
                last.take()
            }
            _ => {
                left -= 1;
                last.clone()
            }
        };
        Caller {
            node,
            arrival: Arrival(Arc::new(Mutex::new(ticket))),
        }
    }
}
