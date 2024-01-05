use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use r2kad_lib::domain::NodeId;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("node_id_xor", |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::new(0, 0);
            for _i in 0..iters {
                let first = NodeId::random();
                let second = NodeId::random();

                let start = Instant::now();
                black_box(&first ^ &second);
                elapsed += start.elapsed();
            }
            elapsed
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
