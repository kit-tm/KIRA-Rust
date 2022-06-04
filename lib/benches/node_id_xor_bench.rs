use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use r2kad_lib::domain::NodeId;

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_id_xor");
    group.bench_function(BenchmarkId::from_parameter(1), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<1>::random();
                let second = NodeId::<1>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(2), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<2>::random();
                let second = NodeId::<2>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(4), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<4>::random();
                let second = NodeId::<4>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(8), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<8>::random();
                let second = NodeId::<8>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(16), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<16>::random();
                let second = NodeId::<16>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(32), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<32>::random();
                let second = NodeId::<32>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.bench_function(BenchmarkId::from_parameter(64), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::<64>::random();
                let second = NodeId::<64>::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
