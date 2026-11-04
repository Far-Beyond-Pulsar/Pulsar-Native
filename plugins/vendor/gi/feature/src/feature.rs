//! GI as a feature: the nodes it adds to preset graphs.
//!
//! The probes' update runs before the G-buffer phase (it reads no G-buffer,
//! and a terrain drawer traces its rays while it draws); the meshes' tracer
//! joins at [`Slot::Geometry`]; the resolve at [`Slot::LightingInputs`],
//! publishing the frame's indirect diffuse. The probes' hits are lit
//! through the frame's sun transmittance when a medium published one.

use helio_feature_api::{Feature, FeatureRegistrar, NodeBuild, NodeDescriptor, ParamDesc};
use helio_frame_api::lighting::{inputs_order, sun_transmittance, InputsOrder};
use helio_frame_api::{FrameNode, Phase, Slot};

use crate::{GiMeshTracePass, GiPass, GiResolvePass, GiSettings};

/// World-space diffuse GI ([`crate`]).
pub struct GiFeature;

const CATEGORY: &str = "Passes: Lighting";

fn settings(build: &NodeBuild<'_, '_>) -> GiSettings {
    build
        .env
        .setting::<GiSettings>()
        .copied()
        .unwrap_or_default()
}

/// Whether a feature traces the probes against its terrain this graph.
fn terrain_traced(build: &NodeBuild<'_, '_>) -> bool {
    build.env.setting::<helio_gi_api::TerrainTracer>().is_some()
}

fn node(
    name: &str,
    title: &str,
    description: &str,
    build: impl Fn(&mut NodeBuild<'_, '_>) -> Result<(), String> + Send + Sync + 'static,
) -> NodeDescriptor {
    NodeDescriptor::new(
        &format!("helio.pass.{name}"),
        title,
        CATEGORY,
        description,
        build,
    )
    .with_param(ParamDesc::boolean("enabled", "Enabled", true))
    .with_generated_pins()
}

impl Feature for GiFeature {
    fn id(&self) -> &'static str {
        "helio.gi"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        registrar.node(
            node(
                "gi",
                "GI: Probe Update",
                "Integrates last frame's hits into the probe clipmap and writes this frame's rays. \
                 Builds nothing where nothing can trace them.",
                |build| {
                    let settings = settings(build);
                    let traced = terrain_traced(build);
                    if settings.active(build.env.device, traced) {
                        let sun = sun_transmittance::current(build.env);
                        let pass = GiPass::new(build.env.device, settings.config(traced))
                            .with_sun_transmittance(sun);
                        build.graph.add_pass(Box::new(pass));
                    }
                    Ok(())
                },
            )
            .before_phase(Phase::GBuffer, 10),
        );
        registrar.node(
            node(
                "gi_mesh_trace",
                "GI: Mesh Rays",
                "Traces the probes' rays against the scene's TLAS (hardware ray queries).",
                |build| {
                    if let Some(pass) = GiMeshTracePass::new(build.env.device) {
                        let sun = sun_transmittance::current(build.env);
                        build
                            .graph
                            .add_pass(Box::new(pass.with_sun_transmittance(sun)));
                    }
                    Ok(())
                },
            )
            .in_slot(Slot::Geometry, 20),
        );
        registrar.node(
            node(
                "gi_resolve",
                "GI: Resolve",
                "The probes' irradiance at every pixel (the frame's indirect diffuse) for the lighting.                  Builds nothing where the lighting ran before it (forward).",
                |build| {
                    let read = inputs_order(build.env) == InputsOrder::BeforeLighting;
                    if read && settings(build).active(build.env.device, terrain_traced(build)) {
                        build
                            .graph
                            .add_pass(Box::new(GiResolvePass::new(build.env.device)));
                    }
                    Ok(())
                },
            )
            .in_slot(Slot::LightingInputs, 0),
        );
    }
}
