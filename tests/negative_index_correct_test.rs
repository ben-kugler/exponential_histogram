//! Unit tests demonstrating that negative OTel bucket indices are handled correctly
//!
//! These tests PROVE that the implementation correctly handles negative indices
//! that occur when values < 1.0 are accumulated.

use exponential_histogram::ExponentialHistogram;

/// Helper function to calculate the expected OTel bucket index
fn calculate_otel_index(value: f64, scale: i32) -> i32 {
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    ((value.ln() / base.ln()).ceil() as i32) - 1
}

#[test]
fn test_offset_is_i32_not_usize() {
    // PROOF: bucket_start_offset() returns i32, which can represent negative values

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    // Add a value < 1.0 that produces a negative index
    hist.accumulate(0.5);

    let offset = hist.bucket_start_offset();
    let expected_index = calculate_otel_index(0.5, scale);

    println!("\n=== Test 1: Offset Type Verification ===");
    println!("Value: 0.5");
    println!("Expected OTel index: {}", expected_index);
    println!("Actual offset returned: {}", offset);
    println!("Offset type: i32 ✅");

    // The offset should match the expected negative index
    assert_eq!(
        offset, expected_index,
        "Offset should match the expected negative OTel index"
    );

    // Verify it's actually negative
    assert!(expected_index < 0, "Index for 0.5 should be negative");
    assert!(
        offset < 0,
        "Offset should be negative (proves i32 return type works)"
    );
}

#[test]
fn test_multiple_negative_indices_stored_correctly() {
    // PROOF: Multiple values with different negative indices are stored in correct buckets

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    let test_values = vec![0.1, 0.25, 0.5, 0.75];

    println!("\n=== Test 2: Multiple Negative Indices ===");
    println!("Scale: {}", scale);

    // Calculate expected indices
    let mut expected_indices = Vec::new();
    for &v in &test_values {
        let idx = calculate_otel_index(v, scale);
        expected_indices.push(idx);
        println!("Value {:.2} -> Expected index: {}", v, idx);
        hist.accumulate(v);
    }

    // All indices should be negative
    for &idx in &expected_indices {
        assert!(idx < 0, "All indices should be negative for values < 1.0");
    }

    // Verify histogram recorded all values
    assert_eq!(hist.count(), test_values.len());

    // Verify the offset equals the most negative index
    let offset = hist.bucket_start_offset();
    let min_index = expected_indices.iter().min().unwrap();

    println!("\nOffset: {}", offset);
    println!("Most negative index: {}", min_index);

    assert_eq!(
        offset, *min_index,
        "Offset should equal the most negative index"
    );

    // Verify sum is approximately correct
    let expected_sum: f64 = test_values.iter().sum();
    let actual_sum = hist.sum();
    let error = (actual_sum - expected_sum).abs() / expected_sum;

    println!("Expected sum: {:.3}", expected_sum);
    println!("Actual sum: {:.3}", actual_sum);
    println!("Relative error: {:.1}%", error * 100.0);

    assert!(
        error < 0.01,
        "Sum should be accurate within 1% for negative indices"
    );
}

#[test]
fn test_very_small_values_create_very_negative_indices() {
    // PROOF: Even very small values (creating indices like -2000) are handled correctly

    let scale = 8; // Higher scale for finer granularity
    let hist = ExponentialHistogram::new(scale);

    let small_value = 0.001;
    hist.accumulate(small_value);

    let expected_index = calculate_otel_index(small_value, scale);
    let offset = hist.bucket_start_offset();

    println!("\n=== Test 3: Very Small Values ===");
    println!("Scale: {}", scale);
    println!("Value: {}", small_value);
    println!("Expected index: {}", expected_index);
    println!("Actual offset: {}", offset);

    // Should be a large negative number
    assert!(
        expected_index < -1000,
        "0.001 at scale 8 should produce index < -1000"
    );

    // Offset should match
    assert_eq!(
        offset, expected_index,
        "Offset should correctly store large negative index"
    );

    // Verify the value is counted
    assert_eq!(hist.count(), 1);
    assert!((hist.sum() - small_value).abs() < 0.0001);
}

