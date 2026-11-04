//! WGSL source assembly. There is no `#include`: each kernel is the
//! concatenation of the shared declarations and its own file.

pub const FORMAT: &str = helio_vg_format::WGSL;
pub const BINDINGS: &str = include_str!("../shaders/vg_bindings.wgsl");
/// THE position/index/attribute decode, shared by every VG pass
/// (`helio_vg_format::DECODE_WGSL`).
pub const DECODE: &str = helio_vg_format::DECODE_WGSL;
pub const RASTER_COMMON: &str = include_str!("../shaders/vg_raster_common.wgsl");
pub const HW_RASTER: &str = include_str!("../shaders/vg_hw_raster.wgsl");
pub const SW_RASTER: &str = include_str!("../shaders/vg_sw_raster.wgsl");
pub const SW_BIN: &str = include_str!("../shaders/vg_sw_bin.wgsl");
/// Reusable tiled shared-memory raster kernel (Tier 2 fallback, Tier 3 base).
pub const TILE_RASTER: &str = include_str!("../shaders/vg_tile_raster.wgsl");
/// Dense-bin overlap estimate (`dense_count_main`).
pub const DENSE_COUNT: &str = include_str!("../shaders/vg_dense_count.wgsl");
pub const TILE_CLASSIFY: &str = include_str!("../shaders/vg_tile_classify.wgsl");
/// Alpha-masked Tier 1 (`vs_masked`, `fs_masked`).
pub const HW_RASTER_MASKED: &str = include_str!("../shaders/vg_hw_raster_masked.wgsl");
/// Alpha-masked Tier 2, 64-bit atomic kernel (`sw_raster_masked_main`).
pub const SW_RASTER_MASKED: &str = include_str!("../shaders/vg_sw_raster_masked.wgsl");
/// `vg_tri_cover_at`: coverage plus barycentrics (masked software kernel only).
pub const TRI_COVER: &str = include_str!("../shaders/vg_tri_cover.wgsl");

/// Group 0 bindings 1 (page pool) and 2 (asset quant) read by [`DECODE`].
pub fn decode_bindings() -> String {
    helio_vg_format::decode_bindings_wgsl(0, 1, 2)
}

fn join(parts: &[&str]) -> String {
    parts.join("\n")
}

fn scene(extra: &[&str]) -> String {
    let db = decode_bindings();
    let mut parts: Vec<&str> = vec![FORMAT, BINDINGS, &db, DECODE];
    parts.extend_from_slice(extra);
    join(&parts)
}

/// Tier 1 vertex + fragment module.
pub fn hw_raster_source() -> String {
    scene(&[HW_RASTER])
}
/// Tier 2 with 64-bit atomics (`sw_raster_main`).
pub fn sw_raster_source() -> String {
    scene(&[RASTER_COMMON, SW_RASTER])
}
/// Tier 2 fallback binner (`sw_bin_main`).
pub fn sw_bin_source() -> String {
    scene(&[RASTER_COMMON, SW_BIN])
}
/// Tiled raster kernel (`tile_raster_main`).
pub fn tile_raster_source() -> String {
    scene(&[RASTER_COMMON, TILE_RASTER])
}
/// Tier 1 masked module; the alpha bindings are group 1 starting at binding 0.
pub fn hw_masked_source(binding: helio_mats::MaterialBindingConfig) -> String {
    let db = decode_bindings();
    let alpha = helio_mats::vg_alpha::bindings_wgsl(binding, 1, 0);
    format!(
        "{}{}",
        helio_mats::vg_alpha::module_header(binding),
        join(&[
            FORMAT,
            BINDINGS,
            &db,
            DECODE,
            &alpha,
            helio_mats::vg_alpha::WGSL,
            helio_mats::vg_alpha::UV_WGSL,
            HW_RASTER_MASKED
        ])
    )
}
/// Tier 2 masked module (64-bit atomics); the alpha bindings are group 2 starting at binding 0.
pub fn sw_masked_source(binding: helio_mats::MaterialBindingConfig) -> String {
    let db = decode_bindings();
    let alpha = helio_mats::vg_alpha::bindings_wgsl(binding, 2, 0);
    format!(
        "{}{}",
        helio_mats::vg_alpha::module_header(binding),
        join(&[
            FORMAT,
            BINDINGS,
            &db,
            DECODE,
            RASTER_COMMON,
            TRI_COVER,
            &alpha,
            helio_mats::vg_alpha::WGSL,
            helio_mats::vg_alpha::UV_WGSL,
            SW_RASTER_MASKED
        ])
    )
}
/// Dense-bin overlap estimate kernel.
pub fn dense_count_source() -> String {
    scene(&[DENSE_COUNT])
}
/// `classify_main` and `dense_args_main` (standalone, no scene bindings).
pub fn tile_classify_source() -> String {
    TILE_CLASSIFY.to_string()
}

/// Every module with a label, for validation.
pub fn all_sources() -> Vec<(&'static str, String)> {
    let bindless = helio_mats::MaterialBindingConfig {
        mode: helio_mats::MaterialBindingMode::BindingArray,
        max_textures: 256,
    };
    let expanded = helio_mats::MaterialBindingConfig {
        mode: helio_mats::MaterialBindingMode::Expanded,
        max_textures: 11,
    };
    vec![
        ("vg_hw_raster_masked (bindless)", hw_masked_source(bindless)),
        ("vg_hw_raster_masked (expanded)", hw_masked_source(expanded)),
        ("vg_sw_raster_masked (bindless)", sw_masked_source(bindless)),
        ("vg_sw_raster_masked (expanded)", sw_masked_source(expanded)),
        ("vg_hw_raster", hw_raster_source()),
        ("vg_sw_raster", sw_raster_source()),
        ("vg_sw_bin", sw_bin_source()),
        ("vg_tile_raster", tile_raster_source()),
        ("vg_dense_count", dense_count_source()),
        ("vg_tile_classify", tile_classify_source()),
    ]
}
