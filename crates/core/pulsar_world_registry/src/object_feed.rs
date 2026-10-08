//! A view's feed of one object's changes, over SceneDB object subscriptions
//! (Pulsar-Native#1035, Phase 6).
//!
//! Data flows from an edit straight into the `World`; views that display an
//! object follow writes made elsewhere (a gizmo drag, a script, an undo)
//! through this feed instead of polling. Renderers and other bulk readers do
//! not use it: they read the `World` (or the GPU mirror) directly.
//!
//! [`ObjectFeed::subscribe`] watches one object: the object entity and its
//! component instances, including ones attached later
//! ([`pulsar_scene_model::attachments::object_of`] is installed as the
//! world's object resolver). Inside each write, SceneDB hands the
//! subscription the component's full new value; the feed clones it (a
//! `GpuHeavy<T>` field clones only its reference), queues an
//! [`ObjectUpdate`], and calls the caller's `wake`. The view then takes the
//! queued updates on its own thread and applies the values it was given,
//! without reading the `World` again.

use std::any::Any;
use std::sync::{Arc, Mutex};

use pulsar_scenedb::{
    ComponentChangeKind, ComponentId, Entity, ObjectEvent, SubscriptionId, World,
};

/// One change to a component of the watched object.
pub struct ObjectDelta {
    /// The entity holding the component: the object itself, or one of its
    /// component instances.
    pub entity: Entity,
    pub component: ComponentId,
    pub kind: ComponentChangeKind,
    /// The component's full value after the write, or the removed value.
    /// `None` for a component type the feed cannot copy (no class
    /// registration and not a scene-model type), and for despawn removals.
    pub value: Option<Box<dyn Any + Send + Sync>>,
}

pub enum ObjectUpdate {
    Changed(ObjectDelta),
    /// The object despawned; the feed has ended.
    Despawned,
}

/// A subscription to one object, delivering [`ObjectUpdate`]s to a queue
/// the owner drains.
pub struct ObjectFeed {
    object: Entity,
    id: SubscriptionId,
    updates: Arc<Mutex<Vec<ObjectUpdate>>>,
}

impl ObjectFeed {
    /// Watch `object`. `wake` runs inside the write that queued an update
    /// (on whichever thread wrote), so it must only signal: send on a
    /// channel, set a flag. `None` for a dead `object`.
    pub fn subscribe(
        world: &mut World,
        object: Entity,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Option<Self> {
        world.set_object_resolver(pulsar_scene_model::attachments::object_of);
        let updates = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::clone(&updates);
        let id = world.subscribe_object(object, move |_, event| {
            let update = match event {
                ObjectEvent::Despawned => ObjectUpdate::Despawned,
                ObjectEvent::Changed(change) => ObjectUpdate::Changed(ObjectDelta {
                    entity: change.entity,
                    component: change.component,
                    kind: change.kind,
                    value: change
                        .value
                        .and_then(|value| clone_component_value(change.component, value)),
                }),
            };
            queue.lock().unwrap_or_else(|p| p.into_inner()).push(update);
            wake();
        })?;
        Some(Self {
            object,
            id,
            updates,
        })
    }

    /// The watched object.
    pub fn object(&self) -> Entity {
        self.object
    }

    /// Every update queued since the last call, oldest first.
    pub fn take(&self) -> Vec<ObjectUpdate> {
        std::mem::take(&mut *self.updates.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Stop watching. Dropping the feed without this leaves the SceneDB
    /// subscription queuing into a buffer nobody reads until the object
    /// despawns.
    pub fn unsubscribe(self, world: &mut World) {
        world.unsubscribe_object(self.id);
    }
}

/// Clone a component value the feed can copy: any registered component
/// class, and the scene model's object components (transform, name,
/// visibility).
pub fn clone_component_value(
    component: ComponentId,
    value: &dyn Any,
) -> Option<Box<dyn Any + Send + Sync>> {
    use pulsar_scene_model::{Name, Transform, Visibility};
    fn copy<T: Clone + Send + Sync + 'static>(
        value: &dyn Any,
    ) -> Option<Box<dyn Any + Send + Sync>> {
        value
            .downcast_ref::<T>()
            .map(|v| Box::new(v.clone()) as Box<dyn Any + Send + Sync>)
    }
    if let Some(registration) = crate::find_by_component_id(component) {
        return (registration.clone_value)(value);
    }
    copy::<Transform>(value)
        .or_else(|| copy::<Name>(value))
        .or_else(|| copy::<Visibility>(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::attachments::{spawn_instance, NewInstance};
    use pulsar_scene_model::Transform;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn watch(world: &mut World, object: Entity) -> (ObjectFeed, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&wakes);
        let feed = ObjectFeed::subscribe(world, object, move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        (feed, wakes)
    }

    fn values<T: Clone + 'static>(updates: &[ObjectUpdate]) -> Vec<T> {
        updates
            .iter()
            .filter_map(|u| match u {
                ObjectUpdate::Changed(d) => d.value.as_ref()?.downcast_ref::<T>().cloned(),
                ObjectUpdate::Despawned => None,
            })
            .collect()
    }

    #[test]
    fn a_transform_written_elsewhere_arrives_with_its_value() {
        let mut world = World::new();
        let object = world.spawn();
        world.insert(object, Transform::default());
        let (feed, wakes) = watch(&mut world, object);

        world.get_mut::<Transform>(object).unwrap().position = [1.0, 2.0, 3.0];
        let updates = feed.take();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(values::<Transform>(&updates)[0].position, [1.0, 2.0, 3.0]);
        assert!(feed.take().is_empty());
    }

    #[test]
    fn a_component_instance_arrives_with_its_full_value() {
        let mut world = World::new();
        let object = world.spawn();
        let other = world.spawn();
        let (feed, _) = watch(&mut world, object);

        let instance =
            spawn_instance(&mut world, object, NewInstance::new("TestComponent")).unwrap();
        world.insert(instance, crate::tests::TestComponent { value: 4 });
        world
            .get_mut::<crate::tests::TestComponent>(instance)
            .unwrap()
            .value = 5;
        // Another object's instance stays out of this feed.
        let foreign = spawn_instance(&mut world, other, NewInstance::new("TestComponent")).unwrap();
        world.insert(foreign, crate::tests::TestComponent { value: 9 });

        let got: Vec<i32> = values::<crate::tests::TestComponent>(&feed.take())
            .into_iter()
            .map(|c| c.value)
            .collect();
        assert_eq!(got, [4, 5]);
    }

    #[test]
    fn despawn_ends_the_feed_and_unsubscribe_stops_it() {
        let mut world = World::new();
        let object = world.spawn();
        let (feed, _) = watch(&mut world, object);
        world.despawn(object);
        assert!(matches!(feed.take().last(), Some(ObjectUpdate::Despawned)));

        let object = world.spawn();
        world.insert(object, Transform::default());
        let (feed, wakes) = watch(&mut world, object);
        feed.unsubscribe(&mut world);
        world.get_mut::<Transform>(object).unwrap().scale = [2.0; 3];
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
    }
}
