//! Tests demonstrating behavior when bucket span exceeds 256 buckets
//!
//! The InnerHistogram has a fixed capacity of 256 buckets. What happens when
//! we try to store values whose OTel indices span more than 256 buckets?

use exponential_histogram::ExponentialHistogram;

/// Helper function to calculate the expected OTel bucket index
fn calculate_otel_index(value: f64, scale: i32) -> i32 {
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    ((value.ln() / base.ln()).ceil() as i32) - 1
}

#[test]
fn test_span_exactly_256_buckets() {
    // First, verify that exactly 256 buckets works fine

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Span Exactly 256 Buckets ===");

    // Start with a small value
    let min_value = 0.1;
    hist.accumulate(min_value);

    let min_index = calculate_otel_index(min_value, scale);
    let offset = hist.bucket_start_offset();

    println!("Min value: {}", min_value);
    println!("Min index: {}", min_index);
    println!("Offset: {}", offset);

    // The maximum index we can store is offset + 255
    let max_storable_index = offset + 255;
    println!("Max storable index: {}", max_storable_index);

    // Find a value that produces this index
    let base = 2.0_f64.powf(2.0_f64.powi(-scale));
    let max_value = base.powf((max_storable_index + 1) as f64);

    println!("Max storable value (approx): {:.2}", max_value);

    // Add a value close to the max
    hist.accumulate(max_value * 0.9);

    let actual_index = calculate_otel_index(max_value * 0.9, scale);
    println!("Actual index for {:.2}: {}", max_value * 0.9, actual_index);

    // Should be within bounds
    assert!(actual_index - offset < 256,
        "Should fit within 256 buckets");

    assert_eq!(hist.count(), 2, "Both values should be stored");

    println!("✅ Span of {} buckets fits within capacity", actual_index - offset + 1);
}

#[test]
fn test_span_exceeds_256_by_adding_large_value() {
    // What happens when we add a value that would exceed 256 buckets?

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Exceed 256 Buckets by Adding Large Value ===");

    // Start with a small value to set a negative offset
    let small_value = 0.1;
    hist.accumulate(small_value);

    let offset = hist.bucket_start_offset();
    let small_index = calculate_otel_index(small_value, scale);

    println!("Small value: {}", small_value);
    println!("Small index: {}", small_index);
    println!("Offset: {}", offset);

    // Add a huge value that's way beyond 256 buckets
    let huge_value = 10000.0;
    let huge_index = calculate_otel_index(huge_value, scale);

    println!("\nHuge value: {}", huge_value);
    println!("Huge index: {}", huge_index);
    println!("Span: {} - {} = {}", huge_index, offset, huge_index - offset);

    hist.accumulate(huge_value);

    // Check what happened
    let count = hist.count();
    let min = hist.min();
    let max = hist.max();
    let sum = hist.sum();

    println!("\nAfter adding huge value:");
    println!("  Count: {}", count);
    println!("  Min: {}", min);
    println!("  Max: {}", max);
    println!("  Sum: {:.2}", sum);

    // The value should still be counted
    assert_eq!(count, 2, "Both values should be counted");
    assert_eq!(min, small_value, "Min should be preserved");
    assert_eq!(max, huge_value, "Max should be updated");

    // But where did it go in the buckets?
    let (counts, _) = hist.take_counts();
    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("\nNon-zero buckets: {}", non_zero.len());
    for &(i, &count) in &non_zero {
        let index = offset + i as i32;
        println!("  Bucket {}: OTel index {}, count {}", i, index, count);
    }

    // Check if it was capped at the last bucket
    if counts[255] > 0 {
        println!("\n⚠️  Value was capped at bucket[255]");
        println!("  This bucket has count: {}", counts[255]);
    }
}

