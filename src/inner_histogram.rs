use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
    },
};

use arc_swap::ArcSwap;
use itertools::Itertools;

/// Pre-allocated capacity to avoid allocations in hot path
const INITIAL_CAPACITY: usize = 256;

/// Maximum capacity to prevent unbounded growth
const MAX_CAPACITY: usize = 16384;

/// Growth factor when resizing
const GROWTH_FACTOR: usize = 2;

// In place of histogram::Histogram
#[derive(Debug)]
pub(crate) struct InnerHistogram {
    bucket_counts: ArcSwap<Box<[AtomicU64]>>,
    // Current capacity of the bucket array
    capacity: AtomicI32,
    // The index of the first entry of OTel data in bucket_counts
    offset: AtomicI32,
    // Our data lies between min_boundry and max_boundry
    pub(crate) min_boundary: AtomicI32,
    pub(crate) max_boundary: AtomicI32,
    // Have we seen any data?
    initialized: AtomicBool,
}

impl Default for InnerHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for InnerHistogram {
    fn clone(&self) -> Self {
        let capacity = self.capacity();
        let buckets = self.bucket_counts.load();

        // Create new bucket array by copying values from old buckets
        let mut new_buckets = Vec::with_capacity(capacity);
        for i in 0..capacity {
            let count = buckets[i].load(Ordering::Acquire);
            new_buckets.push(AtomicU64::new(count));
        }

        Self {
            bucket_counts: ArcSwap::from_pointee(new_buckets.into()),
            capacity: (capacity as i32).into(),
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
            bucket_counts: ArcSwap::from_pointee(buckets.into()),
            capacity: (INITIAL_CAPACITY as i32).into(),
            offset: 0.into(),
            min_boundary: 0.into(),
            max_boundary: 0.into(),
            initialized: false.into(),
        }
    }

