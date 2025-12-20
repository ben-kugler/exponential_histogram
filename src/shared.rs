use std::sync::{Arc, Mutex};

use crate::exponential_histogram::ExponentialHistogram;

pub struct SharedExponentialHistogram {
    inner: Arc<Mutex<ExponentialHistogram>>,
}

impl Default for SharedExponentialHistogram {
    fn default() -> Self {
        Self::new(0)
    }
}

impl SharedExponentialHistogram {
    pub fn new(desired_scale: i32) -> Self {
        Self::new_with_max_buckets(desired_scale, 160)
    }

    pub fn new_with_max_buckets(desired_scale: i32, _ignored: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ExponentialHistogram::new(desired_scale))),
        }
    }

    pub fn accumulate(&self, value: f64) {
        if let Ok(mut hist) = self.inner.lock() {
            hist.accumulate(value);
        }
    }

    pub fn snapshot(&self) -> ExponentialHistogram {
        if let Ok(hist) = self.inner.lock() {
            hist.clone()
        } else {
            ExponentialHistogram::default()
        }
    }

    pub fn snapshot_and_reset(&self) -> ExponentialHistogram {
        if let Ok(mut hist) = self.inner.lock() {
            let snapshot = hist.clone();
            hist.reset();
            snapshot
        } else {
            ExponentialHistogram::default()
        }
    }
}