#[test]
fn test_span_exceeds_256_by_adding_small_value() {
    // What happens when we add a very small value after large values?

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Exceed 256 Buckets by Adding Small Value ===");

    // Start with a large value
    let large_value = 1000.0;
    hist.accumulate(large_value);

    let offset_before = hist.bucket_start_offset();
    let large_index = calculate_otel_index(large_value, scale);

    println!("Large value: {}", large_value);
    println!("Large index: {}", large_index);
    println!("Initial offset: {}", offset_before);

    // Now add a very small value that's more than 256 buckets away
    let tiny_value = 0.001;
    let tiny_index = calculate_otel_index(tiny_value, scale);

    println!("\nTiny value: {}", tiny_value);
    println!("Tiny index: {}", tiny_index);
    println!("Span: {} - {} = {}", large_index, tiny_index, large_index - tiny_index);

    hist.accumulate(tiny_value);

    // Check what happened to the offset
    let offset_after = hist.bucket_start_offset();
    println!("\nOffset after tiny value: {}", offset_after);

    if offset_after < offset_before {
        println!("✅ Offset was updated to accommodate smaller value");
        println!("  Old offset: {}", offset_before);
        println!("  New offset: {}", offset_after);
    }

    // Check statistics
    let count = hist.count();
    let min = hist.min();
    let max = hist.max();

    println!("\nAfter adding tiny value:");
    println!("  Count: {}", count);
    println!("  Min: {}", min);
    println!("  Max: {}", max);

    assert_eq!(count, 2, "Both values should be counted");
    assert_eq!(min, tiny_value, "Min should be updated");
    assert_eq!(max, large_value, "Max should be preserved");

    // Check bucket distribution
    let (counts, _) = hist.take_counts();
    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("\nNon-zero buckets: {}", non_zero.len());
    for &(i, &count) in &non_zero {
        let index = offset_after + i as i32;
        println!("  Bucket {}: OTel index {}, count {}", i, index, count);
    }
}

#[test]
fn test_many_values_exceeding_span() {
    // What happens when we add many values that collectively exceed 256 buckets?

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Many Values Exceeding 256 Bucket Span ===");

    // Add values spanning a huge range
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

    println!("\nTotal span: {} buckets", span);
    println!("Capacity: 256 buckets");
    println!("Overflow: {} buckets", span - 256);

    // Check results
    let count = hist.count();
    let min = hist.min();
    let max = hist.max();
    let sum = hist.sum();

    println!("\nHistogram statistics:");
    println!("  Count: {}", count);
    println!("  Min: {}", min);
    println!("  Max: {}", max);
    println!("  Sum: {:.2}", sum);

    // All values should be counted
    assert_eq!(count, values.len(), "All values should be counted");
    assert_eq!(min, 0.001, "Min should be correct");
    assert_eq!(max, 10000.0, "Max should be correct");

    // Check bucket utilization
    let (counts, _) = hist.take_counts();
    let non_zero_count = counts.iter().filter(|&&c| c > 0).count();
    let total_in_buckets: usize = counts.iter().sum();

    println!("\nBucket analysis:");
    println!("  Non-zero buckets: {}", non_zero_count);
    println!("  Total count in buckets: {}", total_in_buckets);
    println!("  Expected count: {}", values.len());

    if total_in_buckets == values.len() {
        println!("  ✅ All values stored in buckets");
    } else if total_in_buckets < values.len() {
        println!("  ⚠️  Some values lost: {} < {}", total_in_buckets, values.len());
    } else {
        println!("  ⚠️  Unexpected: more counts than values!");
    }

    // Check if buckets at boundaries are used
    println!("\nBoundary buckets:");
    println!("  Bucket[0]: {}", counts[0]);
    println!("  Bucket[255]: {}", counts[255]);
}

#[test]
fn test_offset_shift_behavior() {
    // Detailed test of how offset shifts when values exceed capacity

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Offset Shift Behavior ===");

    // Start with a value in the middle range
    let v1 = 1.0;
    hist.accumulate(v1);

    let offset1 = hist.bucket_start_offset();
    let idx1 = calculate_otel_index(v1, scale);

    println!("Step 1: Add {}", v1);
    println!("  Index: {}", idx1);
    println!("  Offset: {}", offset1);

    // Add a smaller value (should shift offset if needed)
    let v2 = 0.1;
    hist.accumulate(v2);

    let offset2 = hist.bucket_start_offset();
    let idx2 = calculate_otel_index(v2, scale);

    println!("\nStep 2: Add {}", v2);
    println!("  Index: {}", idx2);
    println!("  Offset: {}", offset2);

    if offset2 < offset1 {
        println!("  ✅ Offset shifted down: {} -> {}", offset1, offset2);
    }

    // Add a larger value
    let v3 = 100.0;
    hist.accumulate(v3);

    let offset3 = hist.bucket_start_offset();
    let idx3 = calculate_otel_index(v3, scale);

    println!("\nStep 3: Add {}", v3);
    println!("  Index: {}", idx3);
    println!("  Offset: {}", offset3);

    if offset3 == offset2 {
        println!("  ✅ Offset unchanged (value fit in existing range)");
    }

    // Now add a value that would exceed capacity
    let v4 = 0.0001;
    let idx4 = calculate_otel_index(v4, scale);

    println!("\nStep 4: Add {}", v4);
    println!("  Index: {}", idx4);
    println!("  Span to existing offset: {}", offset3 - idx4);

    hist.accumulate(v4);

    let offset4 = hist.bucket_start_offset();
    println!("  Offset after: {}", offset4);

    // Check final state
    println!("\nFinal state:");
    println!("  Count: {}", hist.count());
    println!("  Offset: {}", offset4);

    let (counts, _) = hist.take_counts();
    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("  Non-zero buckets: {}", non_zero.len());

    // Show where each value ended up
    println!("\nValue placement:");
    for &(i, count) in non_zero.iter() {
        let otel_idx = offset4 + i as i32;
        println!("  Bucket[{}]: OTel index {}, count {}", i, otel_idx, count);
    }
}

