use std::sync::Arc;

use crate::ExponentialHistogram;

/// An ExponentialHistogram with interior mutability
#[derive(Debug, Clone, Default)]
pub struct SharedExponentialHistogram {
    inner: Arc<ExponentialHistogram>,
}

impl SharedExponentialHistogram {
    /// Observe a value, increasing its bucket's count by 1
    #[inline]
    pub fn accumulate(&self, value: f64) {
        self.inner.accumulate(value);
    }

    /// Get the current snapshot of the histogram. This gives you an owned clone of the backing histogram
    /// at a point in time, so you can work with it without holding a lock.
    pub fn snapshot(&self) -> ExponentialHistogram {
        self.inner.as_ref().clone()
    }

    /// Get the current snapshot of the histogram and reset the histogram to zero.
    /// Note: snapshot_and_reset is not supported with the current Arc-based design.
    /// This method just returns a snapshot. To reset, create a new SharedExponentialHistogram.
    pub fn snapshot_and_reset(&self) -> ExponentialHistogram {
        self.inner.as_ref().clone()
    }
}
