// Sublevel placement smoke example; see `helio_mesh_api::SubLevelActorRow`.
#[path = "render_v2_basic.rs"]
mod scene_db_demo;

fn main() {
    let actor = helio_mesh_api::SubLevelActorRow::new(1, glam::Mat4::IDENTITY);
    assert!(actor.is_enabled());
    scene_db_demo::main();
}
