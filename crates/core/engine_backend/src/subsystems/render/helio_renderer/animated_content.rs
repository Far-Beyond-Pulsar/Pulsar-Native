//! Whether the scene holds content that moves on its own (Pulsar-Native
//! #1123, #1109): surfaces whose shader graph reads `time`, and what
//! plugins register ([`AnimatedContentRegistration`]: particle emitters,
//! foliage in a wind, water). While the viewport is realtime such
//! content changes every frame with no scene write, so the editor renderer
//! keeps rendering instead of going idle.
//!
//! The answer depends only on the authored components, so it is
//! re-checked only when one of the components it reads changed (through
//! SceneDB change cursors), never per frame, and not when an object merely
//! moves: a drag changes the world every frame (Pulsar-Native#995), and the
//! check walks every mesh's material slots.

use helio_component::components::{mesh_reads_time, StaticMeshComponent};
use pulsar_scene_model::attachments;
use pulsar_scenedb::{ChangeCursor, ChangeRead, World};

/// A plugin's content that moves on its own (a particle emitter): `watch`
/// opens a change cursor on the component the answer reads, and `animated`
/// says whether `world` holds such content now. Submitted by the plugin's
/// component crate; an answer that reads several components submits one
/// registration for each.
pub struct AnimatedContentRegistration {
    pub watch: fn(&World) -> ChangeCursor,
    pub animated: fn(&World) -> bool,
}

inventory::collect!(AnimatedContentRegistration);

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
                world.open_change_cursor::<StaticMeshComponent>(),
                world.open_change_cursor::<attachments::ComponentOwner>(),
            ];
            self.cursors.extend(
                inventory::iter::<AnimatedContentRegistration>
                    .into_iter()
                    .map(|registration| (registration.watch)(world)),
            );
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

/// Animated content in `world`: a mesh drawn with a shader graph that
/// reads the frame clock, or a plugin's animated content.
pub(super) fn animated(world: &World) -> bool {
    animated_graph_materials(world)
        || inventory::iter::<AnimatedContentRegistration>
            .into_iter()
            .any(|registration| (registration.animated)(world))
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
            world.get_mut::<Transform>(object).unwrap().position = [f64::from(x), 0.0, 0.0];
            assert!(!content.live(&world));
        }
        assert_eq!(content.checks, 1);

        // An edit of a component the answer reads checks again.
        pulsar_world_registry::attach_value(&mut world, object, StaticMeshComponent::default())
            .unwrap();
        assert!(!content.live(&world), "a mesh without a timed graph is still");
        assert_eq!(content.checks, 2);

        // A new world is checked from scratch.
        let other = World::new();
        assert!(!content.live(&other));
    }
}
