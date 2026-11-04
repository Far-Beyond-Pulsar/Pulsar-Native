//! naga parse + validate of every virtual-geometry raster module, with and
//! without the 64-bit atomic capabilities.

use helio_pass_vg_raster::shaders;
use naga::valid::{Capabilities, ValidationFlags, Validator};

fn validate(label: &str, src: &str, caps: Capabilities) -> Result<naga::Module, String> {
    let module = naga::front::wgsl::parse_str(src)
        .map_err(|e| format!("{label}: parse: {}", e.emit_to_string(src)))?;
    Validator::new(ValidationFlags::all(), caps)
        .validate(&module)
        .map_err(|e| format!("{label}: validate: {e:?}"))?;
    Ok(module)
}

fn base_caps() -> Capabilities {
    // What the Tier 1 / fallback modules may rely on: nothing 64-bit.
    Capabilities::empty()
}

#[test]
fn modules_without_int64_validate_with_no_extra_capabilities() {
    for (label, src) in shaders::all_sources() {
        if label.starts_with("vg_sw_raster") || label.contains("(bindless)") {
            continue; // u64 atomics / binding arrays need capabilities, see below
        }
        validate(label, &src, base_caps()).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn sw_raster_requires_exactly_the_int64_min_max_capabilities() {
    let src = shaders::sw_raster_source();
    // Not valid without the capabilities...
    assert!(validate("sw", &src, base_caps()).is_err());
    // ...valid with MIN_MAX only (no ALL_OPS: the atomicMax result is unused).
    let caps = Capabilities::SHADER_INT64 | Capabilities::SHADER_INT64_ATOMIC_MIN_MAX;
    validate("sw", &src, caps).unwrap_or_else(|e| panic!("{e}"));
    // And with everything.
    validate("sw", &src, Capabilities::all()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn all_modules_validate_with_all_capabilities() {
    for (label, src) in shaders::all_sources() {
        validate(label, &src, Capabilities::all()).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn entry_points_exist() {
    let expect = [
        ("vg_hw_raster", vec!["vs_main", "fs_main"]),
        ("vg_sw_raster", vec!["sw_raster_main"]),
        ("vg_sw_bin", vec!["sw_bin_main"]),
        ("vg_tile_raster", vec!["tile_raster_main"]),
        ("vg_tile_classify", vec!["classify_main", "dense_args_main"]),
        ("vg_dense_count", vec!["dense_count_main"]),
        (
            "vg_hw_raster_masked (bindless)",
            vec!["vs_masked", "fs_masked"],
        ),
        (
            "vg_hw_raster_masked (expanded)",
            vec!["vs_masked", "fs_masked"],
        ),
        (
            "vg_sw_raster_masked (bindless)",
            vec!["sw_raster_masked_main"],
        ),
        (
            "vg_sw_raster_masked (expanded)",
            vec!["sw_raster_masked_main"],
        ),
    ];
    for (label, src) in shaders::all_sources() {
        let m = validate(label, &src, Capabilities::all()).unwrap();
        let names: Vec<_> = m.entry_points.iter().map(|e| e.name.as_str()).collect();
        for want in &expect.iter().find(|(l, _)| *l == label).unwrap().1 {
            assert!(names.contains(want), "{label} lacks {want}: {names:?}");
        }
    }
}

/// The WGSL structs the CPU mirrors rely on keep their sizes.
#[test]
fn abi_struct_sizes_match() {
    use helio_pass_vg_raster::abi::*;
    let src = format!(
        "{}\n{}\n@compute @workgroup_size(1) fn main() {{}}",
        shaders::FORMAT,
        shaders::BINDINGS
    );
    let module = validate("abi", &src, Capabilities::all()).unwrap();
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).unwrap();
    let size = |name: &str| {
        let (h, _) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap();
        layouter[h].size as usize
    };
    assert_eq!(
        size("VgVisibleCluster"),
        std::mem::size_of::<helio_vg_format::GpuVisibleCluster>()
    );
    assert_eq!(size("VgObject") as u64, OBJECT_STRIDE);
    assert_eq!(
        size("VgRasterParams"),
        std::mem::size_of::<VgRasterParams>()
    );
    assert_eq!(
        size("VgCamera"),
        std::mem::size_of::<helio_core::GpuCameraUniforms>()
    );
}
