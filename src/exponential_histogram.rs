use std::{
    collections::VecDeque,
    sync::atomic::{AtomicI32, AtomicU64, Ordering},
};

use atomic_float::AtomicF64;

use crate::inner_histogram::InnerHistogram;

// Maximum scale supported (per OTel spec)
const MAX_SCALE: i32 = 20;

/// An exponential histogram that stores f64 values in OpenTelemetry proto like buckets
/// https://github.com/open-telemetry/opentelemetry-proto/blob/cfbf9357c03bf4ac150a3ab3bcbe4cc4ed087362/opentelemetry/proto/metrics/v1/metrics.proto#L466
// #[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExponentialHistogram {
    pub(crate) positive_buckets: InnerHistogram,
    pub(crate) negative_buckets: InnerHistogram,
    // zero_count is the count of values that are either exactly zero or
    // within the zero_threshold, which defaults to 0.0
    pub(crate) zero_count: AtomicU64,
    pub(crate) zero_threshold: AtomicF64,
    // scale describes the resolution of the histogram
    pub(crate) scale: AtomicI32,
    // running sum of values seen so far
    pub(crate) sum: AtomicF64,
    // min and max values seen so far
    pub(crate) min_value: AtomicF64,
    pub(crate) max_value: AtomicF64,
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
            zero_count: self.zero_count.load(Ordering::Relaxed).into(),
            zero_threshold: self.zero_threshold.load(Ordering::Relaxed).into(),
            scale: self.scale.load(Ordering::Relaxed).into(),
            sum: self.sum.load(Ordering::Relaxed).into(),
            min_value: self.min_value.load(Ordering::Relaxed).into(),
            max_value: self.max_value.load(Ordering::Relaxed).into(),
        }
    }
}