#[test]
fn test_precision_loss_with_overflow() {
    // Test if sum/statistics are still accurate when buckets overflow

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Precision with Bucket Overflow ===");

    // Add values that will exceed 256 bucket span
    let values = vec![0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0];

    println!("Adding values that exceed 256 bucket span:");
    for &v in &values {
        println!("  {}", v);
        hist.accumulate(v);
    }

    let expected_sum: f64 = values.iter().sum();
    let actual_sum = hist.sum();
    let expected_count = values.len();
    let actual_count = hist.count();

    println!("\nStatistics:");
    println!("  Expected count: {}", expected_count);
    println!("  Actual count: {}", actual_count);
    println!("  Expected sum: {:.3}", expected_sum);
    println!("  Actual sum: {:.3}", actual_sum);

    // Count should always be exact (tracked separately)
    assert_eq!(actual_count, expected_count,
        "Count should be exact even with bucket overflow");

    // Sum should also be exact (tracked separately as AtomicF64)
    let sum_error = (actual_sum - expected_sum).abs() / expected_sum;
    println!("  Sum relative error: {:.4}%", sum_error * 100.0);

    assert!(sum_error < 0.0001,
        "Sum should be exact (tracked separately, not from buckets)");

    // Min/max should be exact
    assert_eq!(hist.min(), 0.001);
    assert_eq!(hist.max(), 1000.0);

    println!("\n✅ Core statistics (count, sum, min, max) remain exact");
    println!("   Only bucket granularity is affected by overflow");
}

#[test]
fn test_bucket_counts_with_overflow() {
    // Specifically check what happens to bucket_counts when span exceeds 256

    let scale = 4;
    let hist = ExponentialHistogram::new(scale);

    println!("\n=== Test: Bucket Counts with Overflow ===");

    // Add values with huge span
    hist.accumulate(0.01);
    hist.accumulate(1000.0);

    let idx_small = calculate_otel_index(0.01, scale);
    let idx_large = calculate_otel_index(1000.0, scale);
    let span = idx_large - idx_small + 1;

    println!("Small value index: {}", idx_small);
    println!("Large value index: {}", idx_large);
    println!("Span: {} buckets", span);

    if span > 256 {
        println!("⚠️  Span exceeds 256 bucket capacity");
    }

    let offset = hist.bucket_start_offset();
    let (counts, _) = hist.take_counts();

    println!("\nBucket array:");
    println!("  Offset: {}", offset);
    println!("  Array length: {}", counts.len());

    let non_zero: Vec<_> = counts.iter().enumerate()
        .filter(|&(_, &c)| c > 0)
        .collect();

    println!("  Non-zero buckets: {}", non_zero.len());

    for &(i, &count) in &non_zero {
        let otel_idx = offset + i as i32;
        println!("    Bucket[{}]: OTel index {}, count {}", i, otel_idx, count);
    }

    // If span > 256, one value must be capped
    if span > 256 {
        println!("\nBehavior when span > 256:");

        // Check if large value is at bucket[255]
        if counts[255] > 0 {
            println!("  ✅ Large value capped at bucket[255]");
            println!("     OTel index {} mapped to bucket[255]", idx_large);
            println!("     Actual index would be: {}", idx_large);
        }

        // Or small value at bucket[0]
        if counts[0] > 0 {
            let expected_pos = idx_small - offset;
            println!("  Small value at bucket[0]");
            println!("     Expected position: {}", expected_pos);
        }
    }

    assert_eq!(hist.count(), 2, "Both values should be counted");
}
