# Performance


### pelikan-io/histogram
This is great, but use of this crate will reduce our precision when converting f64 -> u64
```
histograms/h2histogram  time:   [3.0626 ns 3.0664 ns 3.0717 ns]
                        thrpt:  [325.55 Melem/s 326.12 Melem/s 326.52 Melem/s]
```

### kvc0/exponential_histogram
```
histograms/exponential  time:   [9.8152 ns 9.8321 ns 9.8497 ns]
                        thrpt:  [101.53 Melem/s 101.71 Melem/s 101.88 Melem/s]
```

### brayniac/exponential_histogram#main
Super fast with good throughput, but not correct
```
histograms/exponential  time:   [3.6896 ns 3.6921 ns 3.6947 ns]
                        thrpt:  [270.66 Melem/s 270.85 Melem/s 271.03 Melem/s]
```

### brayniac/exponential_histogram#correctness-investigation
Decent compromise between the two, slightly slower but fits the OTel spec
```
histograms/exponential  time:   [5.0242 ns 5.0315 ns 5.0411 ns]
                        thrpt:  [198.37 Melem/s 198.75 Melem/s 199.03 Melem/s]
```

# correctness issues
Issues were diagnosed with the help of Claude and OTel documentation.

## Use of the crate histogram

While fast, results in loss of precision when converting f64 -> u64

## Storing in integer buckets

The histogram crate uses power of 2 buckets for integers:
- Bucket 0: [0, 1)
- Bucket 1: [1, 2)
- Bucket 2: [2, 4)
- etc.

OTel spec requires exponential buckets for floating-point with configurable scale:
For scale=0 (base=2):
- Bucket -2: (0.25, 0.5]
- Bucket -1: (0.5, 1]
- Bucket 0: (1, 2]
- Bucket 1: (2, 4]
- etc.

```
Original Value: 0.75
                 ↓
          (truncate to u64)
                 ↓
              Value: 0
                 ↓
        (histogram crate)
                 ↓
     Integer Bucket 0: [0, 1)
                 ↓
    (try to convert to OTel)
                 ↓
      Incorrect OTel Bucket
```
Correct OTel bucket for 0.75 should be:
  Bucket -1: (0.5, 1]

## `value_to_otel_index` not correctly implementing OTel conversion
Newer implementation is based off of 
https://opentelemetry.io/docs/specs/otel/metrics/data-model/#producer-expectations

Where positive (lower bounds) are handled
