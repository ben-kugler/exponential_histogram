# Bucket Overflow Behavior - When Span Exceeds 256 Buckets

## Question
**What happens when the span of OTel bucket indices exceeds the 256-bucket capacity?**

## Answer
The implementation uses **bucket capping** - values that would exceed the 256-bucket range are capped to the boundary buckets (bucket[0] or bucket[255]).

**Critical Finding:** Core statistics (count, sum, min, max) remain **exact** even during overflow. Only bucket-level granularity is affected.

## How Bucket Overflow Works

### Fixed Capacity
```rust
const INITIAL_CAPACITY: usize = 256;

pub(crate) struct InnerHistogram {
    bucket_counts: Arc<Box<[AtomicU64]>>,  // Fixed 256 buckets
    offset: AtomicI32,                      // Minimum OTel index
    min_boundary: AtomicI32,                // Actual min index seen
    max_boundary: AtomicI32,                // Actual max index seen
}
```

### Storage Capacity
- **Can store:** Any 256 consecutive OTel indices
- **Cannot store:** Indices spanning more than 256 buckets simultaneously

**Examples:**
- ✅ Indices from -1000 to -745 (span of 256)
- ✅ Indices from -54 to 201 (span of 256)
- ❌ Indices from -160 to 212 (span of 373, exceeds capacity)

## Overflow Scenarios

### Scenario 1: Adding Large Value After Small Value

**Setup:**
```rust
let hist = ExponentialHistogram::new(4);
hist.accumulate(0.1);    // Index: -54, offset: -54
hist.accumulate(10000.0); // Index: 212, span: 266 buckets
```

**Result:**
```
Small value: 0.1 -> index -54
Huge value: 10000.0 -> index 212
Span: 212 - (-54) = 266 buckets (exceeds 256!)

Offset: -54
Non-zero buckets: 2
  Bucket[0]: OTel index -54, count 1 (value 0.1)
  Bucket[255]: OTel index 201, count 1 (value 10000.0 CAPPED)

⚠️ Value 10000.0 capped at bucket[255]
   Actual index: 212
   Stored at: 201 (offset -54 + 255)
```

**Behavior:**
- Small value sets offset to -54
- Large value would be at position 212 - (-54) = 266
- Position 266 exceeds capacity, so it's **capped at bucket[255]**
- Represents OTel index 201 instead of actual 212

### Scenario 2: Adding Small Value After Large Value

**Setup:**
```rust
let hist = ExponentialHistogram::new(4);
hist.accumulate(1000.0);  // Index: 159, offset: 159
hist.accumulate(0.001);   // Index: -160, span: 319 buckets
```

**Result:**
```
Large value: 1000.0 -> index 159
Tiny value: 0.001 -> index -160
Span: 159 - (-160) = 319 buckets (exceeds 256!)

Offset before: 159
Offset after: 159 (unchanged!)
Non-zero buckets: 1
  Bucket[0]: OTel index 159, count 2 (both values!)

⚠️ Both values collapsed into bucket[0]
```

**Behavior:**
- Large value sets offset to 159
- Small value would require shifting offset to -160
- But shift of 319 exceeds capacity
- Small value is **capped at bucket[0]** instead
- Both values end up in the same bucket (loss of granularity)

### Scenario 3: Many Values Spanning Wide Range

**Setup:**
```rust
let hist = ExponentialHistogram::new(4);
let values = vec![0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0];
for &v in &values {
    hist.accumulate(v);
}
```

**Result:**
```
Total span: 373 buckets (from -160 to 212)
Capacity: 256 buckets
Overflow: 117 buckets

Histogram statistics:
  Count: 8 ✅ (all values counted)
  Min: 0.001 ✅ (exact)
  Max: 10000 ✅ (exact)
  Sum: 11111.11 ✅ (exact)

Bucket analysis:
  Non-zero buckets: 6 (some values collapsed together)
  Total count in buckets: 8
  ✅ All values stored in buckets

Boundary buckets:
  Bucket[0]: 1 (smallest value)
  Bucket[255]: 3 (largest values collapsed together)
```

