use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
    },
};

use itertools::Itertools;

/// Pre-allocated capacity to avoid allocations in hot path
const INITIAL_CAPACITY: usize = 256;

// In place of histogram::Histogram
#[derive(Debug, Default)]
pub(crate) struct InnerHistogram {
    bucket_counts: Arc<Box<[AtomicU64]>>,
    // The index of the first entry of OTel data in bucket_counts
    offset: AtomicI32,
    // Our data lies between min_boundry and max_boundry
    min_boundary: AtomicI32,
    max_boundary: AtomicI32,
    // Have we seen any data?
    initialized: AtomicBool,
}

impl Clone for InnerHistogram {
    fn clone(&self) -> Self {
        Self {
            bucket_counts: self.bucket_counts.clone(),
            offset: self.offset.load(Ordering::Acquire).into(),
            min_boundary: self.min_boundary.load(Ordering::Acquire).into(),
            max_boundary: self.max_boundary.load(Ordering::Acquire).into(),
            initialized: self.initialized.load(Ordering::Acquire).into(),
        }
    }
}

impl InnerHistogram {
    pub(crate) fn new() -> Self {
        let mut buckets = Vec::with_capacity(INITIAL_CAPACITY);
        buckets.resize_with(INITIAL_CAPACITY, || AtomicU64::new(0));

        Self {
            bucket_counts: Arc::new(buckets.into()),
            offset: 0.into(),
            min_boundary: 0.into(),
            max_boundary: 0.into(),
            initialized: false.into(),
        }
    }

    #[inline(always)]
    pub(crate) fn increment(&self, otel_index: i32) {
        if !self.initialized.load(Ordering::Acquire) {
            if self
                .initialized
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.offset.store(otel_index, Ordering::Release);
                self.min_boundary.store(otel_index, Ordering::Release);
                self.max_boundary.store(otel_index, Ordering::Release);
                self.bucket_counts[0].store(1, Ordering::Release);
                return;
            }
        }

        let offset = self.offset.load(Ordering::Acquire);
        let index = otel_index - offset;

        // Handle negative index by atomically updating offset
        // This is the lock-free equivalent of shifting the array
        if index < 0 {
            let shift = -index;

            if shift < INITIAL_CAPACITY as i32 {
                let new_offset = otel_index;
                match self.offset.compare_exchange(
                    offset,
                    new_offset,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // Increment the offset
                        self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);

                        // Update min bondary
                        let mut old_min = self.min_boundary.load(Ordering::Relaxed);
                        // Keep trying until we succeed
                        while otel_index < old_min {
                            match self.min_boundary.compare_exchange_weak(
                                old_min,
                                otel_index,
                                Ordering::Release,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => break,
                                Err(x) => old_min = x,
                            }
                        }
                        return;
                    }
                    Err(current_offset) => {
                        // Someone else updated the offset, retry with new offset
                        let index = otel_index - current_offset;

                        // If still negative after retry, just cap at bucket[0]
                        if index < 0 {
                            self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);

                            let mut old_min = self.min_boundary.load(Ordering::Relaxed);
                            while otel_index < old_min {
                                match self.min_boundary.compare_exchange_weak(
                                    old_min,
                                    otel_index,
                                    Ordering::Release,
                                    Ordering::Relaxed,
                                ) {
                                    Ok(_) => break,
                                    Err(x) => old_min = x,
                                }
                            }
                            return;
                        }
                        // Fall through to normal path if now within bounds
                    }
                }
            } else {
                // Shift too large, cap at bucket[0]
                self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);

                let mut old_min = self.min_boundary.load(Ordering::Relaxed);
                while otel_index < old_min {
                    match self.min_boundary.compare_exchange_weak(
                        old_min,
                        otel_index,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(x) => old_min = x,
                    }
                }
                return;
            }
        }

        // Fast path, within bounds
        if index >= 0 && (index as usize) < INITIAL_CAPACITY {
            self.bucket_counts[index as usize].fetch_add(1, Ordering::Relaxed);

            // Update boundaries atomically
            let mut old_min = self.min_boundary.load(Ordering::Relaxed);
            while otel_index < old_min {
                match self.min_boundary.compare_exchange_weak(
                    old_min,
                    otel_index,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(x) => old_min = x,
                }
            }

            let mut old_max = self.max_boundary.load(Ordering::Relaxed);
            while otel_index > old_max {
                match self.max_boundary.compare_exchange_weak(
                    old_max,
                    otel_index,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(x) => old_max = x,
                }
            }
            return;
        }

        // Slow path: index beyond capacity - cap at max bucket
        if index >= INITIAL_CAPACITY as i32 {
            let cap = INITIAL_CAPACITY - 1;
            self.bucket_counts[cap].fetch_add(1, Ordering::Relaxed);

            let capped_index = offset + cap as i32;
            let mut old_max = self.max_boundary.load(Ordering::Relaxed);
            while capped_index > old_max {
                match self.max_boundary.compare_exchange_weak(
                    old_max,
                    capped_index,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(x) => old_max = x,
                }
            }
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        !self.initialized.load(Ordering::Acquire)
    }

    pub fn offset(&self) -> i32 {
        self.offset.load(Ordering::Relaxed)
    }

    pub fn total_count(&self) -> u64 {
        if !self.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let offset = self.offset.load(Ordering::Acquire);
        let max_boundary = self.max_boundary.load(Ordering::Acquire);
        let end = ((max_boundary - offset + 1) as usize).min(INITIAL_CAPACITY);

        let mut sum = 0u64;
        for i in 0..end {
            sum += self.bucket_counts[i].load(Ordering::Acquire);
        }
        sum
    }

    pub fn as_vec_deque(&self) -> VecDeque<usize> {
        self.bucket_counts
            .clone()
            .as_ref()
            .iter()
            .map(|cnts| cnts.load(Ordering::Acquire) as usize)
            .collect_vec()
            .into()
    }
}
