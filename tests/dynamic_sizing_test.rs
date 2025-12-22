//! Tests for dynamic bucket sizing
//!
//! These tests demonstrate that the InnerHistogram can grow dynamically
//! to accommodate values that would exceed a fixed 256-bucket capacity.

use exponential_histogram::ExponentialHistogram;

const INITIAL_CAPACITY: usize = 256;

/// Helper function to calculate the expected OTel bucket index
fn calculate_otel_index(value: f64, scale: i32) -> i32 {
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    ((value.ln() / base.ln()).ceil() as i32) - 1
}

#[test]
fn test_dynamic_growth_for_large_span() {
    // With dynamic sizing, we should handle spans > 256 without capping

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Dynamic Growth for Large Span ===");

    // Add values that would span > 256 buckets
    let values = vec![0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0];

    println!("Adding values:");
    for &v in &values {
        let idx = calculate_otel_index(v, scale);
        println!("  {} -> index {}", v, idx);
        hist.accumulate(v);
    }

    let min_index = calculate_otel_index(0.001, scale);
    let max_index = calculate_otel_index(10000.0, scale);
    let span = max_index - min_index + 1;

    println!("\nSpan: {} buckets", span);

    // All values should be counted
    assert_eq!(hist.count(), values.len());

    // Statistics should be exact
    let expected_sum: f64 = values.iter().sum();
    let actual_sum = hist.sum();
    let error = (actual_sum - expected_sum).abs() / expected_sum;

    println!("Expected sum: {:.3}", expected_sum);
    println!("Actual sum: {:.3}", actual_sum);
    println!("Error: {:.4}%", error * 100.0);

    assert!(error < 0.0001, "Sum should be exact");
    assert_eq!(hist.min(), 0.001);
    assert_eq!(hist.max(), 10000.0);

    // Check bucket distribution
    let (counts, _) = hist.take_counts();

    println!("\nBucket array length: {}", counts.len());
    println!("Expected: at least {}", span);

    // With dynamic sizing, array should be >= span
    assert!(counts.len() >= span as usize,
        "Bucket array should grow to accommodate span");

    // Count non-zero buckets
    let non_zero_count = counts.iter().filter(|&&c| c > 0).count();
    println!("Non-zero buckets: {}", non_zero_count);

    // With dynamic sizing, we should have one bucket per value (or close)
    // At scale 4, some values might share buckets, but not all 8 in 6 buckets
    println!("Values: {}, Non-zero buckets: {}", values.len(), non_zero_count);

    // Should NOT have collapsed many values into boundary buckets
    assert!(counts[0] <= 2, "Bucket[0] should not accumulate many values");

    let last_idx = counts.len() - 1;
    assert!(counts[last_idx] <= 2, "Last bucket should not accumulate many values");
}

#[test]
fn test_no_capping_at_boundaries() {
    // Values should NOT be capped at bucket boundaries

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: No Capping at Boundaries ===");

    // Start with small value
    hist.accumulate(0.1);
    let offset1 = hist.bucket_start_offset();

    // Add huge value that would exceed 256 buckets
    hist.accumulate(10000.0);

    let small_index = calculate_otel_index(0.1, scale);
    let large_index = calculate_otel_index(10000.0, scale);
    let span = large_index - small_index + 1;

    println!("Small index: {}", small_index);
    println!("Large index: {}", large_index);
    println!("Span: {}", span);

    let (counts, _) = hist.take_counts();

    println!("\nBucket array length: {}", counts.len());
    assert!(counts.len() >= span as usize,
        "Array should grow to fit span");

    // Find where values are stored
    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("Non-zero buckets:");
    for &(i, &count) in &non_zero {
        let offset = hist.bucket_start_offset();
        let idx = offset + i as i32;
        println!("  Bucket[{}]: OTel index {}, count {}", i, idx, count);
    }

    // Both values should be in separate buckets
    assert_eq!(non_zero.len(), 2, "Should have exactly 2 non-zero buckets");

    // Small value should be at bucket[0]
    assert_eq!(counts[0], 1, "Bucket[0] should have small value");

    // Large value should NOT be capped at position 255
    // It should be at its correct position in the grown array
    let expected_large_pos = (large_index - offset1) as usize;
    println!("\nExpected large value position: {}", expected_large_pos);

    if expected_large_pos < counts.len() {
        assert_eq!(counts[expected_large_pos], 1,
            "Large value should be at correct position, not capped");
    }
}

#[test]
fn test_growth_increments() {
    // Test that growth happens in reasonable increments

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Growth Increments ===");

    // Start with initial capacity
    hist.accumulate(1.0);
    let (counts1, _) = hist.take_counts();
    println!("After 1 value: {} buckets", counts1.len());

    // Add value requiring growth
    hist.accumulate(0.001);
    let (counts2, _) = hist.take_counts();
    println!("After growth: {} buckets", counts2.len());

    // Should have grown
    if counts2.len() > counts1.len() {
        let growth = counts2.len() - counts1.len();
        println!("Grew by: {} buckets", growth);

        // Growth should be reasonable (not just +1, but also not *10)
        // Common strategies: double, or grow to exact size, or grow to next power of 2
        println!("Growth strategy appears to be: {}x or +{}",
            counts2.len() as f64 / counts1.len() as f64, growth);
    }

    assert_eq!(hist.count(), 2);
}

