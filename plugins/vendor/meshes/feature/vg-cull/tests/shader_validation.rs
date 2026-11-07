//! naga parse + validate of the fully expanded cull shader, and the struct
//! sizes the Rust side uploads.

use helio_pass_vg_cull::{validation_sources, BIN_COUNT, MAX_BIN_CAPACITY};
use helio_vg_format::MAX_VISIBLE_CLUSTERS;

fn module() -> naga::Module {
    let (_, src) = validation_sources().into_iter().next().unwrap();
    naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)))
}

#[test]
fn cull_shader_validates() {
    for (name, src) in validation_sources() {
        let m = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for entry in [
            "cs_density",
            "cs_seed",
            "cs_round_prepare",
            "cs_traverse",
            "cs_finalize",
        ] {
            assert!(
                m.entry_points.iter().any(|e| e.name == entry),
                "missing {entry}"
            );
        }
    }
}

#[test]
fn shader_struct_sizes_match_the_uploads() {
    let m = module();
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(m.to_ctx()).unwrap();
    let size = |name: &str| {
        let (h, _) = m
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        layouter[h].size
    };
    assert_eq!(size("CullParams"), 80);
    assert_eq!(size("VgBvhNode"), 32);
    assert_eq!(size("VgBvhHeader"), 128);
    assert_eq!(size("RoundParams"), 16);
    // Scene-join row: 63 words, only `transform` (word 4) is read.
    assert_eq!(size("VgObjectRow"), 252);
}

#[test]
fn vis_id_can_address_every_bin() {
    assert!(BIN_COUNT * MAX_BIN_CAPACITY <= MAX_VISIBLE_CLUSTERS);
}
