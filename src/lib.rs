use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// Maximum scale supported (per OTel spec)
const MAX_SCALE: i32 = 20;

/// Pre-allocated capacity to avoid allocations in hot path
const INITIAL_CAPACITY: usize = 256;

/// Convert a value to an OpenTelemetry exponential histogram bucket index.
/// https://opentelemetry.io/docs/specs/otel/metrics/data-model/#producer-expectations
#[inline(always)]
fn value_to_otel_index(value: f64, scale: i32) -> Option<i32> {
    if value <= 0.0 || !value.is_finite() {
        return None;
    }

    // For negative or 0 scales, bit manipulation instead of OTel formula of
    // index = ceiling(log(value) / log(base)) - 1
    if scale <= 0 {
        let bits = value.to_bits();
        // Wiki: For a f64, the exponent is stored in the range [1, 2046]
        // and is interpreted by subtracting the bias for an 11-bit exponent (1023)
        // to get an exponent value in the range [−1022, 1023].
        //
        // 1. Shift right 52 bits to move the exponent to the low bits
        // 2. 0x7FF, clears the sign bit and keep only the 11 bits of the exponent
        // 3. Subtract the IEEE 754 bias (1023) to get the actual exponent
        let exponent = ((bits >> 52) & 0x7FF) as i32 - 1023;
        return Some(if scale == 0 {
            exponent
        } else {
            exponent >> (-scale)
        });
    }

    // For positive scales
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    Some((value.ln() / base.ln()).ceil() as i32 - 1)
}

// Calculate bucket boundaries, used at export time
#[inline]
fn bucket_boundaries(index: i32, scale: i32) -> (f64, f64) {
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    (base.powi(index), base.powi(index + 1))
}

// In place of histogram::Histogram
#[derive(Clone, Debug)]
struct InnerHistogram {
    bucket_counts: Box<[u64; INITIAL_CAPACITY]>,
    // The index of the first entry of OTel data in bucket_counts
    offset: i32,
    // Our data lies between min_boundry and max_boundry
    min_boundry: i32,
    max_boundry: i32,
    // Have we seen any data?
    initialized: bool,
}

impl InnerHistogram {
    fn new() -> Self {
        Self {
            bucket_counts: Box::new([0u64; INITIAL_CAPACITY]),
            offset: 0,
            min_boundry: 0,
            max_boundry: 0,
            initialized: false,
        }
    }

    #[inline(always)]
    fn increment(&mut self, otel_index: i32) {
        if !self.initialized {
            // First value: set offset
            self.offset = otel_index;
            self.min_boundry = otel_index;
            self.max_boundry = otel_index;
            self.initialized = true;
            self.bucket_counts[0] = 1;
            return;
        }

        let index = otel_index - self.offset;

        // Common path
        if index >= 0 && (index as usize) < INITIAL_CAPACITY {
            self.bucket_counts[index as usize] += 1;
            if otel_index < self.min_boundry {
                self.min_boundry = otel_index;
            }
            if otel_index > self.max_boundry {
                self.max_boundry = otel_index;
            }
            return;
        }

        // Uncommon path, slower
        if index < 0 {
            let shift = (-index) as usize;
            if shift < INITIAL_CAPACITY {
                // Shift existing data right
                for i in (shift..INITIAL_CAPACITY).rev() {
                    self.bucket_counts[i] = self.bucket_counts[i - shift];
                }
                // Zero out new space
                for i in 0..shift {
                    self.bucket_counts[i] = 0;
                }
                self.bucket_counts[0] = 1;
                self.offset = otel_index;
                self.min_boundry = otel_index;
            }
        } else {
            // Beyond capacity: cap at max bucket
            let cap = (INITIAL_CAPACITY - 1).min(index as usize);
            self.bucket_counts[cap] += 1;
            if otel_index > self.max_boundry {
                self.max_boundry = self.max_boundry.max(self.offset + cap as i32);
            }
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        !self.initialized
    }

    fn total_count(&self) -> u64 {
        if !self.initialized {
            return 0;
        }
        let end = ((self.max_boundry - self.offset + 1) as usize).min(INITIAL_CAPACITY);
        self.bucket_counts[0..end].iter().sum()
    }

    fn min_index(&self) -> Option<i32> {
        if !self.initialized {
            None
        } else {
            Some(self.min_boundry)
        }
    }

    fn iter(&self) -> impl Iterator<Item = (i32, u64)> {
        if !self.initialized {
            return IterHelper::Empty;
        }
        let end = ((self.max_boundry - self.offset + 1) as usize).min(INITIAL_CAPACITY);
        IterHelper::Active {
            buckets: &self.bucket_counts[0..end],
            offset: self.offset,
            index: 0,
        }
    }

    fn to_vec_deque(&self) -> VecDeque<usize> {
        if !self.initialized {
            return VecDeque::new();
        }
        let end = ((self.max_boundry - self.offset + 1) as usize).min(INITIAL_CAPACITY);
        self.bucket_counts[0..end]
            .iter()
            .map(|&c| c as usize)
            .collect()
    }
}

enum IterHelper<'a> {
    Empty,
    Active {
        buckets: &'a [u64],
        offset: i32,
        index: usize,
    },
}

