use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use r2kad_lib::domain::NodeId;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("node_id_shared_prefix_bits", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::random();
                let second = NodeId::random();

                let start = Instant::now();
                black_box(first.shared_prefix_bits(&second));
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
