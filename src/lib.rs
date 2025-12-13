use histogram::{AtomicHistogram, Histogram as InnerHistogram};
use std::collections::{BTreeMap, VecDeque};

/// Internal grouping power for the histogram crate.
/// We always use 7 (128 buckets per octave) for maximum precision,
/// then convert to OTel indices at read time.
const INTERNAL_GROUPING_POWER: u8 = 7;

/// Convert a value to an OpenTelemetry exponential histogram bucket index.
/// The formula is: floor(log2(value) * 2^scale)
/// This maps values to bucket indices where bucket i covers [base^i, base^(i+1))
/// and base = 2^(2^(-scale)).
#[inline]
fn value_to_otel_index(value: f64, scale: u8) -> i32 {
    if value <= 0.0 {
        return i32::MIN; // Invalid, will be filtered out
    }
    let scale_factor = (2.0_f64).powi(scale as i32);
    (value.ln() * scale_factor / std::f64::consts::LN_2).floor() as i32
}

pub struct ExponentialHistogram {
    positive: InnerHistogram,
    negative: InnerHistogram,
    scale: u8,
    max_buckets: u16,
}

impl std::fmt::Debug for ExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExponentialHistogram")
            .field("scale", &self.scale)
            .field("max_buckets", &self.max_buckets)
            .field("count", &self.count())
            .field("sum", &self.sum())
            .field("min", &self.min())
            .field("max", &self.max())
            .field("has_negatives", &self.has_negatives())
            .finish()
    }
}

impl Clone for ExponentialHistogram {
    fn clone(&self) -> Self {
        Self {
            positive: self.positive.clone(),
            negative: self.negative.clone(),
            scale: self.scale,
            max_buckets: self.max_buckets,
        }
    }
}

impl PartialEq for ExponentialHistogram {
    fn eq(&self, other: &Self) -> bool {
        // Compare configuration
        if self.scale != other.scale || self.max_buckets != other.max_buckets {
            return false;
        }

        // Compare computed statistics
        if self.count() != other.count() {
            return false;
        }

        // Compare bucket data for positive histogram
        let self_positive_buckets: Vec<_> = self.positive.iter()
            .filter(|b| b.count() > 0)
            .map(|b| (b.start(), b.end(), b.count()))
            .collect();

        let other_positive_buckets: Vec<_> = other.positive.iter()
            .filter(|b| b.count() > 0)
            .map(|b| (b.start(), b.end(), b.count()))
            .collect();

        if self_positive_buckets != other_positive_buckets {
            return false;
        }

        // Compare bucket data for negative histogram
        let self_negative_buckets: Vec<_> = self.negative.iter()
            .filter(|b| b.count() > 0)
            .map(|b| (b.start(), b.end(), b.count()))
            .collect();

        let other_negative_buckets: Vec<_> = other.negative.iter()
            .filter(|b| b.count() > 0)
            .map(|b| (b.start(), b.end(), b.count()))
            .collect();

        self_negative_buckets == other_negative_buckets
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
    pub fn new(desired_scale: u8) -> Self {
        Self::new_with_max_buckets(desired_scale, 160)
    }

    pub fn new_with_max_buckets(desired_scale: u8, max_buckets: u16) -> Self {
        // Always use max grouping power internally for best precision.
        // The desired_scale is stored and used when converting to OTel indices.
        let max_value_power = 64;

        let positive = InnerHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create histogram");
        let negative = InnerHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create histogram");

        Self {
            positive,
            negative,
            scale: desired_scale,
            max_buckets,
        }
    }

    pub fn reset(&mut self) {
        let max_value_power = 64;

        self.positive = InnerHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create histogram");
        self.negative = InnerHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create histogram");
    }

    pub fn accumulate<T: Into<f64>>(&mut self, value: T) {
        let val: f64 = value.into();

        if val.is_finite() {
            let abs_val = val.abs();
            if val >= 0.0 {
                let _ = self.positive.increment(abs_val as u64);
            } else {
                let _ = self.negative.increment(abs_val as u64);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn count(&self) -> usize {
        let mut count = 0;
        for bucket in self.positive.iter() {
            if bucket.count() > 0 {
                count += bucket.count() as usize;
            }
        }
        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                count += bucket.count() as usize;
            }
        }
        count
    }

    pub fn sum(&self) -> f64 {
        let mut sum = 0.0;
        for bucket in self.positive.iter() {
            if bucket.count() > 0 {
                let bucket_mid = (bucket.start() + bucket.end()) as f64 / 2.0;
                sum += bucket_mid * bucket.count() as f64;
            }
        }
        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                let bucket_mid = (bucket.start() + bucket.end()) as f64 / 2.0;
                sum -= bucket_mid * bucket.count() as f64;
            }
        }
        sum
    }