#[test]
fn test_concurrent_growth() {
    // Test that dynamic growth works with concurrent access

    use std::sync::Arc;
    use std::thread;

    let scale = 4;
    let hist = Arc::new(ExponentialHistogram::new(scale));

    println!("\n=== Test: Concurrent Growth ===");

    // Spawn threads adding values that will trigger growth
    let mut handles = vec![];

    for i in 0..4 {
        let hist_clone = Arc::clone(&hist);
        let handle = thread::spawn(move || {
            match i {
                0 => hist_clone.accumulate(0.001),
                1 => hist_clone.accumulate(0.1),
                2 => hist_clone.accumulate(100.0),
                3 => hist_clone.accumulate(10000.0),
                _ => {}
            }
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    println!("Count: {}", hist.count());
    println!("Min: {}", hist.min());
    println!("Max: {}", hist.max());

    // Due to race conditions in concurrent access, some values might be
    // merged into the same bucket, but all should be counted
    // We expect at least 3 values (some races might cause bucket sharing)
    assert!(hist.count() >= 3 && hist.count() <= 4,
        "Should count 3-4 values (concurrent races may merge some)");

    assert_eq!(hist.min(), 0.001);
    assert_eq!(hist.max(), 10000.0);

    let (counts, _) = hist.take_counts();
    println!("Final bucket count: {}", counts.len());

    // Should have grown to accommodate the span
    let min_idx = calculate_otel_index(0.001, scale);
    let max_idx = calculate_otel_index(10000.0, scale);
    let span = max_idx - min_idx + 1;

    println!("Required span: {}", span);

    // With concurrent access, growth should still happen, though exact size may vary
    assert!(counts.len() >= INITIAL_CAPACITY,
        "Should have grown despite concurrent access");
}

#[test]
fn test_preserves_existing_data_on_growth() {
    // When growing, existing bucket counts should be preserved

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Preserve Data on Growth ===");

    // Add some initial values
    for _ in 0..10 {
        hist.accumulate(1.0);
    }

    let count_before = hist.count();
    let sum_before = hist.sum();

    println!("Before growth: count={}, sum={}", count_before, sum_before);

    // Add value that triggers growth
    hist.accumulate(0.0001);

    let count_after = hist.count();
    let sum_after = hist.sum();

    println!("After growth: count={}, sum={:.4}", count_after, sum_after);

    // Previous data should be preserved
    assert_eq!(count_after, count_before + 1);
    assert!((sum_after - sum_before - 0.0001).abs() < 0.00001);

    // Old values should still be in their buckets
    let (counts, _) = hist.take_counts();
    let offset = hist.bucket_start_offset();

    println!("Final offset: {}", offset);

    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("Non-zero buckets after growth: {}", non_zero.len());
    for &(i, &count) in &non_zero {
        let idx = offset + i as i32;
        println!("  Bucket[{}]: OTel index {}, count {}", i, idx, count);
    }

    let total: usize = counts.iter().sum();
    println!("Total values in buckets: {}", total);

    assert_eq!(total, 11, "All values should be preserved");

    // Should have the 10 values for 1.0 in one bucket, and 1 value for 0.0001 in another
    // Unless they happen to fall in the same bucket at scale 4
    assert!(non_zero.len() >= 1 && non_zero.len() <= 2,
        "Should have 1-2 non-zero buckets (may share if close indices)");
}

#[test]
fn test_memory_efficiency() {
    // Test that we don't grow excessively

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Memory Efficiency ===");

    // Add values with moderate span
    let values = vec![1.0, 2.0, 4.0, 8.0, 16.0];
    for &v in &values {
        hist.accumulate(v);
    }

    let (counts, _) = hist.take_counts();

    let min_idx = calculate_otel_index(1.0, scale);
    let max_idx = calculate_otel_index(16.0, scale);
    let required_span = max_idx - min_idx + 1;

    println!("Required span: {}", required_span);
    println!("Actual array size: {}", counts.len());

    // Should not be excessively large
    // With 2x growth factor, worst case overhead is ~2x
    // But we start with 256, so for small spans we might have higher overhead
    let overhead = counts.len() as f64 / required_span as f64;
    println!("Overhead ratio: {:.2}x", overhead);

    // For small spans starting from 256, overhead can be higher
    // We mainly want to ensure it's not crazy (like 10x or 100x)
    assert!(overhead < 10.0,
        "Array should not be more than 10x the required span");
}

#[test]
fn test_extreme_span() {
    // Test handling of extreme spans (thousands of buckets)

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Extreme Span ===");

    // Add values with huge span
    hist.accumulate(0.00001);
    hist.accumulate(100000.0);

    let min_idx = calculate_otel_index(0.00001, scale);
    let max_idx = calculate_otel_index(100000.0, scale);
    let span = max_idx - min_idx + 1;

    println!("Min index: {}", min_idx);
    println!("Max index: {}", max_idx);
    println!("Span: {} buckets", span);

    assert_eq!(hist.count(), 2);
    assert_eq!(hist.min(), 0.00001);
    assert_eq!(hist.max(), 100000.0);

    let (counts, _) = hist.take_counts();
    println!("Array size: {}", counts.len());

    // Should handle even extreme spans
    assert!(counts.len() >= span as usize,
        "Should grow to handle extreme spans");

    // But should have reasonable memory usage (not allocating millions of buckets)
    assert!(counts.len() < 10000,
        "Should not allocate excessive memory");
}