impl<'a> Iterator for IterHelper<'a> {
    type Item = (i32, u64);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IterHelper::Empty => None,
            IterHelper::Active {
                buckets,
                offset,
                index,
            } => {
                while *index < buckets.len() {
                    let i = *index;
                    *index += 1;
                    if buckets[i] > 0 {
                        return Some((*offset + i as i32, buckets[i]));
                    }
                }
                None
            }
        }
    }
}

/// An exponential histogram that stores f64 values in OpenTelemetry proto like buckets
/// https://github.com/open-telemetry/opentelemetry-proto/blob/cfbf9357c03bf4ac150a3ab3bcbe4cc4ed087362/opentelemetry/proto/metrics/v1/metrics.proto#L466
pub struct ExponentialHistogram {
    positive_buckets: InnerHistogram,
    negative_buckets: InnerHistogram,
    // zero_count is the count of values that are either exactly zero or
    // within the zero_threshold, which defaults to 0.0
    zero_count: u64,
    zero_threshold: f64,
    // scale describes the resolution of the histogram
    scale: i32,
    // running sum of values seen so far
    sum: f64,
    // min and max values seen so far
    min_value: Option<f64>,
    max_value: Option<f64>,
}

impl std::fmt::Debug for ExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExponentialHistogram")
            .field("scale", &self.scale)
            .field("zero_threshold", &self.zero_threshold)
            .field("count", &self.count())
            .field("zero_count", &self.zero_count)
            .field("sum", &self.sum)
            .field("min", &self.min())
            .field("max", &self.max())
            .field("has_negatives", &self.has_negatives())
            .finish()
    }
}

impl Clone for ExponentialHistogram {
    fn clone(&self) -> Self {
        Self {
            positive_buckets: self.positive_buckets.clone(),
            negative_buckets: self.negative_buckets.clone(),
            zero_count: self.zero_count,
            zero_threshold: self.zero_threshold,
            scale: self.scale,
            sum: self.sum,
            min_value: self.min_value,
            max_value: self.max_value,
        }
    }
}

impl PartialEq for ExponentialHistogram {
    fn eq(&self, other: &Self) -> bool {
        if self.scale != other.scale
            || self.zero_count != other.zero_count
            || (self.zero_threshold - other.zero_threshold).abs() >= f64::EPSILON
            || (self.sum - other.sum).abs() >= f64::EPSILON
        {
            return false;
        }

        let self_positive: Vec<_> = self.positive_buckets.iter().collect();
        let other_positive: Vec<_> = other.positive_buckets.iter().collect();
        if self_positive != other_positive {
            return false;
        }

        let self_negative: Vec<_> = self.negative_buckets.iter().collect();
        let other_negative: Vec<_> = other.negative_buckets.iter().collect();

        self_negative == other_negative
            && self.min_value == other.min_value
            && self.max_value == other.max_value
    }
}

impl std::fmt::Display for ExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ExponentialHistogram(scale={}, count={}, sum={:.2}, min={:.2}, max={:.2})",
            self.scale,
            self.count(),
            self.sum(),
            self.min(),
            self.max()
        )
    }
}

impl Default for ExponentialHistogram {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ExponentialHistogram {
    pub fn new(desired_scale: i32) -> Self {
        Self::new_with_max_buckets(desired_scale, 160)
    }

    pub fn new_with_max_buckets(desired_scale: i32, _ignored: u16) -> Self {
        let scale = desired_scale.min(MAX_SCALE);
        Self {
            positive_buckets: InnerHistogram::new(),
            negative_buckets: InnerHistogram::new(),
            zero_count: 0,
            zero_threshold: 0.0,
            scale,
            sum: 0.0,
            min_value: None,
            max_value: None,
        }
    }

    pub fn with_zero_threshold(mut self, threshold: f64) -> Self {
        self.zero_threshold = threshold;
        self
    }