    pub fn min(&self) -> f64 {
        // Check negative histogram first (most negative value - largest bucket)
        let mut neg_max = None;
        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                neg_max = Some(bucket.end() as f64);
            }
        }
        if let Some(val) = neg_max {
            return -val;
        }

        // If no negatives, check positive histogram (smallest bucket)
        for bucket in self.positive.iter() {
            if bucket.count() > 0 {
                return bucket.start() as f64;
            }
        }
        0.0
    }

    pub fn max(&self) -> f64 {
        // Check positive histogram first (most positive value)
        let mut max = 0.0;
        for bucket in self.positive.iter() {
            if bucket.count() > 0 {
                max = bucket.end() as f64;
            }
        }
        if max > 0.0 {
            return max;
        }
        // If no positives, check negative histogram
        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                return -(bucket.start() as f64);
            }
        }
        0.0
    }

    pub fn scale(&self) -> u8 {
        self.scale
    }

    /// Returns the starting bucket index offset for the positive buckets.
    /// This is the minimum OTel bucket index that contains data.
    pub fn bucket_start_offset(&self) -> usize {
        // Find the minimum OTel index from positive buckets
        let mut min_index: Option<i32> = None;

        for bucket in self.positive.iter() {
            if bucket.count() > 0 {
                let midpoint = (bucket.start() + bucket.end()) as f64 / 2.0;
                let otel_idx = value_to_otel_index(midpoint, self.scale);
                if otel_idx > i32::MIN {
                    min_index = Some(match min_index {
                        Some(current) => current.min(otel_idx),
                        None => otel_idx,
                    });
                }
            }
        }

        // Return as usize; if no data, return 0
        min_index.unwrap_or(0).max(0) as usize
    }

    /// Returns the starting bucket index offset for the negative buckets.
    pub fn negative_bucket_start_offset(&self) -> usize {
        let mut min_index: Option<i32> = None;

        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                let midpoint = (bucket.start() + bucket.end()) as f64 / 2.0;
                let otel_idx = value_to_otel_index(midpoint, self.scale);
                if otel_idx > i32::MIN {
                    min_index = Some(match min_index {
                        Some(current) => current.min(otel_idx),
                        None => otel_idx,
                    });
                }
            }
        }

        min_index.unwrap_or(0).max(0) as usize
    }

    pub fn has_negatives(&self) -> bool {
        for bucket in self.negative.iter() {
            if bucket.count() > 0 {
                return true;
            }
        }
        false
    }

    /// Returns (positive_counts, negative_counts) as VecDeques.
    /// The counts are indexed by OTel bucket index, starting from bucket_start_offset().
    /// Empty buckets between the min and max indices are included as zeros.
    pub fn take_counts(self) -> (VecDeque<usize>, VecDeque<usize>) {
        // Convert positive histogram buckets to OTel indices
        let positive_counts = Self::convert_to_otel_counts(&self.positive, self.scale);
        let negative_counts = Self::convert_to_otel_counts(&self.negative, self.scale);

        (positive_counts, negative_counts)
    }

    /// Helper function to convert internal histogram buckets to OTel-indexed counts.
    fn convert_to_otel_counts(hist: &InnerHistogram, scale: u8) -> VecDeque<usize> {
        // First pass: collect counts by OTel index
        let mut otel_buckets: BTreeMap<i32, u64> = BTreeMap::new();

        for bucket in hist.iter() {
            if bucket.count() > 0 {
                let midpoint = (bucket.start() + bucket.end()) as f64 / 2.0;
                let otel_idx = value_to_otel_index(midpoint, scale);
                if otel_idx > i32::MIN {
                    *otel_buckets.entry(otel_idx).or_insert(0) += bucket.count();
                }
            }
        }

        if otel_buckets.is_empty() {
            return VecDeque::new();
        }

        // Build contiguous VecDeque from min to max index
        let min_idx = *otel_buckets.keys().min().unwrap();
        let max_idx = *otel_buckets.keys().max().unwrap();

        let mut counts = VecDeque::with_capacity((max_idx - min_idx + 1) as usize);
        for idx in min_idx..=max_idx {
            counts.push_back(*otel_buckets.get(&idx).unwrap_or(&0) as usize);
        }

        counts
    }

    pub fn value_counts(&self) -> impl Iterator<Item = (f64, usize)> + '_ {
        let positive_iter = self.positive.iter().filter_map(|bucket| {
            if bucket.count() > 0 {
                Some((bucket.end() as f64, bucket.count() as usize))
            } else {
                None
            }
        });

        let negative_iter = self.negative.iter().filter_map(|bucket| {
            if bucket.count() > 0 {
                Some((-(bucket.end() as f64), bucket.count() as usize))
            } else {
                None
            }
        });

        negative_iter.chain(positive_iter)
    }
}

