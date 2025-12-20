# Performance


### pelikan-io/histogram
This is great, but use of this crate will reduce our precision when converting f64 -> u64
```
histograms/h2histogram  time:   [3.0626 ns 3.0664 ns 3.0717 ns]
                        thrpt:  [325.55 Melem/s 326.12 Melem/s 326.52 Melem/s]
```

### kvc0
```
histograms/exponential  time:   [9.8152 ns 9.8321 ns 9.8497 ns]
                        thrpt:  [101.53 Melem/s 101.71 Melem/s 101.88 Melem/s]

histograms/shared-exponential
                        time:   [15.207 ns 15.233 ns 15.264 ns]
                        thrpt:  [65.515 Melem/s 65.647 Melem/s 65.757 Melem/s]
```

### brayniac#main
Super fast with good throughput, but not correct
```
histograms/exponential  time:   [3.7246 ns 3.7325 ns 3.7408 ns]
                        thrpt:  [267.32 Melem/s 267.92 Melem/s 268.48 Melem/s]

histograms/shared-exponential
                        time:   [3.7480 ns 3.7589 ns 3.7692 ns]
                        thrpt:  [265.31 Melem/s 266.04 Melem/s 266.81 Melem/s]
```

### brayniac#correctness-investigation
Decent compromise between the two, slightly slower but fits the OTel spec
```
histograms/exponential  time:   [5.0242 ns 5.0315 ns 5.0411 ns]
                        thrpt:  [198.37 Melem/s 198.75 Melem/s 199.03 Melem/s]

histograms/shared-exponential
                        time:   [9.5334 ns 9.6272 ns 9.7336 ns]
                        thrpt:  [102.74 Melem/s 103.87 Melem/s 104.89 Melem/s]
```

### brayniac#correctness-investigation#554646d6d15e2404b758a98bcc0ad02152b427a4
```
histograms/exponential  time:   [16.700 ns 16.719 ns 16.742 ns]
                        thrpt:  [59.732 Melem/s 59.814 Melem/s 59.880 Melem/s]

histograms/shared-exponential
                        time:   [5.4478 ns 5.4506 ns 5.4540 ns]
                        thrpt:  [183.35 Melem/s 183.47 Melem/s 183.56 Melem/s]
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
