//! Timings for cooking real images, for people who want numbers from their own
//! project. Ignored by default.
//!
//! ```text
//! PULSAR_TEST_TEXTURES="a.jpg;b.jpg" cargo test -p helio_component --release \
//!     --test real_texture_cook -- --ignored --nocapture
//! ```
//!
//! It loads each image three ways and prints how long each took and how big
//! each form is: the first load (decode, resize, mips, BC7, write the derived
//! file), a second load of the same file (read the derived file), and a rebuild
//! after the residency cache dropped the value (read it again).

use std::path::PathBuf;
use std::time::Instant;

use helio_component::cooked_texture::CookFormat;
use helio_component::texture_assets::{set_preferred_cook_format, LoadedTexture};
use pulsar_scenedb::{
    derived_stats, residency_budget, set_derived_cache_dir, set_residency_budget,
};

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[test]
#[ignore = "needs PULSAR_TEST_TEXTURES; run on demand"]
fn cook_real_textures_and_report_what_it_costs() {
    let Ok(list) = std::env::var("PULSAR_TEST_TEXTURES") else {
        eprintln!("set PULSAR_TEST_TEXTURES to a ';'-separated list of image files");
        return;
    };
    let paths: Vec<PathBuf> = list.split(';').filter(|p| !p.is_empty()).map(PathBuf::from).collect();
    let scratch = tempfile::tempdir().unwrap();
    set_derived_cache_dir(Some(scratch.path().join("derived")));
    set_preferred_cook_format(CookFormat::Bc7);

    println!(
        "\n{:<34} {:>11} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "image", "source", "first(ms)", "again(ms)", "evict(ms)", "bc7 MiB", "rgba8 MiB", "rgba8x1"
    );
    let (mut total_source, mut total_bc7, mut total_rgba, mut total_flat) = (0usize, 0usize, 0usize, 0usize);
    for path in &paths {
        let source_bytes = std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0);

        let started = Instant::now();
        let first = LoadedTexture::load(path).expect("first load");
        let first_ms = started.elapsed().as_secs_f64() * 1000.0;

        let started = Instant::now();
        let again = LoadedTexture::load(path).expect("second load");
        let again_ms = started.elapsed().as_secs_f64() * 1000.0;
        drop(again);

        // Make the residency cache drop `first`, then rebuild it.
        let budget = residency_budget();
        set_residency_budget(1);
        let filler_path = paths.iter().find(|p| *p != path).unwrap_or(path);
        let _filler = LoadedTexture::load(filler_path).unwrap();
        let evicted = !first.is_resident();
        let started = Instant::now();
        let bc7 = first.cooked(CookFormat::Bc7).expect("rebuild");
        let evict_ms = started.elapsed().as_secs_f64() * 1000.0;
        set_residency_budget(budget);

        let rgba8 = first.cooked(CookFormat::Rgba8).expect("rgba8");
        let flat = first.width as usize * first.height as usize * 4;
        println!(
            "{:<34} {:>8.1} MiB {:>9.0} {:>9.1} {:>9.1} {:>9.2} {:>9.2} {:>9.2}{}",
            path.file_name().unwrap().to_string_lossy(),
            mib(source_bytes),
            first_ms,
            again_ms,
            evict_ms,
            mib(bc7.byte_size()),
            mib(rgba8.byte_size()),
            mib(flat),
            if evicted { "" } else { "  (not evicted)" }
        );
        total_source += source_bytes;
        total_bc7 += bc7.byte_size();
        total_rgba += rgba8.byte_size();
        total_flat += flat;
    }
    let stats = derived_stats();
    println!(
        "\nTOTAL source {:.1} MiB | bc7+mips {:.1} MiB | rgba8+mips {:.1} MiB | rgba8 one level {:.1} MiB",
        mib(total_source), mib(total_bc7), mib(total_rgba), mib(total_flat)
    );
    println!(
        "derived cache: {} hits, {} cooked, {:.1} MiB written, {:.1} MiB read",
        stats.hits, stats.misses, mib(stats.bytes_written as usize), mib(stats.bytes_read as usize)
    );
    set_derived_cache_dir(None);
}