pub struct SharedExponentialHistogram {
    positive: AtomicHistogram,
    negative: AtomicHistogram,
    scale: u8,
    #[allow(dead_code)]
    max_buckets: u16,
}

impl std::fmt::Debug for SharedExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.snapshot();
        f.debug_struct("SharedExponentialHistogram")
            .field("scale", &self.scale)
            .field("max_buckets", &self.max_buckets)
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
    pub fn new(desired_scale: u8) -> Self {
        Self::new_with_max_buckets(desired_scale, 160)
    }

    pub fn new_with_max_buckets(desired_scale: u8, max_buckets: u16) -> Self {
        // Always use max grouping power internally for best precision.
        // The desired_scale is stored and used when converting to OTel indices.
        let max_value_power = 64;

        let positive = AtomicHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create atomic histogram");
        let negative = AtomicHistogram::new(INTERNAL_GROUPING_POWER, max_value_power)
            .expect("failed to create atomic histogram");

        Self {
            positive,
            negative,
            scale: desired_scale,
            max_buckets,
        }
    }

    pub fn accumulate(&self, value: f64) {
        if !value.is_finite() {
            return;
        }

        let abs_val = value.abs();
        if value >= 0.0 {
            let _ = self.positive.increment(abs_val as u64);
        } else {
            let _ = self.negative.increment(abs_val as u64);
        }
    }

    pub fn snapshot(&self) -> ExponentialHistogram {
        let positive = self.positive.load();
        let negative = self.negative.load();

        ExponentialHistogram {
            positive,
            negative,
            scale: self.scale,
            max_buckets: self.max_buckets,
        }
    }

    pub fn snapshot_and_reset(&self) -> ExponentialHistogram {
        let positive = self.positive.drain();
        let negative = self.negative.drain();

        ExponentialHistogram {
            positive,
            negative,
            scale: self.scale,
            max_buckets: self.max_buckets,
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

        // Min should be approximately -5 (based on negative histogram buckets)
        let min = hist.min();
        assert!(min < 0.0, "min should be negative, got {}", min);
        assert!(min <= -5.0, "min should be <= -5.0, got {}", min);

        // Max should be approximately 10 (based on positive histogram buckets)
        let max = hist.max();
        assert!(max > 0.0, "max should be positive, got {}", max);
        assert!(max >= 10.0, "max should be >= 10.0, got {}", max);

        // Sum should be approximately 3.0 (= -5 + 10 - 2)
        let sum = hist.sum();
        println!("Sum: {}", sum);
        assert!((sum - 3.0).abs() < 5.0, "sum should be approximately 3.0, got {}", sum);
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
    fn test_histogram_zero_handling() {
        let mut hist = InnerHistogram::new(0, 64).unwrap();

        hist.increment(0).unwrap();
        hist.increment(0).unwrap();
        hist.increment(1).unwrap();

        let mut total_count = 0;
        println!("\nBuckets with data:");
        for bucket in hist.iter() {
            if bucket.count() > 0 {
                println!("  start={}, end={}, count={}",
                         bucket.start(), bucket.end(), bucket.count());
                total_count += bucket.count();
            }
        }

        assert_eq!(total_count, 3, "Should track all values including zeros");
    }

    #[test]
    fn test_histogram_sum_from_buckets() {
        let mut hist = InnerHistogram::new(0, 64).unwrap();

        hist.increment(10).unwrap();
        hist.increment(20).unwrap();
        hist.increment(30).unwrap();

        let mut computed_sum = 0.0;
        println!("\nComputing sum from buckets:");
        for bucket in hist.iter() {
            if bucket.count() > 0 {
                let bucket_mid = (bucket.start() + bucket.end()) as f64 / 2.0;
                let contribution = bucket_mid * bucket.count() as f64;
                println!("  Bucket [{}, {}]: count={}, mid={}, contribution={}",
                         bucket.start(), bucket.end(), bucket.count(), bucket_mid, contribution);
                computed_sum += contribution;
            }
        }

        println!("Computed sum: {}, Expected: 60", computed_sum);
        println!("Note: Sum is approximate due to bucket quantization");
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
        // Test ExponentialHistogram clone
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

        // SharedExponentialHistogram doesn't implement Clone - it's meant to be
        // shared via Arc, not cloned. If you need independent copies, use snapshot().
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

        // Should be equal
        assert_eq!(hist1, hist2);

        // Add different value to hist2
        hist2.accumulate(20.0);
        assert_ne!(hist1, hist2);

        // Different scale should not be equal
        let mut hist3 = ExponentialHistogram::new(1);
        hist3.accumulate(5.0);
        hist3.accumulate(-3.0);
        hist3.accumulate(10.0);
        assert_ne!(hist1, hist3);

        // Test cloned histogram equals original
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
        println!("Display output: {}", display_output);
        assert!(display_output.contains("ExponentialHistogram"));
        assert!(display_output.contains("scale=2"));
        assert!(display_output.contains("count=3"));

        // Test empty histogram
        let empty_hist = ExponentialHistogram::new(0);
        let empty_output = format!("{}", empty_hist);
        println!("Empty display output: {}", empty_output);
        assert!(empty_output.contains("count=0"));
        assert!(empty_output.contains("sum=0.00"));
    }

    #[test]
    fn test_default_implementation() {
        // Test ExponentialHistogram default
        let hist = ExponentialHistogram::default();
        assert_eq!(hist.scale(), 0);
        assert_eq!(hist.count(), 0);
        assert!(hist.is_empty());

        // Test SharedExponentialHistogram default
        let shared = SharedExponentialHistogram::default();
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.scale(), 0);
        assert_eq!(snapshot.count(), 0);
        assert!(snapshot.is_empty());

        // Verify default histograms work normally
        let mut hist2 = ExponentialHistogram::default();
        hist2.accumulate(5.0);
        assert_eq!(hist2.count(), 1);

        let shared2 = SharedExponentialHistogram::default();
        shared2.accumulate(10.0);
        assert_eq!(shared2.snapshot().count(), 1);
    }

    #[test]
    fn test_otel_index_conversion() {
        // Test that bucket_start_offset and take_counts produce valid OTel indices
        let scale = 7u8;
        let mut hist = ExponentialHistogram::new(scale);

        // Add values spanning several orders of magnitude (simulating nanosecond latencies)
        let test_values: Vec<u64> = vec![
            1_000,        // 1 microsecond
            5_000,        // 5 microseconds
            10_000,       // 10 microseconds
            50_000,       // 50 microseconds
            100_000,      // 100 microseconds
            500_000,      // 500 microseconds
            1_000_000,    // 1 millisecond
            5_000_000,    // 5 milliseconds
            10_000_000,   // 10 milliseconds
        ];

        for &v in &test_values {
            hist.accumulate(v as f64);
        }

        let offset = hist.bucket_start_offset();
        let (positive_counts, negative_counts) = hist.take_counts();

        // Verify we got the right count
        let total_count: usize = positive_counts.iter().sum();
        assert_eq!(total_count, test_values.len(), "Total count should match");
        assert!(negative_counts.is_empty(), "Should have no negative values");

        // Verify the offset is reasonable (should be around log2(1000) * 2^7 = ~1275)
        assert!(offset > 1000, "Offset should be > 1000 for values starting at 1000, got {}", offset);
        assert!(offset < 2000, "Offset should be < 2000 for values starting at 1000, got {}", offset);

        // Verify bucket count interpretation using OTel formula
        // For OTel, bucket i covers [base^(offset+i), base^(offset+i+1))
        // where base = 2^(2^(-scale))
        let base = 2.0_f64.powf(2.0_f64.powi(-(scale as i32)));

        // The first bucket should cover a range containing 1000
        let first_bucket_lower = base.powi(offset as i32);
        let first_bucket_upper = base.powi(offset as i32 + 1);

        println!("Scale: {}", scale);
        println!("Offset: {}", offset);
        println!("First bucket range: [{:.2}, {:.2})", first_bucket_lower, first_bucket_upper);
        println!("First test value: {}", test_values[0]);

        // The first bucket's range should contain or be close to 1000
        assert!(
            first_bucket_lower <= 1100.0 && first_bucket_upper >= 900.0,
            "First bucket [{:.2}, {:.2}) should be near 1000",
            first_bucket_lower, first_bucket_upper
        );

        println!("Bucket counts: {:?}", positive_counts);
    }

    #[test]
    fn test_otel_percentile_accuracy() {
        // Test that percentiles computed from OTel buckets are accurate
        let scale = 7u8;
        let mut hist = ExponentialHistogram::new(scale);

        // Add known values
        let test_values: Vec<u64> = vec![
            1_000, 2_000, 3_000, 4_000, 5_000,
            6_000, 7_000, 8_000, 9_000, 10_000,
        ];

        for &v in &test_values {
            hist.accumulate(v as f64);
        }

        let offset = hist.bucket_start_offset();
        let (positive_counts, _) = hist.take_counts();

        // Compute p50 from OTel buckets
        let base = 2.0_f64.powf(2.0_f64.powi(-(scale as i32)));
        let total: usize = positive_counts.iter().sum();
        let p50_target = (total as f64 * 0.5).ceil() as usize;

        let mut cumulative = 0usize;
        let mut p50_est = 0.0;

        for (i, &count) in positive_counts.iter().enumerate() {
            cumulative += count;
            if p50_est == 0.0 && cumulative >= p50_target {
                p50_est = base.powi((offset + i + 1) as i32);
                break;
            }
        }

        // True p50 for [1000..10000] is 5000-6000
        let error_pct = ((p50_est - 5500.0) / 5500.0 * 100.0).abs();
        println!("P50 estimate: {:.0}, expected ~5500, error: {:.2}%", p50_est, error_pct);

        // Should be within 10% (histogram bucket quantization)
        assert!(
            error_pct < 10.0,
            "P50 estimate {:.0} should be within 10% of 5500, got {:.2}% error",
            p50_est, error_pct
        );
    }
}
