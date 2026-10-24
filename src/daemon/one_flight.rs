use std::future::Future;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

type Flight<T> = Shared<BoxFuture<'static, Arc<T>>>;

pub struct OneFlight<T> {
    running: Mutex<Option<Flight<T>>>,
}

impl<T> Default for OneFlight<T> {
    fn default() -> Self {
        OneFlight {
            running: Mutex::new(None),
        }
    }
}

impl<T: Send + Sync + 'static> OneFlight<T> {
    pub async fn run<F>(&self, start: impl FnOnce() -> F) -> Arc<T>
    where
        F: Future<Output = T> + Send + 'static,
    {
        let flight = {
            let mut slot = self.running.lock().unwrap();
            match slot.as_ref().filter(|f| f.peek().is_none()) {
                Some(flight) => flight.clone(),
                None => {
                    let flight = start().map(Arc::new).boxed().shared();
                    *slot = Some(flight.clone());
                    flight
                }
            }
        };
        let value = flight.await;
        let mut slot = self.running.lock().unwrap();
        if slot.as_ref().is_some_and(|f| f.peek().is_some()) {
            *slot = None;
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;

    #[tokio::test]
    async fn concurrent_runs_share_one_computation() {
        let flight = Arc::new(OneFlight::<usize>::default());
        let starts = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let join = |flight: Arc<OneFlight<usize>>| {
            let (starts, gate) = (starts.clone(), gate.clone());
            tokio::spawn(async move {
                flight
                    .run(|| async move {
                        let n = starts.fetch_add(1, Ordering::SeqCst) + 1;
                        gate.notified().await;
                        n
                    })
                    .await
            })
        };
        let tasks: Vec<_> = (0..5).map(|_| join(flight.clone())).collect();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        gate.notify_waiters();
        for task in tasks {
            assert_eq!(*task.await.unwrap(), 1);
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        let next = flight.run(|| async { 2 }).await;
        assert_eq!(*next, 2);
        assert!(flight.running.lock().unwrap().is_none());
    }
}