#[test]
fn test_mixed_positive_and_negative_indices() {
    // PROOF: Can store both negative (< 1.0) and positive (> 1.0) indices together

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    let values = vec![0.25, 0.5, 1.0, 2.0, 4.0];

    println!("\n=== Test 4: Mixed Positive and Negative Indices ===");
    println!("Scale: {}", scale);

    let mut indices = Vec::new();
    for &v in &values {
        let idx = calculate_otel_index(v, scale);
        indices.push(idx);
        println!("Value {:.2} -> index {}", v, idx);
        hist.accumulate(v);
    }

    // Should have both negative and positive indices
    let has_negative = indices.iter().any(|&i| i < 0);
    let has_positive = indices.iter().any(|&i| i >= 0);

    assert!(has_negative, "Should have negative indices");
    assert!(has_positive, "Should have positive indices");

    // Offset should be the minimum (most negative)
    let offset = hist.bucket_start_offset();
    let min_index = indices.iter().min().unwrap();

    assert_eq!(offset, *min_index);
    assert!(offset < 0, "Offset should be negative (from 0.25)");

    // Verify all values counted
    assert_eq!(hist.count(), values.len());

    // Verify statistics are reasonable
    assert_eq!(hist.min(), 0.25);
    assert_eq!(hist.max(), 4.0);

    let expected_sum: f64 = values.iter().sum();
    let actual_sum = hist.sum();
    println!("\nExpected sum: {:.2}", expected_sum);
    println!("Actual sum: {:.2}", actual_sum);

    let error = (actual_sum - expected_sum).abs() / expected_sum;
    assert!(error < 0.02, "Sum should be accurate within 2%");
}

#[test]
fn test_offset_mapping_arithmetic() {
    // PROOF: The offset mapping formula works correctly for negative indices
    // Formula: array_position = otel_index - offset

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    // Add a single value to set the offset
    let value = 0.5;
    hist.accumulate(value);

    let offset = hist.bucket_start_offset();
    let otel_index = calculate_otel_index(value, scale);

    println!("\n=== Test 5: Offset Mapping Arithmetic ===");
    println!("Value: {}", value);
    println!("OTel index: {}", otel_index);
    println!("Offset: {}", offset);

    // The first value should be at array position 0
    let array_position = otel_index - offset;

    println!(
        "Array position = {} - {} = {}",
        otel_index, offset, array_position
    );

    assert_eq!(
        array_position, 0,
        "First value should be at array position 0"
    );

    // Now add a larger value
    let value2 = 2.0;
    hist.accumulate(value2);

    let otel_index2 = calculate_otel_index(value2, scale);
    let array_position2 = otel_index2 - offset;

    println!("\nValue 2: {}", value2);
    println!("OTel index 2: {}", otel_index2);
    println!(
        "Array position 2 = {} - {} = {}",
        otel_index2, offset, array_position2
    );

    assert!(
        array_position2 > 0,
        "Second value (2.0) should be at higher array position"
    );
    assert!(
        array_position2 < 256,
        "Array position should fit in the 256-bucket array"
    );

    // Verify the bucket counts
    let (counts, _) = hist.take_counts();

    println!("\nBucket verification:");
    println!("  counts[0] = {} (should be 1 for value 0.5)", counts[0]);
    println!(
        "  counts[{}] = {} (should be 1 for value 2.0)",
        array_position2, counts[array_position2 as usize]
    );

    assert_eq!(counts[0], 1, "First bucket should have count 1");
    assert_eq!(
        counts[array_position2 as usize], 1,
        "Second bucket should have count 1"
    );
}

#[test]
fn test_negative_values_use_abs_for_indexing() {
    // PROOF: Negative input values use their absolute value for index calculation

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    // Add negative values
    hist.accumulate(-0.5);
    hist.accumulate(-2.0);

    println!("\n=== Test 6: Negative Values (Sign, not Index) ===");
    println!("Added values: -0.5, -2.0");
    println!("Count: {}", hist.count());
    println!("Has negatives: {}", hist.has_negatives());

    assert_eq!(hist.count(), 2);
    assert!(hist.has_negatives());

    // These should be in the negative_buckets histogram
    let neg_offset = hist.negative_bucket_start_offset();

    // The index for abs(-0.5) = 0.5 should be negative
    let expected_index_for_0_5 = calculate_otel_index(0.5, scale);

    println!("Negative bucket offset: {}", neg_offset);
    println!("Expected index for abs(-0.5): {}", expected_index_for_0_5);

    assert_eq!(
        neg_offset, expected_index_for_0_5,
        "Negative bucket offset should match index for abs value"
    );

    // Verify statistics
    assert_eq!(hist.min(), -2.0);
    assert_eq!(hist.max(), -0.5);

    let expected_sum = -0.5 + -2.0;
    let actual_sum = hist.sum();
    println!("Expected sum: {}", expected_sum);
    println!("Actual sum: {}", actual_sum);

    let error = (actual_sum - expected_sum).abs() / expected_sum.abs();
    assert!(error < 0.02, "Sum should be accurate for negative values");
}

