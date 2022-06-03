use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use r2kad_lib::domain::{Id, NodeId};

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_id_shared_prefix_bits");
    group.bench_function(BenchmarkId::from_parameter(1), |b| {
        let first = NodeId::<1>::zero();
        let second = NodeId::<1>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(2), |b| {
        let first = NodeId::<2>::zero();
        let second = NodeId::<2>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(4), |b| {
        let first = NodeId::<4>::zero();
        let second = NodeId::<4>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(8), |b| {
        let first = NodeId::<8>::zero();
        let second = NodeId::<8>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(16), |b| {
        let first = NodeId::<16>::zero();
        let second = NodeId::<16>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(32), |b| {
        let first = NodeId::<32>::zero();
        let second = NodeId::<32>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.bench_function(BenchmarkId::from_parameter(64), |b| {
        let first = NodeId::<64>::zero();
        let second = NodeId::<64>::one();
        b.iter(|| first.shared_prefix_bits(&second))
    });
    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
