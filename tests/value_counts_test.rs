/// Test for value_counts function
use exponential_histogram::ExponentialHistogram;

#[test]
fn test_value_counts_positive_values() {
    let hist = ExponentialHistogram::new(4);

    // Add some values
    hist.accumulate(1.0);
    hist.accumulate(2.0);
    hist.accumulate(2.0); // Same bucket as one of the above
    hist.accumulate(10.0);
    hist.accumulate(100.0);

    println!("\n=== Value Counts Test ===");
    println!("Scale: 4");
    println!("Total count: {}", hist.count());

    let value_counts: Vec<_> = hist.value_counts().collect();

    println!("\nBuckets with data:");
    for (lower_bound, count) in &value_counts {
        println!("  Lower boundary: {:.6}, Count: {}", lower_bound, count);
    }

    // Verify we got some buckets
    assert!(value_counts.len() > 0, "Should have at least one bucket");

    // Verify total count matches
    let total_count: usize = value_counts.iter().map(|(_, count)| count).sum();
    assert_eq!(
        total_count,
        hist.count(),
        "Sum of bucket counts should equal total count"
    );

    // Verify lower boundaries are in ascending order (since we iterate negative then positive)
    for window in value_counts.windows(2) {
        assert!(
            window[0].0 <= window[1].0,
            "Lower boundaries should be in ascending order"
        );
    }
}

#[test]
fn test_value_counts_with_negative_values() {
    let hist = ExponentialHistogram::new(4);

    // Add negative values
    hist.accumulate(-5.0);
    hist.accumulate(-1.0);

    // Add positive values
    hist.accumulate(1.0);
    hist.accumulate(10.0);

    println!("\n=== Value Counts with Negatives ===");

    let value_counts: Vec<_> = hist.value_counts().collect();

    println!("Buckets:");
    for (lower_bound, count) in &value_counts {
        println!("  Lower boundary: {:.6}, Count: {}", lower_bound, count);
    }

    // Should have both negative buckets (stored by absolute value) and positive buckets
    // All lower boundaries are positive since they represent magnitudes
    let has_small_boundaries = value_counts.iter().any(|(lb, _)| *lb < 1.0);
    assert!(
        has_small_boundaries,
        "Should have buckets with small lower boundaries for negative values"
    );

    // Total count should match
    let total_count: usize = value_counts.iter().map(|(_, count)| count).sum();
    assert_eq!(total_count, hist.count());
}

#[test]
fn test_value_counts_with_fractional_values() {
    let hist = ExponentialHistogram::new(4);

    // Add fractional values < 1.0
    hist.accumulate(0.1);
    hist.accumulate(0.5);
    hist.accumulate(0.75);

    println!("\n=== Value Counts with Fractional Values ===");

    let value_counts: Vec<_> = hist.value_counts().collect();

    println!("Buckets:");
    for (lower_bound, count) in &value_counts {
        println!("  Lower boundary: {:.6}, Count: {}", lower_bound, count);
    }

    // Should have buckets with lower boundaries < 1.0
    let has_fractional = value_counts.iter().any(|(lb, _)| *lb < 1.0 && *lb > 0.0);
    assert!(
        has_fractional,
        "Should have buckets with fractional lower boundaries"
    );

    // Total count should match
    let total_count: usize = value_counts.iter().map(|(_, count)| count).sum();
    assert_eq!(total_count, 3);
}

#[test]
fn test_value_counts_only_nonzero_buckets() {
    let hist = ExponentialHistogram::new(4);

    // Add just a few values that should map to specific buckets
    hist.accumulate(1.0);
    hist.accumulate(100.0);

    let value_counts: Vec<_> = hist.value_counts().collect();

    println!("\n=== Non-zero Buckets Only ===");
    println!("Total buckets returned: {}", value_counts.len());

    // Should only return buckets with count > 0
    for (lower_bound, count) in &value_counts {
        assert!(*count > 0, "All returned buckets should have count > 0");
        println!("  Lower boundary: {:.6}, Count: {}", lower_bound, count);
    }

    // With scale 4 and these values, we should have well-separated buckets
    // So should have at least 2 buckets
    assert!(
        value_counts.len() >= 2,
        "Should have at least 2 separate buckets"
    );
}

#[test]
fn test_value_counts_empty_histogram() {
    let hist = ExponentialHistogram::new(4);

    let value_counts: Vec<_> = hist.value_counts().collect();

    println!("\n=== Empty Histogram ===");
    println!("Buckets: {}", value_counts.len());

    // Empty histogram should return empty iterator
    assert_eq!(
        value_counts.len(),
        0,
        "Empty histogram should have no buckets"
    );
}
