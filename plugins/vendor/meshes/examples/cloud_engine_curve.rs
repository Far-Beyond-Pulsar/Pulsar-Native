// Cloud Engine's autonomous Bézier brush variant.
// Keep the rendering implementation shared with the base example.
#[path = "cloud_engine.rs"]
mod cloud_engine;

fn main() {
    cloud_engine::run(true);
}
