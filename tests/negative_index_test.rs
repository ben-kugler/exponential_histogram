/// Test to verify that the implementation correctly handles negative OTel bucket indices
///
/// In OpenTelemetry exponential histograms, bucket indices can be negative for values < 1.0.
/// For example, with scale=4:
/// - Value 0.5 -> negative index
/// - Value 0.25 -> more negative index
/// - Value 1.0 -> index 0
/// - Value 2.0 -> positive index
///
/// This test verifies that the InnerHistogram correctly handles the offset-based storage
/// for negative indices.

use exponential_histogram::ExponentialHistogram;

#[test]
fn test_values_less_than_one_produce_negative_indices() {
    // OTel formula: index = ceil(log(value) / log(base)) - 1
    // where base = 2^(2^(-scale))

    let scale = 4_i32;
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));

    println!("\n=== Testing Negative Index Handling ===");
    println!("Scale: {}", scale);
    println!("Base: {:.6}", base);

    // Calculate indices for various values
    let test_values = vec![0.1_f64, 0.25_f64, 0.5_f64, 0.75_f64, 1.0_f64, 2.0_f64, 4.0_f64];

    println!("\nExpected OTel indices:");
    for &v in &test_values {
        let index = ((v.ln() / base.ln()).ceil() as i32) - 1;
        println!("  Value {:.2} -> index {}", v, index);
    }

    // Now test with the actual implementation
    let hist = ExponentialHistogram::new(scale);

    for &v in &test_values {
        hist.accumulate(v);
    }

    println!("\nHistogram after accumulating values:");
    println!("  Count: {}", hist.count());
    println!("  Sum: {:.2}", hist.sum());
    println!("  Min: {:.2}", hist.min());
    println!("  Max: {:.2}", hist.max());

    assert_eq!(hist.count(), test_values.len());

    // Get bucket data
    let (positive_counts, _) = hist.take_counts();
    let offset = hist.bucket_start_offset();

    println!("\nBucket storage:");
    println!("  Offset (min index): {}", offset);
    println!("  Number of buckets: {}", positive_counts.len());

    // The offset should be negative if we have values < 1.0
    // because those values map to negative OTel indices
    println!("\nKey Question: Does offset handle negative indices?");

    // For value 0.1 at scale 4:
    // index = ceil(ln(0.1) / ln(base)) - 1
    let index_for_0_1 = ((0.1_f64.ln() / base.ln()).ceil() as i32) - 1;
    println!("  Index for 0.1: {}", index_for_0_1);
    println!("  Offset: {}", offset);

    if index_for_0_1 < 0 {
        println!("  ✓ Index IS negative");
        println!("  Expected: Implementation should store this at offset");

        // The offset should equal the most negative index
        // But since offset is returned as usize, check if it handles negative correctly
        println!("\n  Issue: offset is usize ({}), but OTel index is i32 ({})",
                 offset, index_for_0_1);
        println!("  This is a problem! We can't represent negative indices with usize.");
    } else {
        println!("  Index is NOT negative (test values may need adjustment)");
    }
}

#[test]
fn test_negative_index_storage_with_inner_histogram() {
    // Direct test of negative index handling in InnerHistogram
    // The InnerHistogram uses offset to map i32 indices to array positions

    println!("\n=== Direct InnerHistogram Negative Index Test ===");

    let scale = 8_i32; // Higher scale for finer buckets
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));

    // These values should produce negative indices
    let small_values = vec![0.001_f64, 0.01_f64, 0.1_f64, 0.5_f64];

    for &v in &small_values {
        let index = ((v.ln() / base.ln()).ceil() as i32) - 1;
        println!("Value {:.3} -> OTel index {}", v, index);
    }

    let hist = ExponentialHistogram::new(scale);

    // Add the small values
    for &v in &small_values {
        hist.accumulate(v);
    }

    // Also add some values > 1.0 for comparison
    hist.accumulate(1.0);
    hist.accumulate(10.0);

    println!("\nHistogram stats:");
    println!("  Count: {}", hist.count());
    println!("  Min: {:.3}", hist.min());
    println!("  Max: {:.3}", hist.max());

    let (positive_counts, _) = hist.take_counts();
    let offset = hist.bucket_start_offset();

    println!("  Bucket offset: {}", offset);
    println!("  Bucket count: {}", positive_counts.len());

    // Check if we can distinguish between the small values
    let non_zero_buckets: Vec<_> = positive_counts.iter()
        .enumerate()
        .filter(|&(_, count)| *count > 0)
        .collect();

    println!("\nNon-zero buckets: {}", non_zero_buckets.len());
    for &(i, count) in &non_zero_buckets {
        let otel_index = offset as i32 + i as i32;
        println!("  Bucket {} (OTel index {}): count = {}", i, otel_index, count);
    }

    assert_eq!(hist.count(), small_values.len() + 2);
}

