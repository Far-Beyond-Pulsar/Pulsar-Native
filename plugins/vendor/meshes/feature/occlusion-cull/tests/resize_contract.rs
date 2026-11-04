//! Regression test for a real bug: `OcclusionCullPass::set_screen_size` was
//! dead code (nothing called it), so the cull-uniform resolution it computes
//! `pick_mip`/`screen_radius_px` from stayed frozen at construction-time
//! forever while the "hiz" texture it samples (graph-pooled) DID get
//! correctly reallocated on resize. That desync corrupted mip selection and
//! caused visible geometry to be incorrectly culled after any resize.
//!
//! `OcclusionCullPass::prepare` now re-syncs from `ctx.resize`/`ctx.width`/
//! `ctx.height` every frame, mirroring `HiZBuildPass::prepare`'s own
//! resize-sync. This test drives just that pass through a real
//! `RenderGraph` (no HiZBuild/ObjectBatch needed -- `execute()` no-ops
//! gracefully when `object_batch`/`indirect_dispatch`/`coordinate_spaces`
//! are absent) and asserts `screen_size()` always matches the graph's
//! current resolution.

use std::sync::Arc;

use helio_core::RenderGraph;
use helio_pass_occlusion_cull::OcclusionCullPass;

mod support;

#[test]
fn screen_size_tracks_graph_resolution_across_resize() {
    pollster::block_on(async {
        let Some((device, queue)) =
            support::request_test_device("OcclusionCull Resize Contract").await
        else {
            eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: occlusion-cull resize contract");
            return;
        };
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let scene_input = support::TestSceneInput::new(Arc::clone(&device), Arc::clone(&queue));

        let hiz_sampler = Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Resize Contract HiZ Sampler"),
            ..Default::default()
        }));
        let cull_stats_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Resize Contract Cull Stats"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Constructed at the same resolution `graph.lock` uses below --
        // matches production (`helio-presets::add_common_early_passes`
        // always constructs `OcclusionCullPass`/`HiZBuildPass` with the exact
        // `iw, ih` that the same call site's `graph.lock(iw, ih)` uses), so
        // frame 1 is already correctly sized without needing a resize pulse
        // (`ctx.resize` is `false` on the very first frame regardless --
        // see `helio-core/tests/render_graph_resize_contract.rs`'s observed
        // `[(false, ..), (true, ..), (false, ..)]` sequence). What actually
        // matters, and what was broken, is whether a LATER live resize gets
        // picked up.
        let pass = OcclusionCullPass::new(&device, hiz_sampler, 64, 48, cull_stats_buf);

        let mut graph = RenderGraph::new(&device, &queue);
        graph.add_pass(Box::new(pass));
        graph.lock(64, 48);

        let (target, depth) = support::frame_views(&device, 64, 48);
        graph
            .execute(&scene_input, &target, &depth)
            .expect("initial graph frame must execute");
        assert_eq!(
            graph
                .find_pass::<OcclusionCullPass>()
                .expect("pass must still be in the graph")
                .screen_size(),
            (64, 48),
            "screen_size must match construction/lock resolution on the first frame"
        );

        graph.set_render_size(128, 96);
        let (target, depth) = support::frame_views(&device, 128, 96);
        graph
            .execute(&scene_input, &target, &depth)
            .expect("resized graph frame must execute");
        assert_eq!(
            graph
                .find_pass::<OcclusionCullPass>()
                .expect("pass must still be in the graph")
                .screen_size(),
            (128, 96),
            "screen_size must track a live resize instead of staying stuck at the previous resolution"
        );

        // A steady frame at the same (already-synced) resolution must not
        // desync anything either.
        graph
            .execute(&scene_input, &target, &depth)
            .expect("steady graph frame must execute");
        assert_eq!(
            graph
                .find_pass::<OcclusionCullPass>()
                .expect("pass must still be in the graph")
                .screen_size(),
            (128, 96)
        );
    });
}
