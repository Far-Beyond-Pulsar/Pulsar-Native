//! Monumental stone arch: interactive orbit viewer or deterministic captures.
mod architectural_mesh;
mod hlfs_capture;
mod hlfs_viewer;
mod monumental_arch;
mod v3_demo_common;
pub(crate) fn main() {
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
            "monument",
            monumental_arch::populate,
            monumental_arch::camera,
        );
    } else {
        hlfs_viewer::run(hlfs_viewer::Scene {
            name: "Monumental Arch",
            populate: Box::new(monumental_arch::populate),
            update: None,
            key: None,
            camera: monumental_arch::camera,
            orbit_target: glam::Vec3::new(0.0, 24.0, 0.0),
        });
    }
}
