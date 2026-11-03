//! The voxel feature's host settings, described once (as
//! `helio_settings` describes the renderer's): engines build their settings
//! UI and read the values back through it.

use helio_settings::{integer, number, Group};

/// Voxel worlds' residency: which chunks are generated and kept, and the
/// time generation gets ([`crate::chunks::ResidencyConfig`]): a host lists
/// it with its other settings groups (`helio_settings::GROUPS`).
pub static VOXEL_RESIDENCY: Group = Group {
    key: "voxels",
    title: "Voxels",
    description: "Voxel world detail, streaming and generation budgets",
    settings: &[
        number(
            "lod_pixels",
            "Detail (pixels a cell)",
            "Detail",
            (0.25, 16.0, 0.25),
            1.0,
            "Pixels a voxel may cover on screen before a finer level is wanted",
        ),
        number(
            "offscreen_pixels",
            "Off-screen Detail",
            "Detail",
            (0.25, 64.0, 0.25),
            2.0,
            "Pixels a cell may cover out of view",
        ),
        number(
            "unseen_pixels",
            "Hidden Detail",
            "Detail",
            (1.0, 64.0, 0.25),
            2.0,
            "Pixels a cell may cover where no ray met it lately (1: rays decide nothing)",
        ),
        number("near_cells", "Near Cells", "Detail", (0.0, 16.0, 0.5), 2.0, "Cells around the eye kept at full detail"),
        number(
            "coarsen",
            "Coarsen Threshold",
            "Detail",
            (0.1, 1.0, 0.05),
            0.8,
            "Fraction of the detail target below which a level coarsens (hysteresis)",
        ),
        number("view_guard", "View Guard", "Detail", (0.0, 1.5, 0.05), 0.35, "Radians past the view's edge still counted in view"),
        number(
            "view_distance_km",
            "View Distance (km)",
            "Detail",
            (1.0, 100_000.0, 1.0),
            400.0,
            "Kilometres chunks are planned out to",
        ),
        number(
            "motion_flow",
            "Motion Flow",
            "Motion",
            (0.0, 256.0, 1.0),
            16.0,
            "Pixels a frame of motion per extra pixel a cell may cover (0: motion changes nothing)",
        ),
        number("motion_max", "Motion Max", "Motion", (1.0, 16.0, 0.25), 3.0, "Most a cell's pixels grow with motion"),
        number(
            "prefetch_seconds",
            "Prefetch",
            "Motion",
            (0.0, 8.0, 0.1),
            0.5,
            "Seconds ahead along the motion that chunks are planned",
        ),
        integer("seen_frames", "Seen Frames", "Streaming", (1, 10_000), 90, "Frames a ray's visit counts"),
        integer(
            "keep_frames",
            "Keep Frames",
            "Streaming",
            (1, 100_000),
            120,
            "Frames a chunk out of the plan stays before it may be replaced",
        ),
        integer(
            "release_frames",
            "Release Frames",
            "Streaming",
            (1, 100_000),
            600,
            "Frames unvisited before a chunk's memory is released",
        ),
        integer(
            "footprints_per_frame",
            "Footprints per Frame",
            "Generation",
            (1, 65_536),
            256,
            "Chunk footprints generated a frame at most",
        ),
        number(
            "generate_ms",
            "Generation Budget (ms)",
            "Generation",
            (0.0, 100.0, 0.25),
            3.0,
            "GPU milliseconds of generation a frame at 60 fps; 0: unbounded",
        ),
        number(
            "frame_ms",
            "Frame Budget (ms)",
            "Generation",
            (0.0, 1000.0, 0.5),
            16.0,
            "Milliseconds a frame may take: generation grows into what the frame leaves; 0: no growth",
        ),
    ],
};