    pub fn reset(&mut self) {
        self.positive_buckets = InnerHistogram::new();
        self.negative_buckets = InnerHistogram::new();
        self.zero_count = 0;
        self.sum = 0.0;
        self.min_value = None;
        self.max_value = None;
    }

    #[inline(always)]
    pub fn accumulate<T: Into<f64>>(&mut self, value: T) {
        let val: f64 = value.into();

        if !val.is_finite() {
            return;
        }

        self.sum += val;
        self.min_value = Some(self.min_value.map_or(val, |m| m.min(val)));
        self.max_value = Some(self.max_value.map_or(val, |m| m.max(val)));

        if self.zero_threshold == 0.0 && val == 0.0 {
            self.zero_count += 1;
            return;
        } else if val.abs() <= self.zero_threshold {
            self.zero_count += 1;
            return;
        }

        // Determine bucket which bucket to use
        let abs_val = val.abs();
        let buckets = if val >= 0.0 {
            &mut self.positive_buckets
        } else {
            &mut self.negative_buckets
        };

        if let Some(index) = value_to_otel_index(abs_val, self.scale) {
            buckets.increment(index);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn count(&self) -> usize {
        (self.positive_buckets.total_count()
            + self.negative_buckets.total_count()
            + self.zero_count) as usize
    }

    pub fn sum(&self) -> f64 {
        self.sum
    }

    pub fn min(&self) -> f64 {
        self.min_value.unwrap_or(0.0)
    }

    pub fn max(&self) -> f64 {
        self.max_value.unwrap_or(0.0)
    }

    pub fn scale(&self) -> i32 {
        self.scale
    }

    pub fn zero_count(&self) -> u64 {
        self.zero_count
    }

    pub fn bucket_start_offset(&self) -> i32 {
        self.positive_buckets.min_index().unwrap_or(0)
    }

    pub fn negative_bucket_start_offset(&self) -> i32 {
        self.negative_buckets.min_index().unwrap_or(0)
    }

    pub fn has_negatives(&self) -> bool {
        !self.negative_buckets.is_empty()
    }

    pub fn take_counts(self) -> (VecDeque<usize>, VecDeque<usize>) {
        let positive_counts = self.positive_buckets.to_vec_deque();
        let negative_counts = self.negative_buckets.to_vec_deque();
        (positive_counts, negative_counts)
    }

    pub fn value_counts(&self) -> impl Iterator<Item = (f64, usize)> + '_ {
        let positive_iter = self.positive_buckets.iter().map(move |(idx, count)| {
            let (_, upper) = bucket_boundaries(idx, self.scale);
            (upper, count as usize)
        });

        let negative_iter = self.negative_buckets.iter().map(move |(idx, count)| {
            let (_, upper) = bucket_boundaries(idx, self.scale);
            (-upper, count as usize)
        });

        negative_iter.chain(positive_iter)
    }
}

pub struct SharedExponentialHistogram {
    inner: Arc<Mutex<ExponentialHistogram>>,
}

impl std::fmt::Debug for SharedExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.snapshot();
        f.debug_struct("SharedExponentialHistogram")
            .field("scale", &snapshot.scale)
            .field("count", &snapshot.count())
            .field("sum", &snapshot.sum())
            .field("min", &snapshot.min())
            .field("max", &snapshot.max())
            .field("has_negatives", &snapshot.has_negatives())
            .finish()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_histogram_basic() {
        let mut hist = ExponentialHistogram::new(0);
        assert!(hist.is_empty());
        assert_eq!(hist.count(), 0);

        hist.accumulate(1.0);
        hist.accumulate(2.0);
        hist.accumulate(3.0);

        assert!(!hist.is_empty());
        assert_eq!(hist.count(), 3);
        assert_eq!(hist.sum(), 6.0);
        assert_eq!(hist.min(), 1.0);
        assert_eq!(hist.max(), 3.0);
    }

    #[test]
    fn test_exponential_histogram_with_max_buckets() {
        let mut hist = ExponentialHistogram::new_with_max_buckets(2, 100);
        hist.accumulate(10.0);
        assert_eq!(hist.count(), 1);
        assert_eq!(hist.scale(), 2);
    }

    #[test]
    fn test_exponential_histogram_reset() {
        let mut hist = ExponentialHistogram::new(0);
        hist.accumulate(5.0);
        hist.accumulate(10.0);

        assert_eq!(hist.count(), 2);

        hist.reset();

        assert!(hist.is_empty());
        assert_eq!(hist.count(), 0);
    }