    /// Grow the bucket array to accommodate the given required capacity
    /// Returns true if growth occurred
    fn grow_if_needed(&self, required_capacity: usize) -> bool {
        let current_capacity = self.capacity();

        if required_capacity <= current_capacity {
            return false; // No growth needed
        }

        // Calculate new capacity
        let mut new_capacity = current_capacity;
        while new_capacity < required_capacity {
            new_capacity = (new_capacity * GROWTH_FACTOR).min(MAX_CAPACITY);
            if new_capacity >= MAX_CAPACITY {
                break;
            }
        }

        // Cap at MAX_CAPACITY
        new_capacity = new_capacity.min(MAX_CAPACITY);

        if new_capacity <= current_capacity {
            return false; // Already at max or no growth possible
        }

        // Try to atomically update capacity
        match self.capacity.compare_exchange(
            current_capacity as i32,
            new_capacity as i32,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // We won the race, perform the actual growth
                let old_buckets = self.bucket_counts.load();

                // Create new bucket array
                let mut new_buckets = Vec::with_capacity(new_capacity);

                // Copy existing buckets
                for i in 0..current_capacity {
                    let count = old_buckets[i].load(Ordering::Acquire);
                    new_buckets.push(AtomicU64::new(count));
                }

                // Fill remaining with zeros
                new_buckets.resize_with(new_capacity, || AtomicU64::new(0));

                // Swap in the new bucket array
                self.bucket_counts.store(Arc::new(new_buckets.into()));

                true
            }
            Err(_) => {
                // Someone else is growing, they'll handle it
                false
            }
        }
    }

    /// Grow and shift buckets when adding a value with negative index that requires expansion
    fn grow_and_shift(&self, new_offset: i32, shift: usize, required_capacity: usize) -> bool {
        let current_capacity = self.capacity();

        // Calculate new capacity
        let mut new_capacity = current_capacity;
        while new_capacity < required_capacity {
            new_capacity = (new_capacity * GROWTH_FACTOR).min(MAX_CAPACITY);
            if new_capacity >= MAX_CAPACITY {
                break;
            }
        }
        new_capacity = new_capacity.min(MAX_CAPACITY);

        if new_capacity <= current_capacity {
            return false;
        }

        // Try to atomically update both capacity and offset
        match self.capacity.compare_exchange(
            current_capacity as i32,
            new_capacity as i32,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // We won the race
                let old_buckets = self.bucket_counts.load();

                // Create new bucket array with shift
                let mut new_buckets = Vec::with_capacity(new_capacity);

                // Fill shift amount with zeros
                for _ in 0..shift {
                    new_buckets.push(AtomicU64::new(0));
                }

                // Copy existing buckets (shifted by 'shift' positions)
                for i in 0..current_capacity {
                    let count = old_buckets[i].load(Ordering::Acquire);
                    new_buckets.push(AtomicU64::new(count));

                    if new_buckets.len() >= new_capacity {
                        break;
                    }
                }

                // Fill remaining with zeros
                while new_buckets.len() < new_capacity {
                    new_buckets.push(AtomicU64::new(0));
                }

                // Update offset
                self.offset.store(new_offset, Ordering::Release);

                // Swap in the new bucket array
                self.bucket_counts.store(Arc::new(new_buckets.into()));

                true
            }
            Err(_) => {
                // Someone else is growing
                false
            }
        }
    }

    #[inline(always)]
    pub(crate) fn increment(&self, otel_index: i32) {
        // Load bucket pointer once for fast path
        let buckets = self.bucket_counts.load();

        if !self.initialized.load(Ordering::Acquire) {
            if self
                .initialized
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.offset.store(otel_index, Ordering::Release);
                self.min_boundary.store(otel_index, Ordering::Release);
                self.max_boundary.store(otel_index, Ordering::Release);
                buckets[0].store(1, Ordering::Release);
                return;
            }
            // Race lost - call slow path
            return self.increment_slow(otel_index);
        }

        // Fast path - single load of offset and capacity
        let offset = self.offset.load(Ordering::Relaxed);
        let capacity = self.capacity.load(Ordering::Relaxed) as usize;
        let index = otel_index - offset;

        // Fast path: positive index within capacity
        if index >= 0 {
            let idx = index as usize;
            if idx < capacity {
                buckets[idx].fetch_add(1, Ordering::Relaxed);
                self.update_min_boundary(otel_index);
                self.update_max_boundary(otel_index);
                return;
            }
        }

        // Slow path - needs growth or special handling
        self.increment_slow(otel_index);
    }

    // Slow path - not inlined to keep fast path small
    #[inline(never)]
    fn increment_slow(&self, otel_index: i32) {
        let offset = self.offset.load(Ordering::Acquire);
        let index = otel_index - offset;

        if index < 0 {
            // Need to expand downward (shift offset)
            let shift = (-index) as usize;
            let current_capacity = self.capacity();
            let max_boundary = self.max_boundary.load(Ordering::Acquire);
            let required_span = (max_boundary - otel_index + 1) as usize;

            // Check if we need to grow and/or shift
            if required_span > current_capacity {
                self.grow_and_shift(otel_index, shift, required_span);

                // After growth, add to bucket[0]
                let buckets = self.bucket_counts.load();
                buckets[0].fetch_add(1, Ordering::Relaxed);
                self.update_min_boundary(otel_index);
                return;
            }

            // Try to update offset (shift within current capacity)
            let new_capacity = self.capacity();
            if shift < new_capacity {
                match self.offset.compare_exchange(
                    offset,
                    otel_index,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        let buckets = self.bucket_counts.load();
                        buckets[0].fetch_add(1, Ordering::Relaxed);

                        self.update_min_boundary(otel_index);
                        return;
                    }
                    Err(current_offset) => {
                        // Retry with new offset
                        let index = otel_index - current_offset;
                        if index < 0 {
                            // Still negative, cap at bucket[0]
                            let buckets = self.bucket_counts.load();
                            buckets[0].fetch_add(1, Ordering::Relaxed);
                            self.update_min_boundary(otel_index);
                            return;
                        }
                        // Fall through if now in bounds
                    }
                }
            } else {
                // Beyond max capacity, cap at bucket[0]
                let buckets = self.bucket_counts.load();
                buckets[0].fetch_add(1, Ordering::Relaxed);
                self.update_min_boundary(otel_index);
                return;
            }
        }

        // Positive index or fell through from negative handling
        let index = (otel_index - self.offset.load(Ordering::Acquire)) as usize;
        let current_capacity = self.capacity();

        if index >= current_capacity {
            // Need to grow
            let required_capacity = index + 1;
            self.grow_if_needed(required_capacity);

            // After potential growth, check capacity again
            let new_capacity = self.capacity();
            if index < new_capacity {
                let buckets = self.bucket_counts.load();
                buckets[index].fetch_add(1, Ordering::Relaxed);
                self.update_min_boundary(otel_index);
                self.update_max_boundary(otel_index);
            } else {
                // Still beyond capacity (hit MAX_CAPACITY), cap at last bucket
                let buckets = self.bucket_counts.load();
                buckets[new_capacity - 1].fetch_add(1, Ordering::Relaxed);
                self.update_max_boundary(otel_index);
            }
        } else {
            // Within bounds, fast path
            let buckets = self.bucket_counts.load();
            buckets[index].fetch_add(1, Ordering::Relaxed);
            self.update_min_boundary(otel_index);
            self.update_max_boundary(otel_index);
        }
    }

    #[inline]
    fn update_min_boundary(&self, otel_index: i32) {
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
    }

    #[inline]
    fn update_max_boundary(&self, otel_index: i32) {
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
        let capacity = self.capacity.load(Ordering::Acquire);
        let end = ((max_boundary - offset + 1) as usize).min(capacity as usize);

        let buckets = self.bucket_counts.load();
        let mut sum = 0u64;
        for i in 0..end {
            sum += buckets[i].load(Ordering::Acquire);
        }
        sum
    }

    pub fn as_vec_deque(&self) -> VecDeque<usize> {
        let buckets = self.bucket_counts.load();
        buckets
            .as_ref()
            .iter()
            .map(|cnts| cnts.load(Ordering::Acquire) as usize)
            .collect_vec()
            .into()
    }

    /// Used in testing
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.bucket_counts.load().as_ref().len()
    }

    #[allow(dead_code)]
    pub fn load(&self, index: usize) -> usize {
        let buckets = self.bucket_counts.load();
        buckets[index].load(Ordering::Acquire) as usize
    }

    fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Acquire) as usize
    }
}
