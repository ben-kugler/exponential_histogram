use criterion::Criterion;
use exponential_histogram::ExponentialHistogram;

pub fn accumulate(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("histograms");
    group.throughput(criterion::Throughput::Elements(1));

    let mut histogram = ExponentialHistogram::new_with_max_buckets(8, 40);
    group.bench_function("exponential", |bencher| {
        let mut i = 1;
        bencher.iter(|| {
            histogram.accumulate(i);
            i = (i + 1) % 1000000;
        });
    });

    let mut histogram = histogram::Histogram::new(7, 32).expect("hardcoded but pretty quick!");
    group.bench_function("h2histogram", |bencher| {
        let mut i = 1;
        bencher.iter(|| {
            histogram.increment(i).expect("but pretty fixed");
            i = (i + 1) % 1000000;
        });
    });
}

criterion::criterion_group!(benches, accumulate);
