//! What sculpt strokes cost the callers of CPU world queries on Earth with
//! its features (a measurement, run with `--ignored --nocapture`): the
//! `WorldQueries::edit` call a stamp makes, the longest read another thread
//! waits for meanwhile, and how long an edit takes to show.

use glam::DVec3;
use helio_voxel_world::feature_recipe::FeatureRecipe;
use helio_voxel_world::layers::TerrainLayers;
use helio_voxel_world::queries::WorldQueries;
use helio_voxel_world::{Brush, BrushOp, BrushShape, Matter, Planet, PlanetRecipe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn earth() -> Planet {
    let voxel = 0.1;
    Planet::new(PlanetRecipe {
        voxel_size_m: voxel,
        terrain: TerrainLayers::earth().source(7),
        features: FeatureRecipe::earth().grow(voxel, 7),
        ..Default::default()
    })
    .unwrap()
}

/// A surface point (1.7 m up) under the most feature cells around a few
/// directions: a forest.
fn forest(planet: &Planet) -> DVec3 {
    (0..64)
        .map(|t| {
            let a = t as f64 * 0.37;
            let dir = DVec3::new(a.cos(), 0.8 + 0.1 * (a * 1.3).sin(), a.sin()).normalize();
            let eye = planet.surface_point_of(dir * planet.outer_radius(), 1.7, Matter::Ground);
            (
                planet.ground_height_of(eye, Matter::Ground)
                    - planet.ground_height_of(eye, Matter::All),
                eye,
            )
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .unwrap()
        .1
}

fn stat(v: &mut [f64]) -> String {
    v.sort_by(f64::total_cmp);
    let at = |q: f64| {
        v.get(((v.len() as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    format!(
        "p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} ms ({})",
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0),
        v.len()
    )
}

fn wait_loaded(queries: &WorldQueries) {
    let started = Instant::now();
    while queries.loading() > 0 {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "bricks still loading"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore]
fn sculpt_strokes_against_queries() {
    let planet = earth();
    let eye = forest(&planet);
    let up = planet.grid().up(eye);
    let side = up.any_orthonormal_vector();
    let ahead = up.cross(side);
    let ground = planet.surface_point_of(eye + ahead * 8.0, 0.0, Matter::Ground);
    let queries = Arc::new(WorldQueries::new(Arc::new(planet.clone()), 4));
    // An editor's sculpt focus.
    queries.focus(1, ground, 16.0);
    let started = Instant::now();
    wait_loaded(&queries);
    eprintln!(
        "QUERY_EDITS {} bricks resident in {:.1} s",
        queries.resident(),
        started.elapsed().as_secs_f64()
    );
    for (name, op, shape, radius) in [
        ("build", BrushOp::Add, BrushShape::Cube, 6.0),
        ("dig", BrushOp::Remove, BrushShape::Sphere, 4.0),
        ("mound", BrushOp::Add, BrushShape::Sphere, 6.0),
        ("block", BrushOp::Add, BrushShape::Cube, 0.25),
    ] {
        // A reader on another thread: rays and cells around the stroke, as a
        // player or a sculpt cursor reads them.
        let stop = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let probe = {
            let (queries, stop, reads) = (queries.clone(), stop.clone(), reads.clone());
            let grid = *planet.grid();
            std::thread::spawn(move || {
                let mut n = 0u32;
                let mut out = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let a = f64::from(n) * 0.37;
                    let dir =
                        (ahead * 8.0 + side * (a.sin() * 6.0) - up * (1.0 + a.cos())).normalize();
                    let t = Instant::now();
                    std::hint::black_box(queries.raycast(eye, dir, 14.0, Matter::All));
                    let cell = grid.locate(ground + side * a.cos() * 4.0).0;
                    std::hint::black_box(queries.solid(cell, Matter::All));
                    out.push(t.elapsed().as_secs_f64() * 1e3);
                    n += 1;
                    std::thread::sleep(Duration::from_micros(500));
                }
                reads.lock().unwrap().extend(out);
            })
        };
        let mut calls = Vec::new();
        let stroke = Instant::now();
        for n in 0..20 {
            let frame = Instant::now();
            let at = ground
                + side * (radius * (f64::from(n) * 0.35).sin())
                + ahead * (radius * 0.1 * f64::from(n));
            let center = if op == BrushOp::Add {
                at + up * (radius * 0.5)
            } else {
                at
            };
            let brush = Brush {
                center: center.to_array(),
                radius,
                shape,
                op,
                material: 9,
                height: 0.0,
            };
            let t = Instant::now();
            queries.edit(&[brush]);
            calls.push(t.elapsed().as_secs_f64() * 1e3);
            // 60 Hz frames.
            if let Some(rest) = Duration::from_micros(16_667).checked_sub(frame.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        let stroke_s = stroke.elapsed().as_secs_f64();
        let shown = Instant::now();
        while queries.editing() > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let shown_ms = shown.elapsed().as_secs_f64() * 1e3;
        stop.store(true, Ordering::Relaxed);
        probe.join().unwrap();
        let mut reads = std::mem::take(&mut *reads.lock().unwrap());
        eprintln!(
            "QUERY_EDITS {name} r {radius} m: stroke {stroke_s:.2} s; edit call {}; reads {}; all shown {shown_ms:.1} ms after the last stamp",
            stat(&mut calls),
            stat(&mut reads),
        );
    }
}
