//! The meshes' scene join compiles as its derivation builds it (the shared
//! join declarations, then the meshes' join): validated without a device.

use naga::valid::{Capabilities, ValidationFlags, Validator};

#[test]
fn the_mesh_join_parses_and_validates() {
    let source = helio_pass_object_batch::scene_join::mesh_join_source();
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("scene_join_meshes.wgsl:\n{}", e.emit_to_string(&source)));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("scene_join_meshes.wgsl: {e:?}"));
}