**Key Finding:**
- ✅ **Count is exact** - tracked separately in AtomicU64
- ✅ **Sum is exact** - tracked separately in AtomicF64
- ✅ **Min is exact** - tracked separately in AtomicF64
- ✅ **Max is exact** - tracked separately in AtomicF64
- ⚠️ **Bucket granularity is lost** - some values collapsed into boundary buckets

## Implementation Details

### Code Handling Overflow

From `src/inner_histogram.rs:72-145`:

```rust
pub(crate) fn increment(&self, otel_index: i32) {
    let offset = self.offset.load(Ordering::Acquire);
    let index = otel_index - offset;

    if index < 0 {
        let shift = -index;

        if shift < INITIAL_CAPACITY as i32 {
            // Shift is within capacity, update offset
            match self.offset.compare_exchange(
                offset, otel_index,
                Ordering::AcqRel, Ordering::Acquire
            ) {
                Ok(_) => {
                    self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);
                    // Update min_boundary...
                }
                Err(current_offset) => {
                    // Retry with new offset
                    let index = otel_index - current_offset;
                    if index < 0 {
                        // Still negative, cap at bucket[0]
                        self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        } else {
            // Shift too large (> 256), cap at bucket[0]
            self.bucket_counts[0].fetch_add(1, Ordering::Relaxed);
            // Update min_boundary to track actual min...
        }
    }

    // Fast path: within bounds
    if index >= 0 && index < INITIAL_CAPACITY {
        self.bucket_counts[index].fetch_add(1, Ordering::Relaxed);
        // Update boundaries...
    } else {
        // Beyond capacity, cap at bucket[255]
        self.bucket_counts[INITIAL_CAPACITY - 1].fetch_add(1, Ordering::Relaxed);
        // Update max_boundary to track actual max...
    }
}
```

### Boundary Tracking

Even when values are capped, the implementation tracks the **actual** min/max indices:

```rust
pub(crate) struct InnerHistogram {
    offset: AtomicI32,        // Bucket[0] represents this index
    min_boundary: AtomicI32,  // Actual minimum index seen (may be < offset)
    max_boundary: AtomicI32,  // Actual maximum index seen (may be > offset+255)
}
```

This allows the histogram to report accurate min/max values even when those values are capped in buckets.

## Test Results

### Test 1: Exact Span of 256 ✅
```
Values: 0.1 to ~6000
Span: 254 buckets
Result: All values stored in separate buckets
```

### Test 2: Large Value Exceeds Capacity ⚠️
```
Values: 0.1, 10000.0
Span: 266 buckets (exceeds 256)
Result: Large value capped at bucket[255]
  Bucket[0]: index -54, count 1
  Bucket[255]: index 201, count 1 (actual: 212)
```

### Test 3: Small Value Exceeds Capacity ⚠️
```
Values: 1000.0, 0.001
Span: 319 buckets (exceeds 256)
Result: Both values in bucket[0]
  Bucket[0]: index 159, count 2
```

### Test 4: Wide Range of Values ⚠️
```
Values: 0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0, 10000.0
Span: 373 buckets (exceeds 256)
Result:
  Count: 8 ✅ (exact)
  Sum: 11111.11 ✅ (exact)
  Min: 0.001 ✅ (exact)
  Max: 10000.0 ✅ (exact)
  Non-zero buckets: 6 (some collapsed)
  Bucket[255]: 3 values
```

### Test 5: Statistics Remain Exact ✅
```
Values with huge span (exceeds 256 buckets)
Expected count: 7
Actual count: 7 ✅
Expected sum: 1111.111
Actual sum: 1111.111 ✅
Sum relative error: 0.0000% ✅

Min: exact ✅
Max: exact ✅
```

## Impact on OTel Compliance

### What Remains OTel-Compliant ✅
1. **Count** - Always exact
2. **Sum** - Always exact
3. **Min** - Always exact
4. **Max** - Always exact
5. **Zero count** - Always exact
6. **Scale** - Preserved
7. **Offset** - Correctly represents bucket[0] index (may be negative)

### What Gets Degraded ⚠️
1. **Bucket granularity** - Values outside the 256-bucket window get collapsed into boundary buckets
2. **Percentile accuracy** - Cannot calculate accurate percentiles if values are capped
3. **Distribution shape** - The detailed distribution is lost for capped values

