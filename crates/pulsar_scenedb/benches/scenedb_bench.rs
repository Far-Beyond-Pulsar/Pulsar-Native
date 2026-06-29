use criterion::{criterion_group, criterion_main, Criterion};
use pulsar_scenedb::{Aabb, Frustum, SpatialCell};
use std::hint::black_box;

fn bench_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("spatial_query");
    for &n in &[256u32, 1024] {
        let mut cell = SpatialCell::new(n).unwrap();
        for i in 0..n {
            let f = i as f32;
            cell.alloc(Aabb {
                min: [f, 0.0, 0.0],
                max: [f + 1.0, 1.0, 1.0],
            })
            .unwrap();
        }
        let q = Aabb {
            min: [0.0, 0.0, 0.0],
            max: [n as f32 / 2.0, 1.0, 1.0],
        };
        let mut out = vec![0u32; n as usize];
        group.bench_function(format!("scalar_aabb_scan_{n}"), |b| {
            b.iter(|| black_box(cell.query_aabb(black_box(&q), &mut out)))
        });
    }
    group.finish();
}

fn bench_churn(c: &mut Criterion) {
    c.bench_function("alloc_free_compact_256", |b| {
        b.iter(|| {
            let mut cell = SpatialCell::new(256).unwrap();
            let hs: Vec<_> = (0..256)
                .map(|i| {
                    cell.alloc(Aabb {
                        min: [i as f32; 3],
                        max: [i as f32 + 1.0; 3],
                    })
                    .unwrap()
                })
                .collect();
            for h in hs.iter().step_by(2) {
                cell.free(*h);
            }
            cell.compact();
            black_box(cell.rows_in_use())
        })
    });
}

fn bench_aabb_dispatch(c: &mut criterion::Criterion) {
    let mut group = c.benchmark_group("aabb_dispatch");
    for &n in &[256u32, 1024] {
        let mut cell = SpatialCell::new(n).unwrap();
        for i in 0..n {
            let f = i as f32;
            cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
        }
        let q = Aabb { min: [0.0, 0.0, 0.0], max: [n as f32 / 2.0, 1.0, 1.0] };
        let mut out = vec![0u32; n as usize];
        // This routes through the runtime dispatcher (AVX2 where available).
        group.bench_function(format!("dispatched_aabb_scan_{n}"), |b| {
            b.iter(|| black_box(cell.query_aabb(black_box(&q), &mut out)))
        });
    }
    group.finish();
}

fn bench_frustum(c: &mut criterion::Criterion) {
    let mut cell = SpatialCell::new(1024).unwrap();
    for i in 0..1024u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    let f = Frustum { planes: [
        [1.0, 0.0, 0.0, 200.0], [-1.0, 0.0, 0.0, 800.0],
        [0.0, 1.0, 0.0, 10.0], [0.0, -1.0, 0.0, 10.0],
        [0.0, 0.0, 1.0, 10.0], [0.0, 0.0, -1.0, 10.0],
    ] };
    let mut out = vec![0u32; 1024];
    c.bench_function("frustum_scan_1024", |b| {
        b.iter(|| black_box(cell.query_frustum(black_box(&f), &mut out)))
    });
}

criterion_group!(benches, bench_query, bench_churn, bench_aabb_dispatch, bench_frustum);
criterion_main!(benches);