impl std::fmt::Display for ExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ExponentialHistogram(scale={}, count={}, sum={:.2}, min={:.2}, max={:.2})",
            self.scale.load(Ordering::Relaxed),
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
        let scale = desired_scale.clamp(0, MAX_SCALE);
        Self {
            positive_buckets: InnerHistogram::new(),
            negative_buckets: InnerHistogram::new(),
            zero_count: 0.into(),
            zero_threshold: 0.0.into(),
            scale: scale.into(),
            sum: 0.0.into(),
            min_value: f64::MAX.into(),
            max_value: f64::MIN.into(),
        }
    }

    pub fn with_zero_threshold(mut self, threshold: f64) -> Self {
        self.zero_threshold = threshold.into();
        self
    }

    pub fn reset(&mut self) {
        self.positive_buckets = InnerHistogram::new();
        self.negative_buckets = InnerHistogram::new();
        self.zero_count = 0.into();
        self.sum = 0.0.into();
        self.min_value = f64::MAX.into();
        self.max_value = f64::MIN.into();
    }

    #[inline(always)]
    pub fn accumulate<T: Into<f64>>(&self, value: T) {
        let val: f64 = value.into();

        if !val.is_finite() {
            return;
        }

        let zero_threshold = self.zero_threshold.load(Ordering::Relaxed);

        if zero_threshold == 0.0 && val == 0.0 {
            self.zero_count.fetch_add(1, Ordering::Relaxed);
            return;
        } else if val.abs() <= zero_threshold {
            self.zero_count.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.sum.fetch_add(val, Ordering::Relaxed);

        self.min_value.fetch_min(val, Ordering::Relaxed);

        self.max_value.fetch_max(val, Ordering::Relaxed);

        let abs_val = val.abs();

        let scale = self.scale.load(Ordering::Relaxed);

        if let Some(index) = value_to_otel_index(scale, abs_val) {
            if val >= 0.0 {
                self.positive_buckets.increment(index);
            } else {
                self.negative_buckets.increment(index);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn count(&self) -> usize {
        (self.positive_buckets.total_count()
            + self.negative_buckets.total_count()
            + self.zero_count.load(Ordering::Acquire)) as usize
    }

    pub fn sum(&self) -> f64 {
        self.sum.load(Ordering::Acquire)
    }

    pub fn min(&self) -> f64 {
        let min = self.min_value.load(Ordering::Acquire);
        if min == f64::MAX { 0.0 } else { min }
    }

    pub fn max(&self) -> f64 {
        let max = self.max_value.load(Ordering::Acquire);
        if max == f64::MIN { 0.0 } else { max }
    }

    pub fn scale(&self) -> i32 {
        self.scale.load(Ordering::Acquire)
    }

    pub fn zero_count(&self) -> u64 {
        self.zero_count.load(Ordering::Acquire)
    }

    pub fn bucket_start_offset(&self) -> i32 {
        self.positive_buckets.offset().unwrap_or(0)
    }

    pub fn negative_bucket_start_offset(&self) -> i32 {
        self.negative_buckets.offset().unwrap_or(0)
    }

    pub fn has_negatives(&self) -> bool {
        !self.negative_buckets.is_empty()
    }

    pub fn take_counts(&self) -> (VecDeque<usize>, VecDeque<usize>) {
        (
            self.positive_buckets.as_vec_deque(),
            self.negative_buckets.as_vec_deque(),
        )
    }
}

/// Convert a value to an OpenTelemetry exponential histogram bucket index.
/// https://opentelemetry.io/docs/specs/otel/metrics/data-model/#producer-expectations
#[inline(always)]
fn value_to_otel_index(scale: i32, value: f64) -> Option<i32> {
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

#[cfg(test)]
mod tests {
    use crate::exponential_histogram::value_to_otel_index;

    use super::ExponentialHistogram;

    #[test]
    fn check_range() {
        assert_eq!(Some(1275), value_to_otel_index(6, 1_000_000.0));
        assert_eq!(Some(1275 + 160), value_to_otel_index(6, 5_650_000.0));

        assert_eq!(Some(637), value_to_otel_index(5, 1_000_000.0));
        assert_eq!(Some(637 + 160), value_to_otel_index(5, 32_000_000.0));
    }

    #[test]
    fn indices_scale_downgrade_positive_numbers() {
        //
        // -------- Start out with a fine-grained histogram --------
        //
        let e = ExponentialHistogram::new(8);

        e.accumulate(24_000_000);
        assert_eq!(
            6276,
            e.bucket_start_offset(),
            "histogram initializes with the first observation in the middle of the range"
        );
        assert_eq_epsilon(24000000.0, e.min(), "min and max should be equal");
        assert_eq_epsilon(24000000.0, e.max(), "min and max should be equal");

        assert_eq!(
            8,
            e.scale(),
            "initial value should not change scale since it falls in the numeric range"
        );

        assert_eq!(
            1,
            e.positive_buckets.load(0),
            "initial value should go in index 0"
        );
        assert_eq!(
            6276,
            e.bucket_start_offset(),
            "bucket start offset should index into scale 8"
        );

        // assert some bucket boundaries for convenience
        // assert_value_lowerboundary(&e, 24_000_000, 23984931.775);
        // assert_value_lowerboundary(&e, 24_040_000, 23984931.775);
        // assert_value_lowerboundary(&e, 24_050_000, 24049961.522);

        // assert_eq_epsilon(
        //     19313750.368,
        //     lower_boundary(8, 0, 6196),
        //     "lower boundary of histogram",
        // );
        // assert_eq_epsilon(
        //     29785874.896,
        //     lower_boundary(8, 0, 6196 + 160),
        //     "upper boundary of histogram",
        // );

        // Accumulate some data in a bucket's range
        for i in 0..40_000 {
            e.accumulate(24_000_000 + i);
        }
        assert_eq!(
            40001,
            e.positive_buckets.load(0),
            "initial value should go in index 0"
        );

        e.accumulate(24_050_000);
        assert_eq!(
            8,
            e.scale(),
            "a value in the next higher bucket should not change the scale"
        );

        assert_eq!(1, e.positive_buckets.load(1), "index 1 has a new count");
        assert_eq!(
            6276,
            e.bucket_start_offset(),
            "bucket start offset does not change when adding a bucket in the same range"
        );

        // Poke at growth boundary conditions
        e.accumulate(23_984_000);
        assert_eq!(
            8,
            e.scale(),
            "a value in the next lower bucket should not change the scale"
        );

        assert_eq!(1, e.positive_buckets.load(0), "index 79 has a new count");
        assert_eq!(
            6276,
            e.bucket_start_offset(),
            "bucket start offset does not change when using a bucket in the same range"
        );

        e.accumulate(19_313_750);
        assert_eq!(
            8,
            e.scale(),
            "a value below the covered range should not change the scale yet because there is room above the observed range to shift"
        );
        assert_eq!(
            83,
            e.positive_buckets.len(),
            "bucket count should not increase when a new bucket value is observed within the covered range"
        );
        assert_eq!(1, e.positive_buckets.load(0), "index 0 has a new count");
        // assert_eq!(
        //     6195, e.bucket_start_offset,
        //     "bucket start offset changes because we rotated down 1 position"
        // );
        // assert_eq_epsilon(
        //     29705335.561,
        //     lower_boundary(8, 0, 6195 + 160),
        //     "new upper boundary of histogram",
        // );

        //
        // -------- Expand histogram range with a big number --------
        //
        e.accumulate(29_705_336);
        assert_eq!(
            Some(3177),
            value_to_otel_index(7, 29_705_336_f64),
            "this value pushes the length of scale 7 also"
        );
        assert_eq!(
            7,
            e.scale(),
            "a value above the covered range should now change the scale because the lower end is populated while the upper end is beyond the range this scale can cover in 160 buckets"
        );
        assert_eq!(
            160,
            e.positive_buckets.len(),
            "bucket count should be sensible after rescale"
        );
        assert_eq!(
            1,
            e.positive_buckets.load(e.positive_buckets.len() - 1),
            "last index has a new count"
        );
        assert_eq!(
            3018,
            e.bucket_start_offset(),
            "bucket start offset changes because we scaled and rotated"
        );

        //
        // -------- Skip several zoom scale steps in a single accumulate --------
        //
        let recursive_scale_start_count = e.count();
        // assert_eq!(
        //     2199023255551.996,
        //     lower_boundary(2, 0, 164),
        //     "this value gets us down into scale 2"
        // );
        // assert_eq_epsilon(
        //     4.000,
        //     lower_boundary(2, 0, 8),
        //     "this value gets us down into scale 2",
        // );
        // assert_eq_epsilon(
        //     4.757,
        //     lower_boundary(2, 0, 9),
        //     "this value gets us down into scale 2",
        // );
        // pin the bucket's low value, at scale 2's index 8. It's not in scale 2 yet though!
        e.accumulate(4.25);
        // now blow the range wide, way past scale 7, resulting in a recursive scale down from 7 to precision 2.
        e.accumulate(2_199_023_255_552_f64);
        assert_eq!(2, e.scale(), "this value range should force scale range 2");
        assert_eq!(
            8,
            e.bucket_start_offset(),
            "bucket start offset should match the first element, since we rotated and grew out to the larger value"
        );
        assert_eq!(
            1,
            e.positive_buckets.load(8 - 8),
            "this is the 4.0 bucket, and 4.25 should go in it."
        );
        assert_eq!(
            1,
            e.positive_buckets.load(164 - 8),
            "this is the bucket for the big numer."
        );
        assert_eq!(
            recursive_scale_start_count + 2,
            e.count(),
            "2 more reports were made. The histogram maintains every count across rescaling, even recursive rescaling"
        );
    }

    // /// Look for random index crashes
    // #[test]
    // fn fuzz() {
    //     let start = Instant::now();
    //     while start.elapsed() < Duration::from_millis(50) {
    //         let mut e = ExponentialHistogram::new(8);
    //         let start = Instant::now();
    //         while start.elapsed() < Duration::from_millis(1) {
    //             e.accumulate(1_000_000_000_000_000_f64 * rand::random::<f64>());
    //         }
    //     }
    // }

    // /// Look for random index crashes
    // #[test]
    // fn fuzz_negative() {
    //     let start = Instant::now();
    //     while start.elapsed() < Duration::from_millis(50) {
    //         let mut e = ExponentialHistogram::new(8);
    //         let start = Instant::now();
    //         while start.elapsed() < Duration::from_millis(1) {
    //             e.accumulate(-1_000_000_000_000_000_f64 * rand::random::<f64>());
    //         }
    //     }
    // }

    #[track_caller]
    fn assert_eq_epsilon(j: f64, k: f64, message: &str) {
        const EPSILON: f64 = 1.0 / 128.0;
        let difference = (j - k).abs();
        assert!(
            difference < EPSILON,
            "{message}: {j} != {k} with epsilon {EPSILON}."
        );
    }

    // #[test]
    // fn test_exponential_histogram_basic() {
    //     let hist = ExponentialHistogram::new(0);
    //     assert!(hist.is_empty());
    //     assert_eq!(hist.count(), 0);

    //     hist.accumulate(1.0);
    //     hist.accumulate(2.0);
    //     hist.accumulate(3.0);

    //     assert!(!hist.is_empty());
    //     assert_eq!(hist.count(), 3);
    //     assert_eq!(hist.sum(), 6.0);
    //     assert_eq!(hist.min(), 1.0);
    //     assert_eq!(hist.max(), 3.0);
    // }

    // #[test]
    // fn test_exponential_histogram_with_max_buckets() {
    //     let hist = ExponentialHistogram::new_with_max_buckets(2, 100);
    //     hist.accumulate(10.0);
    //     assert_eq!(hist.count(), 1);
    //     assert_eq!(hist.scale(), 2);
    // }

    // #[test]
    // fn test_exponential_histogram_reset() {
    //     let mut hist = ExponentialHistogram::new(0);
    //     hist.accumulate(5.0);
    //     hist.accumulate(10.0);

    //     assert_eq!(hist.count(), 2);

    //     hist.reset();

    //     assert!(hist.is_empty());
    //     assert_eq!(hist.count(), 0);
    // }

    // #[test]
    // fn test_exponential_histogram_negatives() {
    //     let hist = ExponentialHistogram::new(0);
    //     hist.accumulate(-5.0);
    //     hist.accumulate(10.0);
    //     hist.accumulate(-2.0);

    //     assert!(hist.has_negatives());
    //     assert_eq!(hist.count(), 3);
    //     assert_eq!(hist.min(), -5.0);
    //     assert_eq!(hist.max(), 10.0);
    //     assert_eq!(hist.sum(), 3.0);
    // }

    // #[test]
    // fn test_fractional_values() {
    //     let hist = ExponentialHistogram::new(4);
    //     hist.accumulate(0.1);
    //     hist.accumulate(0.5);
    //     hist.accumulate(1.5);
    //     hist.accumulate(2.7);

    //     assert_eq!(hist.count(), 4);
    //     assert!(
    //         (hist.sum() - 4.8).abs() < 1e-10,
    //         "sum should be approximately 4.8, got {}",
    //         hist.sum()
    //     );
    //     assert_eq!(hist.min(), 0.1);
    //     assert_eq!(hist.max(), 2.7);
    // }

    // #[test]
    // fn test_zero_handling() {
    //     let hist = ExponentialHistogram::new(0);
    //     hist.accumulate(0.0);
    //     hist.accumulate(0.0);
    //     hist.accumulate(1.0);

    //     assert_eq!(hist.count(), 3);
    //     assert_eq!(hist.zero_count(), 2);
    //     assert_eq!(hist.sum(), 1.0);
    // }

    // #[test]
    // fn test_shared_exponential_histogram() {
    //     let hist = SharedExponentialHistogram::default();

    //     hist.accumulate(1.0);
    //     hist.accumulate(2.0);
    //     hist.accumulate(3.0);

    //     let snapshot = hist.snapshot();
    //     assert_eq!(snapshot.count(), 3);
    //     assert_eq!(snapshot.sum(), 6.0);

    //     let snapshot2 = hist.snapshot();
    //     assert_eq!(snapshot2.count(), 3);
    // }

    // #[test]
    // fn test_shared_exponential_histogram_reset() {
    //     let hist = SharedExponentialHistogram::default();

    //     hist.accumulate(1.0);
    //     hist.accumulate(2.0);

    //     let snapshot = hist.snapshot_and_reset();
    //     assert_eq!(snapshot.count(), 2);

    //     let snapshot2 = hist.snapshot();
    //     assert_eq!(snapshot2.count(), 0);
    // }

    // #[test]
    // fn test_shared_exponential_histogram_thread_safety() {
    //     use std::sync::Arc;
    //     use std::thread;

    //     let hist = Arc::new(SharedExponentialHistogram::default());
    //     let mut handles = vec![];

    //     for i in 0..10 {
    //         let hist_clone = Arc::clone(&hist);
    //         let handle = thread::spawn(move || {
    //             for j in 0..100 {
    //                 hist_clone.accumulate((i * 100 + j) as f64);
    //             }
    //         });
    //         handles.push(handle);
    //     }

    //     for handle in handles {
    //         handle.join().unwrap();
    //     }

    //     let snapshot = hist.snapshot();
    //     assert_eq!(snapshot.count(), 1000);
    // }

    // #[test]
    // fn test_clone_implementation() {
    //     let hist = ExponentialHistogram::new(2);
    //     hist.accumulate(5.0);
    //     hist.accumulate(-3.0);
    //     hist.accumulate(10.0);

    //     let cloned = hist.clone();

    //     assert_eq!(hist.count(), cloned.count());
    //     assert_eq!(hist.scale(), cloned.scale());
    //     assert_eq!(hist.has_negatives(), cloned.has_negatives());
    //     assert_eq!(hist.min(), cloned.min());
    //     assert_eq!(hist.max(), cloned.max());
    // }

    // #[test]
    // fn test_display_implementation() {
    //     let hist = ExponentialHistogram::new(2);
    //     hist.accumulate(5.0);
    //     hist.accumulate(-3.0);
    //     hist.accumulate(10.0);

    //     let display_output = format!("{}", hist);
    //     assert!(display_output.contains("ExponentialHistogram"));
    //     assert!(display_output.contains("scale=2"));
    //     assert!(display_output.contains("count=3"));

    //     let empty_hist = ExponentialHistogram::new(0);
    //     let empty_output = format!("{}", empty_hist);
    //     assert!(empty_output.contains("count=0"));
    //     assert!(empty_output.contains("sum=0.00"));
    // }

    // #[test]
    // fn test_default_implementation() {
    //     let hist = ExponentialHistogram::default();
    //     assert_eq!(hist.scale(), 0);
    //     assert_eq!(hist.count(), 0);
    //     assert!(hist.is_empty());

    //     let shared = SharedExponentialHistogram::default();
    //     let snapshot = shared.snapshot();
    //     assert_eq!(snapshot.scale(), 0);
    //     assert_eq!(snapshot.count(), 0);
    //     assert!(snapshot.is_empty());

    //     let hist2 = ExponentialHistogram::default();
    //     hist2.accumulate(5.0);
    //     assert_eq!(hist2.count(), 1);

    //     let shared2 = SharedExponentialHistogram::default();
    //     shared2.accumulate(10.0);
    //     assert_eq!(shared2.snapshot().count(), 1);
    // }

    // #[test]
    // fn test_otel_index_conversion() {
    //     let scale = 7;
    //     let hist = ExponentialHistogram::new(scale);

    //     let test_values: Vec<u64> = vec![
    //         1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000, 5_000_000, 10_000_000,
    //     ];

    //     for &v in &test_values {
    //         hist.accumulate(v as f64);
    //     }

    //     let offset = hist.bucket_start_offset();
    //     let (positive_counts, negative_counts) = hist.take_counts();

    //     let total_count: usize = positive_counts.iter().sum();
    //     assert_eq!(total_count, test_values.len());
    //     assert!(negative_counts.is_empty());

    //     println!("Scale: {}", scale);
    //     println!("Offset: {}", offset);
    //     println!("Bucket counts: {:?}", positive_counts);
    // }
}