### OTel Export Implications
When exporting to OTel format, the consumer will see:
- Accurate aggregate statistics (count, sum, min, max)
- Accurate bucket counts within the 256-bucket window
- **Inflated counts** at boundary buckets (bucket[0] and bucket[255]) if overflow occurred

This is **acceptable** for OTel because:
- The spec allows for limited bucket capacity
- Aggregate statistics remain exact
- Consumers can detect overflow by checking if boundary buckets have unexpectedly high counts

## When Does Overflow Occur?

### Calculating the Span
```
span = max_index - min_index + 1

where index = ceil(log(value) / log(base)) - 1
      base = 2^(2^(-scale))
```

### Examples at Scale 4
```
base = 2^(2^(-4)) = 2^0.0625 ≈ 1.0443

Value Range         Index Range       Span    Overflow?
0.1 to 1.0         -54 to -1         54      ✅ No
0.1 to 10.0        -54 to 53         108     ✅ No
0.1 to 100.0       -54 to 106        161     ✅ No
0.1 to 1000.0      -54 to 159        214     ✅ No
0.1 to 10000.0     -54 to 212        267     ❌ YES (overflow by 11)
0.001 to 1000.0    -160 to 159       320     ❌ YES (overflow by 64)
```

### Risk Factors for Overflow
1. **Wide value ranges** - e.g., subsecond to hours (0.001s to 3600s)
2. **High scale** - Higher scales create more buckets per decade
3. **Mixed small and large values** - e.g., cache hits (µs) and cache misses (ms)

## Mitigation Strategies

### 1. Choose Appropriate Scale
Lower scales reduce buckets per decade, allowing wider ranges:

```
Scale 0: ~3 buckets per power of 2
Scale 4: ~16 buckets per power of 2
Scale 8: ~256 buckets per power of 2
```

For wide ranges (6+ orders of magnitude), use scale ≤ 4.

### 2. Use Separate Histograms
If tracking vastly different value ranges (e.g., < 1ms and > 1s), use separate histograms:

```rust
let fast_hist = ExponentialHistogram::new(8);  // For 0.001 to 10 ms
let slow_hist = ExponentialHistogram::new(4);  // For 10 ms to 100 s
```

### 3. Accept Degraded Granularity
For many use cases, exact aggregate statistics are sufficient even if bucket-level detail is lost.

### 4. Monitor for Overflow
Check if boundary buckets have unusually high counts:

```rust
let (counts, _) = hist.take_counts();
if counts[0] > expected || counts[255] > expected {
    eprintln!("Warning: Bucket overflow detected");
}
```

## Comparison to Alternatives

### This Implementation (Fixed 256 Buckets)
- ✅ Lock-free
- ✅ Fixed memory (no allocations)
- ✅ Fast accumulate (single atomic increment)
- ✅ Exact statistics (count, sum, min, max)
- ⚠️ Degraded granularity if span > 256

### Dynamic Bucket Array
- ❌ Requires locks or complex lock-free data structures
- ❌ Memory allocations during resize
- ❌ Slower accumulate
- ✅ No granularity loss

### Downsampling (Scale Reduction)
- ✅ Keeps all values within capacity
- ⚠️ Loses granularity everywhere (not just at boundaries)
- ✅ Lock-free possible

## Conclusion

**When span exceeds 256 buckets:**

✅ **What stays exact:**
- Count (tracked separately)
- Sum (tracked separately)
- Min (tracked separately)
- Max (tracked separately)
- Zero count (tracked separately)

⚠️ **What degrades:**
- Bucket-level granularity
- Values outside the 256-bucket window are capped to boundary buckets
- Percentile calculations become less accurate

**Is this OTel-compliant?**
✅ Yes - the OTel spec allows limited bucket capacity, and aggregate statistics remain exact.

**When is this a problem?**
- If you need accurate percentiles (p50, p95, p99)
- If your value range regularly spans > 256 buckets at your chosen scale
- If you need the full distribution shape

**When is this acceptable?**
- If you primarily care about aggregate statistics (count, sum, min, max, mean)
- If overflow is rare (< 1% of observations)
- If you can choose an appropriate scale to keep span ≤ 256
- If you can split wide ranges across multiple histograms
