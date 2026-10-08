use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};

#[derive(Clone)]
pub(crate) struct RequestPacing {
    interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl RequestPacing {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) async fn wait(&self) {
        if self.interval.is_zero() {
            return;
        }
        let mut last = self.last.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + self.interval).await;
        }
        *last = Some(Instant::now());
    }
}
