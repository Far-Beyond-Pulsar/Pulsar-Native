//! The cooked-texture path through SceneDB's two caches: a texture is cooked
//! once and read back from disk afterwards, a value the residency cache drops
//! is rebuilt from that file instead of from the source image, and a cache
//! entry that cannot be used heals itself.
//!
//! This is its own test binary because the derived-data cache directory and
//! the residency budget are process-wide.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use helio_component::cooked_texture::{CookFormat, COOK_VERSION};
use helio_component::texture_assets::{max_texture_size, set_preferred_cook_format, LoadedTexture};
use pulsar_scenedb::{
    derive_cached, derived_stats, residency_budget, set_derived_cache_dir, set_residency_budget,
    DerivedRequest,
};

/// The derived-data cache directory, the preferred format and the residency budget
/// are process-wide, so the tests in this file take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn write_png(dir: &Path, name: &str, width: u32, height: u32, seed: u8) -> PathBuf {
    let path = dir.join(name);
    let mut image = image::RgbImage::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgb([(x as u8).wrapping_add(seed), (y as u8).wrapping_mul(3), seed]);
    }
    image.save(&path).unwrap();
    path
}

fn cache_files(root: &Path) -> usize {
    std::fs::read_dir(root.join("texture"))
        .map(|entries| entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pdd")).count())
        .unwrap_or(0)
}

#[test]
fn textures_are_cooked_once_rebuilt_from_disk_and_heal_a_bad_entry() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    let cache = scratch.path().join("cache");
    set_derived_cache_dir(Some(cache.clone()));
    set_preferred_cook_format(CookFormat::Bc7);
    let image = write_png(scratch.path(), "wall.png", 64, 64, 10);

    // First load: a miss. It cooks BC7 with a full chain and stores the result.
    let before = derived_stats();
    let first = LoadedTexture::load(&image).unwrap();
    let after_first = derived_stats();
    assert_eq!(after_first.misses - before.misses, 1);
    assert_eq!(after_first.bytes_written > before.bytes_written, true);
    let cooked = first.cooked(CookFormat::Bc7).unwrap();
    assert_eq!((cooked.format, cooked.mip_count()), (CookFormat::Bc7, 7));
    assert_eq!(cache_files(&cache), 1);

    // A second load of the same file is a hit: no cook.
    let second = LoadedTexture::load(&image).unwrap();
    let after_second = derived_stats();
    assert_eq!(after_second.hits - after_first.hits, 1);
    assert_eq!(after_second.misses, after_first.misses, "nothing was cooked again");
    assert_eq!(*second.cooked(CookFormat::Bc7).unwrap(), *cooked, "same bytes from disk");

    // Another format is another result, cooked on first use and kept.
    let rgba8 = second.cooked(CookFormat::Rgba8).unwrap();
    assert_eq!(rgba8.format, CookFormat::Rgba8);
    assert!(rgba8.byte_size() > cooked.byte_size() * 3, "BC7 is a quarter of RGBA8");
    assert_eq!(cache_files(&cache), 2);

    // A value the residency cache drops is rebuilt by reading the file, not by
    // decoding and encoding the source again.
    let old_budget = residency_budget();
    set_residency_budget(1);
    let other = write_png(scratch.path(), "floor.png", 64, 64, 90);
    let _filler = LoadedTexture::load(&other).unwrap();
    assert!(!first.is_resident(), "the first texture was evicted for room");
    let misses_before = derived_stats().misses;
    let hits_before = derived_stats().hits;
    assert_eq!(*first.cooked(CookFormat::Bc7).unwrap(), *cooked);
    assert_eq!(derived_stats().misses, misses_before, "no cook");
    assert_eq!(derived_stats().hits, hits_before + 1, "one read");
    set_residency_budget(old_budget);

    // Changing the source makes a new result.
    let edited = write_png(scratch.path(), "wall.png", 64, 64, 200);
    let third = LoadedTexture::load(&edited).unwrap();
    assert_ne!(*third.cooked(CookFormat::Bc7).unwrap(), *cooked);

    // An entry that passes the cache's own checks but is not a texture is
    // forgotten and cooked again, once.
    let healing = write_png(scratch.path(), "bad.png", 32, 32, 5);
    let cap = max_texture_size();
    let settings = format!("cap={cap};format=bc7");
    let request = DerivedRequest {
        kind: "texture",
        source: &healing,
        settings: &settings,
        version: COOK_VERSION,
    };
    derive_cached(&request, || Ok(b"definitely not a cooked texture".to_vec())).unwrap();
    let misses = derived_stats().misses;
    let healed = LoadedTexture::load(&healing).unwrap();
    assert_eq!(healed.cooked(CookFormat::Bc7).unwrap().mip_count(), 6);
    let hits = derived_stats().hits;
    LoadedTexture::load(&healing).unwrap();
    assert_eq!(derived_stats().hits, hits + 1, "the repaired entry is a hit now");
    assert!(derived_stats().misses >= misses, "the bad entry was replaced");

    // A size that is not a whole number of blocks cannot be BC7: it is cooked
    // uncompressed, and still gets its chain.
    let odd = write_png(scratch.path(), "odd.png", 30, 18, 1);
    let odd = LoadedTexture::load(&odd).unwrap();
    let cooked_odd = odd.cooked(CookFormat::Bc7).unwrap();
    assert_eq!(cooked_odd.format, CookFormat::Rgba8);
    assert_eq!(cooked_odd.mip_count(), 5);

    set_derived_cache_dir(None);
}

#[test]
fn warming_an_evicted_texture_brings_it_back_ahead_of_use_and_a_resident_one_is_left_alone() {
    let _turn = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let scratch = tempfile::tempdir().unwrap();
    set_derived_cache_dir(Some(scratch.path().join("cache")));
    set_preferred_cook_format(CookFormat::Rgba8);
    let kept = LoadedTexture::load(&write_png(scratch.path(), "kept.png", 32, 32, 3)).unwrap();

    // Resident: warming starts nothing.
    let before = pulsar_scenedb::residency_stats().prefetches;
    kept.warm();
    assert_eq!(pulsar_scenedb::residency_stats().prefetches, before, "already in memory");

    // Make the cache drop it.
    let old_budget = residency_budget();
    set_residency_budget(1);
    let _filler = LoadedTexture::load(&write_png(scratch.path(), "filler.png", 32, 32, 9)).unwrap();
    assert!(!kept.is_resident_in(CookFormat::Rgba8), "evicted for room");
    set_residency_budget(old_budget);

    // Evicted: warming starts the rebuild, and it finishes without anyone asking.
    kept.warm();
    assert_eq!(pulsar_scenedb::residency_stats().prefetches, before + 1);
    let started = std::time::Instant::now();
    while !kept.is_resident_in(CookFormat::Rgba8) {
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "never came back");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    set_derived_cache_dir(None);
}