    #[test]
    fn test_exponential_histogram_negatives() {
        let mut hist = ExponentialHistogram::new(0);
        hist.accumulate(-5.0);
        hist.accumulate(10.0);
        hist.accumulate(-2.0);

        assert!(hist.has_negatives());
        assert_eq!(hist.count(), 3);
        assert_eq!(hist.min(), -5.0);
        assert_eq!(hist.max(), 10.0);
        assert_eq!(hist.sum(), 3.0);
    }

    #[test]
    fn test_fractional_values() {
        let mut hist = ExponentialHistogram::new(4);
        hist.accumulate(0.1);
        hist.accumulate(0.5);
        hist.accumulate(1.5);
        hist.accumulate(2.7);

        assert_eq!(hist.count(), 4);
        assert!(
            (hist.sum() - 4.8).abs() < 1e-10,
            "sum should be approximately 4.8, got {}",
            hist.sum()
        );
        assert_eq!(hist.min(), 0.1);
        assert_eq!(hist.max(), 2.7);
    }

    #[test]
    fn test_zero_handling() {
        let mut hist = ExponentialHistogram::new(0);
        hist.accumulate(0.0);
        hist.accumulate(0.0);
        hist.accumulate(1.0);

        assert_eq!(hist.count(), 3);
        assert_eq!(hist.zero_count(), 2);
        assert_eq!(hist.sum(), 1.0);
    }

    #[test]
    fn test_shared_exponential_histogram() {
        let hist = SharedExponentialHistogram::new(0);

        hist.accumulate(1.0);
        hist.accumulate(2.0);
        hist.accumulate(3.0);

        let snapshot = hist.snapshot();
        assert_eq!(snapshot.count(), 3);
        assert_eq!(snapshot.sum(), 6.0);

        let snapshot2 = hist.snapshot();
        assert_eq!(snapshot2.count(), 3);
    }

    #[test]
    fn test_shared_exponential_histogram_reset() {
        let hist = SharedExponentialHistogram::new(0);

        hist.accumulate(1.0);
        hist.accumulate(2.0);

        let snapshot = hist.snapshot_and_reset();
        assert_eq!(snapshot.count(), 2);

        let snapshot2 = hist.snapshot();
        assert_eq!(snapshot2.count(), 0);
    }

    #[test]
    fn test_value_counts_iterator() {
        let mut hist = ExponentialHistogram::new(0);
        hist.accumulate(1.0);
        hist.accumulate(5.0);
        hist.accumulate(10.0);

        let counts: Vec<_> = hist.value_counts().collect();
        assert!(!counts.is_empty());
    }

    #[test]
    fn test_take_counts() {
        let mut hist = ExponentialHistogram::new(0);
        hist.accumulate(1.0);
        hist.accumulate(5.0);
        hist.accumulate(10.0);

        let (positive, _negative) = hist.take_counts();
        assert!(!positive.is_empty());
    }

    #[test]
    fn test_shared_exponential_histogram_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let hist = Arc::new(SharedExponentialHistogram::new(0));
        let mut handles = vec![];

