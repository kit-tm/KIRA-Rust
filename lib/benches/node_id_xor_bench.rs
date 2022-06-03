use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use r2kad_lib::domain::NodeID;

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_id_xor");
    group.bench_function(BenchmarkId::from_parameter(1), |b| {
        let first = NodeID::<1>::random();
        let second = NodeID::<1>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(2), |b| {
        let first = NodeID::<2>::random();
        let second = NodeID::<2>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(4), |b| {
        let first = NodeID::<4>::random();
        let second = NodeID::<4>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(8), |b| {
        let first = NodeID::<8>::random();
        let second = NodeID::<8>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(16), |b| {
        let first = NodeID::<16>::random();
        let second = NodeID::<16>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(32), |b| {
        let first = NodeID::<32>::random();
        let second = NodeID::<32>::random();
        b.iter(|| &first ^ &second)
    });
    group.bench_function(BenchmarkId::from_parameter(64), |b| {
        let first = NodeID::<64>::random();
        let second = NodeID::<64>::random();
        b.iter(|| &first ^ &second)
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
