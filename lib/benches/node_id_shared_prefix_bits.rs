use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use r2kad_lib::domain::NodeID;

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_id_shared_prefix_bits");
    group.bench_function(BenchmarkId::from_parameter(1), |b| {
        let first = NodeID::<1>::zero();
        let second = NodeID::<1>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(2), |b| {
        let first = NodeID::<2>::zero();
        let second = NodeID::<2>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(4), |b| {
        let first = NodeID::<4>::zero();
        let second = NodeID::<4>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(8), |b| {
        let first = NodeID::<8>::zero();
        let second = NodeID::<8>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(16), |b| {
        let first = NodeID::<16>::zero();
        let second = NodeID::<16>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(32), |b| {
        let first = NodeID::<32>::zero();
        let second = NodeID::<32>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(64), |b| {
        let first = NodeID::<64>::zero();
        let second = NodeID::<64>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
