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
