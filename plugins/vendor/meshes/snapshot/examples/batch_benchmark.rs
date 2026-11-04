use std::path::PathBuf;
use std::time::{Duration, Instant};

use helio_snapshot::{SnapshotBatch, SnapshotConfig};

const ITERS: usize = 100;

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .init();

    let model_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/assets/test.fbx");

    println!("Model: {}", model_path.display());
    println!("Initialising GPU and renderer...");

    let init_start = Instant::now();
    let mut batch = SnapshotBatch::new(SnapshotConfig {
        width: 512,
        height: 512,
        ..Default::default()
    })
    .expect("failed to initialise SnapshotBatch");
    let init_ms = init_start.elapsed().as_millis();

    println!("Init: {}ms\n", init_ms);
    println!("{:<6}  {:>10}  {:>12}", "iter", "iter ms", "running avg");
    println!("{}", "-".repeat(32));

    let mut times: Vec<Duration> = Vec::with_capacity(ITERS);

    for i in 0..ITERS {
        let t = Instant::now();
        let _img = batch.render(&model_path).expect("render failed");
        let elapsed = t.elapsed();
        times.push(elapsed);

        let avg_ms =
            times.iter().map(|d| d.as_secs_f64() * 1000.0).sum::<f64>() / times.len() as f64;

        println!(
            "{:<6}  {:>9.1}ms  {:>11.1}ms",
            i + 1,
            elapsed.as_secs_f64() * 1000.0,
            avg_ms,
        );
    }

    let total: Duration = times.iter().sum();
    let avg = total / ITERS as u32;
    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();

    println!("\n── Results ({ITERS} iters) ─────────────────────");
    println!("  Total : {:.1}ms", total.as_secs_f64() * 1000.0);
    println!("  Avg   : {:.1}ms / frame", avg.as_secs_f64() * 1000.0);
    println!("  Min   : {:.1}ms", min.as_secs_f64() * 1000.0);
    println!("  Max   : {:.1}ms", max.as_secs_f64() * 1000.0);
    println!(
        "  Rate  : {:.1} snapshots/sec",
        ITERS as f64 / total.as_secs_f64()
    );
}