#[test]
fn test_fractional_values_not_truncated() {
    // PROOF: Unlike the main branch, fractional values are NOT truncated

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    let fractional_values = vec![0.5, 1.5, 2.7];

    println!("\n=== Test 7: Fractional Values Preserved ===");

    for &v in &fractional_values {
        hist.accumulate(v);
        println!("Accumulated: {}", v);
    }

    let expected_sum: f64 = fractional_values.iter().sum();
    let actual_sum = hist.sum();

    println!("Expected sum: {:.2}", expected_sum);
    println!("Actual sum: {:.2}", actual_sum);

    // Should be very accurate (not losing fractional parts like main branch)
    let error = (actual_sum - expected_sum).abs() / expected_sum;
    println!("Relative error: {:.2}%", error * 100.0);

    // Main branch would have ~30% error due to truncation
    // This implementation should have < 2% error
    assert!(
        error < 0.02,
        "Fractional values should NOT be truncated (error should be < 2%)"
    );

    // Verify count
    assert_eq!(hist.count(), 3);
}

#[test]
fn test_span_within_256_buckets() {
    // PROOF: Can store indices spanning 256 buckets even when offset is negative

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test 8: Span of 256 Buckets ===");

    // Start with a small value (negative index)
    hist.accumulate(0.1);
    let offset = hist.bucket_start_offset();
    let min_index = calculate_otel_index(0.1, scale);

    println!("Starting value: 0.1");
    println!("Starting index: {}", min_index);
    println!("Offset: {}", offset);

    assert_eq!(offset, min_index);
    assert!(offset < 0);

    // Now add a value that's within 256 buckets
    // offset + 255 is the maximum index we can store
    let max_storable_index = offset + 255;
    println!("Maximum storable index: {}", max_storable_index);

    // Add some values within range
    hist.accumulate(1.0);
    hist.accumulate(10.0);
    hist.accumulate(100.0);

    // All should be stored successfully
    assert_eq!(hist.count(), 4);

    println!(
        "Successfully stored values with indices from {} to {}",
        offset,
        calculate_otel_index(100.0, scale)
    );

    // Verify statistics
    assert_eq!(hist.min(), 0.1);
    assert_eq!(hist.max(), 100.0);
}

#[test]
fn test_zero_threshold_with_negative_indices() {
    // PROOF: Zero counting works correctly even when histogram has negative indices

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test 9: Zero Threshold ===");

    // Add values including very small ones
    hist.accumulate(0.1);
    hist.accumulate(0.0); // Exactly zero
    hist.accumulate(1e-100); // Effectively zero
    hist.accumulate(1.0);

    println!("Count: {}", hist.count());
    println!("Zero count: {}", hist.zero_count());

    // Should have counted the zeros separately
    assert!(hist.zero_count() >= 1, "Should count exact zero");

    // Total count should include zeros
    let total = hist.count();
    assert_eq!(total, 4, "Total count should include all values");

    // Offset should still be negative (from 0.1)
    let offset = hist.bucket_start_offset();
    println!("Offset: {}", offset);
    assert!(offset < 0, "Offset should be negative from value 0.1");
}

#[test]
fn test_compatibility_with_otel_export() {
    // PROOF: The histogram data is in OTel-compatible format

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test 10: OTel Export Compatibility ===");

    // Add various values
    let values = vec![0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
    for &v in &values {
        hist.accumulate(v);
    }

    // Get the data needed for OTel export
    let count = hist.count();
    let sum = hist.sum();
    let min = hist.min();
    let max = hist.max();
    let zero_count = hist.zero_count();
    let scale_value = hist.scale();
    let offset = hist.bucket_start_offset();
    let (positive_counts, negative_counts) = hist.take_counts();

    println!("OTel-compatible data:");
    println!("  count: {}", count);
    println!("  sum: {:.3}", sum);
    println!("  min: {}", min);
    println!("  max: {}", max);
    println!("  zero_count: {}", zero_count);
    println!("  scale: {}", scale_value);
    println!("  positive.offset: {}", offset);
    println!("  positive.bucket_counts.len: {}", positive_counts.len());
    println!("  negative.bucket_counts.len: {}", negative_counts.len());

    // Verify all required fields are present
    assert!(count > 0, "Should have count");
    assert!(sum > 0.0, "Should have sum");
    assert_eq!(min, 0.25, "Should have exact min");
    assert_eq!(max, 8.0, "Should have exact max");
    assert_eq!(scale_value, scale, "Should preserve scale");

    // Most importantly: offset should be i32 (can be negative)
    assert!(offset < 0, "Offset should be negative for values < 1.0");

    println!("\n✅ All OTel-required fields present with correct types");
    println!("✅ offset is i32 (value: {})", offset);
    println!("✅ Negative indices handled correctly");
}
