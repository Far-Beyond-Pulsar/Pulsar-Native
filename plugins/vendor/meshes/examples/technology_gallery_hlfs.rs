mod architectural_mesh;
mod hlfs_capture;
mod hlfs_viewer;
mod technology_gallery;
mod v3_demo_common;
fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    if let Some(argument) = args.next() {
        assert_eq!(
            argument, "--capture",
            "use --capture <directory>, or no arguments for the interactive viewer"
        );
        let directory = args.next().expect("capture output directory");
        hlfs_capture::run_scene(
            &directory,
            "technology",
            technology_gallery::populate,
            technology_gallery::camera,
        );
    } else {
        hlfs_viewer::run(hlfs_viewer::Scene {
            name: "Technology light gallery",
            populate: Box::new(technology_gallery::populate),
            update: None,
            key: None,
            camera: technology_gallery::camera,
            orbit_target: glam::Vec3::new(0.0, 3.0, -18.0),
        });
    }
}
