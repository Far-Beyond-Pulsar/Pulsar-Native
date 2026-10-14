//! Whether the scene holds content that moves on its own (Pulsar-Native
//! #1123, #1109): foliage in a wind, particle emitters, water (#1065),
//! surfaces whose shader graph reads `time`. While the viewport is realtime such content
//! changes every frame with no scene write, so the editor renderer keeps
//! rendering instead of going idle.
//!
//! The answer depends only on the authored components, so it is
//! re-checked only when one of the components it reads changed (through
//! SceneDB change cursors), never per frame, and not when an object merely
//! moves: a drag changes the world every frame (Pulsar-Native#995), and the
//! check walks every mesh's material slots.

use helio_component::components::{
    mesh_reads_time, FoliageComponent, ParticleEmitterComponent, StaticMeshComponent,
    WaterVolumeComponent, WindComponent,
};
use pulsar_scene_model::attachments;
use pulsar_scenedb::{ChangeCursor, ChangeRead, World};

/// The last answer, and cursors on the components it was computed from.
#[derive(Default)]
pub(super) struct AnimatedContent {
    /// The world the cursors are on, and its revision at the last check.
    world: Option<(usize, u64)>,
    cursors: Vec<ChangeCursor>,
    live: bool,
    scratch: Vec<pulsar_scenedb::ComponentChange>,
    /// How many times the full check ran.
    #[cfg(test)]
    checks: usize,
}

impl AnimatedContent {
    /// Whether `world` holds animated content, re-checked only when a
    /// component the answer reads changed (or the world itself did).
    pub(super) fn live(&mut self, world: &World) -> bool {
        let id = world as *const World as usize;
        let fresh = match self.world {
            Some((last, revision)) => last != id || world.revision() < revision,
            None => true,
        };
        if fresh {
            self.cursors = vec![
                world.open_change_cursor::<ParticleEmitterComponent>(),
                world.open_change_cursor::<FoliageComponent>(),
                world.open_change_cursor::<WaterVolumeComponent>(),
                world.open_change_cursor::<WindComponent>(),
                world.open_change_cursor::<StaticMeshComponent>(),
                world.open_change_cursor::<attachments::ComponentOwner>(),
            ];
            self.live = self.check(world);
        } else {
            // Cheap: a cursor read costs the changes since the last one,
            // and moving an object adds none to these journals.
            let mut changed = false;
            for cursor in &mut self.cursors {
                self.scratch.clear();
                changed |= world.read_changes(cursor, &mut self.scratch) == ChangeRead::Overflowed
                    || !self.scratch.is_empty();
            }
            if changed {
                self.live = self.check(world);
            }
        }
        self.world = Some((id, world.revision()));
        self.live
    }

    fn check(&mut self, world: &World) -> bool {
        #[cfg(test)]
        {
            self.checks += 1;
        }
        animated(world)
    }
}

/// Animated content in `world`: an enabled particle emitter, foliage in a
/// wind faster than calm, an enabled water volume, or a mesh drawn with a
/// shader graph that reads the frame clock.
pub(super) fn animated(world: &World) -> bool {
    particles(world) || foliage_in_wind(world) || water(world) || animated_graph_materials(world)
}

fn particles(world: &World) -> bool {
    world
        .query::<&ParticleEmitterComponent>()
        .any(|(instance, emitter)| emitter.enabled && attachments::is_enabled(world, instance))
}

/// Water simulates on the frame clock: its waves travel in a wind, and
/// ripples from bodies moving through it (#1080) spread and settle over
/// many frames after the edit that made them. Any enabled volume counts.
fn water(world: &World) -> bool {
    world
        .query::<&WaterVolumeComponent>()
        .any(|(instance, water)| water.enabled && attachments::is_enabled(world, instance))
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
        .any(|(_, mesh)| mesh_reads_time(mesh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::{world_ext::SceneWorldExt, SpawnObject, Transform};

    #[test]
    fn moving_objects_does_not_recheck_and_component_edits_do() {
        let mut world = World::new();
        let object = world.spawn_object(SpawnObject::new("lake")).unwrap();
        let mut content = AnimatedContent::default();
        assert!(!content.live(&world));
        assert_eq!(content.checks, 1);

        // A drag (Pulsar-Native#995): the world changes every frame, the
        // answer cannot, and the slots are not walked again.
        for x in 0..10 {
            world.get_mut::<Transform>(object).unwrap().position = [x as f32, 0.0, 0.0];
            assert!(!content.live(&world));
        }
        assert_eq!(content.checks, 1);

        let water = pulsar_world_registry::attach_value(
            &mut world,
            object,
            WaterVolumeComponent::default(),
        )
        .unwrap();
        assert!(content.live(&world), "an enabled water volume animates");
        assert_eq!(content.checks, 2);

        world.get_mut::<WaterVolumeComponent>(water).unwrap().enabled = false;
        assert!(!content.live(&world), "a disabled one does not");

        // A new world is checked from scratch.
        let other = World::new();
        assert!(!content.live(&other));
    }
}