        for i in 0..10 {
            let hist_clone = Arc::clone(&hist);
            let handle = thread::spawn(move || {
                for j in 0..100 {
                    hist_clone.accumulate((i * 100 + j) as f64);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = hist.snapshot();
        assert_eq!(snapshot.count(), 1000);
    }

    #[test]
    fn test_debug_implementation() {
        let mut hist = ExponentialHistogram::new(2);
        hist.accumulate(5.0);
        hist.accumulate(-3.0);
        hist.accumulate(10.0);

        let debug_output = format!("{:?}", hist);
        assert!(debug_output.contains("ExponentialHistogram"));
        assert!(debug_output.contains("scale: 2"));
        assert!(debug_output.contains("count: 3"));
        assert!(debug_output.contains("has_negatives: true"));

        let shared = SharedExponentialHistogram::new(1);
        shared.accumulate(1.0);
        shared.accumulate(2.0);

        let debug_output = format!("{:?}", shared);
        assert!(debug_output.contains("SharedExponentialHistogram"));
        assert!(debug_output.contains("scale: 1"));
        assert!(debug_output.contains("count: 2"));
    }

    #[test]
    fn test_clone_implementation() {
        let mut hist = ExponentialHistogram::new(2);
        hist.accumulate(5.0);
        hist.accumulate(-3.0);
        hist.accumulate(10.0);

        let cloned = hist.clone();

        assert_eq!(hist.count(), cloned.count());
        assert_eq!(hist.scale(), cloned.scale());
        assert_eq!(hist.has_negatives(), cloned.has_negatives());
        assert_eq!(hist.min(), cloned.min());
        assert_eq!(hist.max(), cloned.max());
    }

    #[test]
    fn test_partial_eq_implementation() {
        let mut hist1 = ExponentialHistogram::new(2);
        hist1.accumulate(5.0);
        hist1.accumulate(-3.0);
        hist1.accumulate(10.0);

        let mut hist2 = ExponentialHistogram::new(2);
        hist2.accumulate(5.0);
        hist2.accumulate(-3.0);
        hist2.accumulate(10.0);

        assert_eq!(hist1, hist2);

        hist2.accumulate(20.0);
        assert_ne!(hist1, hist2);

        let mut hist3 = ExponentialHistogram::new(1);
        hist3.accumulate(5.0);
        hist3.accumulate(-3.0);
        hist3.accumulate(10.0);
        assert_ne!(hist1, hist3);

        let hist4 = hist1.clone();
        assert_eq!(hist1, hist4);
    }

    #[test]
    fn test_display_implementation() {
        let mut hist = ExponentialHistogram::new(2);
        hist.accumulate(5.0);
        hist.accumulate(-3.0);
        hist.accumulate(10.0);

        let display_output = format!("{}", hist);
        assert!(display_output.contains("ExponentialHistogram"));
        assert!(display_output.contains("scale=2"));
        assert!(display_output.contains("count=3"));

        let empty_hist = ExponentialHistogram::new(0);
        let empty_output = format!("{}", empty_hist);
        assert!(empty_output.contains("count=0"));
        assert!(empty_output.contains("sum=0.00"));
    }

    #[test]
    fn test_default_implementation() {
        let hist = ExponentialHistogram::default();
        assert_eq!(hist.scale(), 0);
        assert_eq!(hist.count(), 0);
        assert!(hist.is_empty());

        let shared = SharedExponentialHistogram::default();
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.scale(), 0);
        assert_eq!(snapshot.count(), 0);
        assert!(snapshot.is_empty());

        let mut hist2 = ExponentialHistogram::default();
        hist2.accumulate(5.0);
        assert_eq!(hist2.count(), 1);

        let shared2 = SharedExponentialHistogram::default();
        shared2.accumulate(10.0);
        assert_eq!(shared2.snapshot().count(), 1);
    }

    #[test]
    fn test_otel_index_conversion() {
        let scale = 7;
        let mut hist = ExponentialHistogram::new(scale);

        let test_values: Vec<u64> = vec![
            1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
        ];

        for &v in &test_values {
            hist.accumulate(v as f64);
        }

        let offset = hist.bucket_start_offset();
        let (positive_counts, negative_counts) = hist.take_counts();

        let total_count: usize = positive_counts.iter().sum();
        assert_eq!(total_count, test_values.len());
        assert!(negative_counts.is_empty());

        println!("Scale: {}", scale);
        println!("Offset: {}", offset);
        println!("Bucket counts: {:?}", positive_counts);
    }

    #[test]
    fn test_otel_bucket_boundaries() {
        let scale = 0;

        let (lower, upper) = bucket_boundaries(0, scale);
        assert_eq!(lower, 1.0);
        assert_eq!(upper, 2.0);

        let (lower, upper) = bucket_boundaries(1, scale);
        assert_eq!(lower, 2.0);
        assert_eq!(upper, 4.0);
    }

    #[test]
    fn test_otel_index_calculation() {
        // Test scale 0 (base = 2)
        assert_eq!(value_to_otel_index(1.5, 0), Some(0)); // (1, 2]
        assert_eq!(value_to_otel_index(2.5, 0), Some(1)); // (2, 4]
        assert_eq!(value_to_otel_index(5.0, 0), Some(2)); // (4, 8]

        // Test fractional values
        assert_eq!(value_to_otel_index(0.5, 0), Some(-1)); // (0.5, 1]
        assert_eq!(value_to_otel_index(0.25, 0), Some(-2)); // (0.25, 0.5]
    }

    #[test]
    fn test_bucket_array_indexing() {
        let mut array = InnerHistogram::new();

        // Test forward indexing
        array.increment(5);
        array.increment(10);
        array.increment(7);

        assert_eq!(array.offset, 5);
        assert_eq!(array.min_boundry, 5);
        assert_eq!(array.max_boundry, 10);

        // Test backward indexing (shift)
        array.increment(2);
        assert_eq!(array.offset, 2);
        assert_eq!(array.min_boundry, 2);
        assert_eq!(array.max_boundry, 10);
    }
}
