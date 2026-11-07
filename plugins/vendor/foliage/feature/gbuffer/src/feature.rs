//! Foliage as a feature: placement before the G-buffer phase, the blades'
//! drawer in it, and the step that grows placement on a terrain's ground.
//! Builds nothing while [`FoliageSettings::enabled`] is off.

use std::sync::Arc;

use helio_feature_api::{missing, pass_node, Feature, FeatureRegistrar};
use helio_frame_api::{FrameNode, Phase};
use helio_pass_foliage_place::{FoliagePlacePass, FoliageRange, FoliageSettings, OfferedGrounds};

use crate::{FoliageBoxesPass, FoliageGBufferPass};

/// The foliage passes `settings` asks for: placement, the boxes expansion
/// (between placement and the G-buffer) and the G-buffer draw (after it),
/// which share the placement's buffers.
pub fn foliage_passes(
    device: &wgpu::Device,
    settings: &FoliageSettings,
) -> (FoliagePlacePass, FoliageBoxesPass, FoliageGBufferPass) {
    let defaults = FoliageRange::default();
    let place = FoliagePlacePass::new_with(
        device,
        settings.quality,
        FoliageRange {
            range: settings.reach(),
            tufts_per_m2: settings.tufts_per_m2.unwrap_or(defaults.tufts_per_m2),
            blades_per_tuft: settings.blades_per_tuft.unwrap_or(defaults.blades_per_tuft),
            ..defaults
        },
    );
    let mut draw = FoliageGBufferPass::new(
        device,
        Arc::clone(&place.blade_arena),
        Arc::clone(&place.tile_table),
        Arc::clone(&place.tile_frames),
        Arc::clone(&place.visible_blades),
        Arc::clone(&place.foliage_indirect),
        Arc::clone(&place.bands_buffer),
        Arc::clone(&place.interactors),
    );
    draw.quality = settings.quality;
    let boxes = draw.boxes_pass(device);
    (place, boxes, draw)
}

/// The foliage G-buffer draw the placement node made, for the draw node.
struct PendingFoliageDraw(Option<FoliageGBufferPass>);

/// Foliage ([`crate`] and `helio_pass_foliage_place`).
pub struct FoliageFeature;

impl Feature for FoliageFeature {
    fn id(&self) -> &'static str {
        "helio.foliage"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // Blades and tufts are placed on the GPU from the layers: no
        // acceleration structure holds them.
        registrar.untraced_geometry(helio_foliage_api::FOLIAGE_LAYERS.name());
        // What shader hot reload may swap without rebuilding the graph.
        registrar.swap_group(&[
            helio_core::graph::type_name_of::<FoliagePlacePass>(),
            helio_core::graph::type_name_of::<FoliageGBufferPass>(),
            helio_core::graph::type_name_of::<FoliageBoxesPass>(),
        ]);
        // Foliage grows on the first ground a terrain offered while the graph
        // was built (the voxel terrain's, say); with none, on its built-in one.
        registrar.finish(Arc::new(|env, graph| {
            let ground = env
                .shared_existing::<OfferedGrounds>()
                .and_then(|offered| offered.0.first().cloned());
            if let (Some(ground), Some(place)) = (ground, graph.find_pass_mut::<FoliagePlacePass>())
            {
                place.set_ground(ground);
            }
            Ok(())
        }));

        const CATEGORY: &str = "Passes: Geometry";

        registrar.node(
            pass_node(
                "foliage_place",
                "Foliage: Place Blades",
                CATEGORY,
                "Places foliage blades on the GPU, before the G-buffer phase so its drawers fuse.",
                |build| {
                    let settings = build
                        .env
                        .setting::<FoliageSettings>()
                        .cloned()
                        .unwrap_or_default();
                    if !settings.enabled {
                        return Ok(());
                    }
                    // The draw shares placement's buffers and its boxes run before the
                    // G-buffer: made here, the draw waits for the Foliage GBuffer node.
                    let (place, boxes, draw) = foliage_passes(build.env.device, &settings);
                    build.graph.add_pass(Box::new(place));
                    build.graph.add_pass(Box::new(boxes));
                    build.env.shared(|| PendingFoliageDraw(None)).0 = Some(draw);
                    Ok(())
                },
            )
            .before_phase(Phase::GBuffer, 0),
        );

        registrar.node(
            pass_node(
                "foliage_gbuffer",
                "Foliage: Draw to GBuffer",
                CATEGORY,
                "Rasterises placed foliage into the GBuffer. Needs Foliage Placement before it.",
                |build| {
                    let enabled = build
                        .env
                        .setting::<FoliageSettings>()
                        .is_none_or(|settings| settings.enabled);
                    if !enabled {
                        return Ok(());
                    }
                    let draw = build
                        .env
                        .shared_existing::<PendingFoliageDraw>()
                        .and_then(|pending| pending.0.take())
                        .ok_or_else(|| missing("Foliage GBuffer", "Foliage Placement"))?;
                    build.graph.add_pass(Box::new(draw));
                    Ok(())
                },
            )
            .drawer(Phase::GBuffer, 20),
        );
    }
}
