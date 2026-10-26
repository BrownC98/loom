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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn counted(held: u32) -> (Arc<AtomicBool>, impl FnMut() -> Caller) {
        let done = Arc::new(AtomicBool::new(false));
        let set = done.clone();
        let callers = callers(NodeId(1), held, move || set.store(true, Ordering::SeqCst));
        (done, callers)
    }

    #[test]
    fn the_check_runs_when_each_held_stream_arrived_or_ended() {
        let (done, mut next) = counted(0);
        assert!(done.load(Ordering::SeqCst), "nothing held: at once");
        assert!(next().arrival.0.lock().unwrap().is_none());

        let (done, mut next) = counted(2);
        let (first, second, later) = (next(), next(), next());
        assert!(later.arrival.0.lock().unwrap().is_none(), "a new stream");
        first.arrival().arrived();
        first.arrival().arrived();
        assert!(!done.load(Ordering::SeqCst));
        drop(second);
        assert!(done.load(Ordering::SeqCst), "the last held stream ended");

        let (done, mut next) = counted(2);
        let only = next();
        drop(next);
        only.arrival().arrived();
        assert!(
            done.load(Ordering::SeqCst),
            "the link ended before all streams came"
        );
    }
}
