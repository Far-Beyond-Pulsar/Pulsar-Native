//! Whether the scene holds content that moves on its own (Pulsar-Native
//! #1123, #1109): foliage in a wind, particle emitters, surfaces whose
//! shader graph reads `time`. While the viewport is realtime such content
//! changes every frame with no scene write, so the editor renderer keeps
//! rendering instead of going idle.
//!
//! The answer depends only on the authored components, so it is
//! re-checked only when the world's revision changes, never per frame.

use helio_component::components::{
    slot_reads_time, FoliageComponent, ParticleEmitterComponent, StaticMeshComponent, WindComponent,
};
use pulsar_scene_model::attachments;
use pulsar_scenedb::World;

/// The last answer and the world revision it was computed for.
#[derive(Default)]
pub(super) struct AnimatedContent {
    checked: Option<(usize, u64)>,
    live: bool,
}

impl AnimatedContent {
    /// Whether `world` holds animated content, re-checked only when its
    /// revision (or the world itself) changed.
    pub(super) fn live(&mut self, world: &World) -> bool {
        let key = (world as *const World as usize, world.revision());
        if self.checked != Some(key) {
            self.live = animated(world);
            self.checked = Some(key);
        }
        self.live
    }
}

/// Animated content in `world`: an enabled particle emitter, foliage in a
/// wind faster than calm, or a mesh drawn with a shader graph that reads
/// the frame clock.
pub(super) fn animated(world: &World) -> bool {
    particles(world) || foliage_in_wind(world) || animated_graph_materials(world)
}

fn particles(world: &World) -> bool {
    world
        .query::<&ParticleEmitterComponent>()
        .any(|(instance, emitter)| emitter.enabled && attachments::is_enabled(world, instance))
}

/// The foliage passes' one wind (as the environment join picks it): a
/// foliage component's own wind when one opts out of the global wind, else
/// the level's global wind, else the components' own. Approximated by the
/// fastest candidate at the deciding level.
fn foliage_in_wind(world: &World) -> bool {
    let foliage: Vec<&FoliageComponent> = world
        .query::<&FoliageComponent>()
        .filter(|(instance, foliage)| {
            foliage.general.enabled && attachments::is_enabled(world, *instance)
        })
        .map(|(_, foliage)| foliage)
        .collect();
    if foliage.is_empty() {
        return false;
    }
    let own_speed = |foliage: &FoliageComponent| {
        if foliage.wind.wind_enabled {
            foliage.wind.wind_speed
        } else {
            0.0
        }
    };
    let opted_out: Vec<f32> = foliage
        .iter()
        .filter(|foliage| !foliage.wind.use_global_wind)
        .map(|foliage| own_speed(foliage))
        .collect();
    if !opted_out.is_empty() {
        return opted_out.iter().any(|speed| *speed > 0.0);
    }
    let global = world
        .query::<&WindComponent>()
        .find(|(instance, wind)| wind.enabled && attachments::is_enabled(world, *instance))
        .map(|(_, wind)| wind.speed);
    match global {
        Some(speed) => speed > 0.0,
        None => foliage.iter().any(|foliage| own_speed(foliage) > 0.0),
    }
}

fn animated_graph_materials(world: &World) -> bool {
    world
        .query::<&StaticMeshComponent>()
        .filter(|(instance, _)| attachments::is_enabled(world, *instance))
        .any(|(_, mesh)| mesh.material_slots.slots.iter().any(slot_reads_time))
}