#[test]
fn test_bucket_start_offset_type_issue() {
    // This test documents a potential bug: bucket_start_offset() returns usize
    // but OTel bucket indices are i32 and can be negative

    println!("\n=== Bucket Offset Type Issue ===");

    let scale = 10_i32;
    let hist = ExponentialHistogram::new(scale);

    // Add a very small value that should create a large negative index
    hist.accumulate(0.0001);

    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    let expected_index = ((0.0001_f64.ln() / base.ln()).ceil() as i32) - 1;

    println!("Value: 0.0001");
    println!("Expected OTel index: {}", expected_index);
    println!("Is negative? {}", expected_index < 0);

    let offset = hist.bucket_start_offset();
    println!("bucket_start_offset() returns: {} (type: usize)", offset);

    if expected_index < 0 {
        println!("\n⚠️  PROBLEM DETECTED:");
        println!("  OTel index is negative ({}) but offset is usize", expected_index);
        println!("  The usize type cannot represent negative numbers!");
        println!("  This means:");
        println!("    - Either the offset is wrong (cast from negative i32 wraps)");
        println!("    - Or negative indices are not being stored correctly");
        println!("    - Or bucket_start_offset() should return i32, not usize");
    }

    // Try to get the bucket counts
    let (positive_counts, _) = hist.take_counts();
    println!("\nBucket counts length: {}", positive_counts.len());

    if !positive_counts.is_empty() {
        println!("First bucket count: {}", positive_counts[0]);
        assert_eq!(positive_counts.iter().sum::<usize>(), 1, "Should have exactly 1 value");
    }
}

#[test]
fn test_negative_index_with_offset_mapping() {
    // Test the actual offset-based index mapping in InnerHistogram
    // According to the code, offset stores the minimum OTel index
    // and maps it to array position 0

    println!("\n=== Offset Mapping for Negative Indices ===");

    let scale = 4_i32;
    let hist = ExponentialHistogram::new(scale);

    let base = 2.0_f64.powf(2.0_f64.powi(-scale));

    // Add values in order from smallest to largest
    let values = vec![0.5_f64, 1.0_f64, 2.0_f64, 4.0_f64];

    println!("Adding values and their expected OTel indices:");
    for &v in &values {
        let index = ((v.ln() / base.ln()).ceil() as i32) - 1;
        println!("  {:.1} -> index {}", v, index);
        hist.accumulate(v);
    }

    let offset = hist.bucket_start_offset();
    let (counts, _) = hist.take_counts();

    println!("\nResult:");
    println!("  Offset: {}", offset);
    println!("  Bucket array length: {}", counts.len());

    // The offset should be the minimum index
    let min_expected_index = ((0.5_f64.ln() / base.ln()).ceil() as i32) - 1;
    println!("  Expected min index: {}", min_expected_index);

    if min_expected_index < 0 {
        println!("\n  ⚠️  Min index is negative!");
        println!("  Question: How is negative {} represented as usize {}?",
                 min_expected_index, offset);

        // If this wraps around, offset would be a huge number
        if offset > 1_000_000 {
            println!("  → Offset is HUGE - likely wrapping from negative i32!");
            assert!(false, "Negative i32 wrapped to huge usize - type mismatch bug!");
        }
    }

    // Print bucket contents
    println!("\nBucket contents:");
    for (i, &count) in counts.iter().enumerate() {
        if count > 0 {
            println!("  Array position {}: count = {}", i, count);
        }
    }
}

#[test]
fn test_negative_bucket_start_offset_method() {
    // Test negative_bucket_start_offset() for negative values

    println!("\n=== Negative Bucket Offset Test ===");

    let scale = 4_i32;
    let hist = ExponentialHistogram::new(scale);

    // Add negative values (which use negative_buckets histogram)
    hist.accumulate(-0.5);
    hist.accumulate(-2.0);

    println!("Added negative values: -0.5, -2.0");
    println!("Count: {}", hist.count());
    println!("Has negatives: {}", hist.has_negatives());

    let neg_offset = hist.negative_bucket_start_offset();
    println!("negative_bucket_start_offset(): {}", neg_offset);

    // For negative values, we use their absolute value for indexing
    // So -0.5 uses abs(-0.5) = 0.5 for the index calculation
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    let index_for_0_5 = ((0.5_f64.ln() / base.ln()).ceil() as i32) - 1;

    println!("Expected index for abs(-0.5): {}", index_for_0_5);

    if index_for_0_5 < 0 {
        println!("⚠️  Index is negative but offset is usize");
    }
}
