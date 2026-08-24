//! Demo: object references through `DYN_METHOD_REGISTRY` (#642).
//!
//! One end-to-end wiring proving a Blueprint-style call can pass an object
//! reference IN and get one BACK through the dynamic (non-`EngineClass`)
//! dispatch registry -- the same link-time registry C's dispatcher and F's
//! graph nodes consume. The receiver is the `World` itself (`World` is
//! `Any + Send + Sync`, so it slots into the `&mut dyn Any` receiver slot
//! without any wrapper); args/returns are `Box<dyn Any>` holding the
//! concrete ref types from [`crate::reflect`].
//!
//! Per the #641 contract, these closures NEVER panic on invalid input:
//! downcast misses and stale handles return `None` (the dyn-registry's
//! "no value" result) with a warning logged. Callers that need richer
//! errors use the typed accessor API instead of raw dyn dispatch.

use pulsar_reflection::{
    DynMethodArgs, DynMethodMetadata, DynMethodRegistration, MethodReturnType, MethodType,
};
use pulsar_scenedb::World;

use crate::reflect::{actor_ref_type_info, component_ref_type_info};
use crate::refs::ComponentRef;

/// Registry key for this demo receiver. Real subsystems register their own
/// names; graphs address methods by `(receiver_name, method_name)`.
pub const RECEIVER_NAME: &str = "scene_object_model";

fn methods() -> Vec<DynMethodMetadata> {
    vec![
        DynMethodMetadata {
            name: "normalize_ref",
            display_name: "Normalize Object Reference".into(),
            category: Some("Scene"),
            params: vec![pulsar_reflection::MethodParameter {
                name: "target",
                type_info: component_ref_type_info(),
            }],
            return_type: Some(MethodReturnType { type_info: component_ref_type_info() }),
            // Pure: same world state in, same reference out; no mutation.
            method_type: MethodType::Pure,
            caller: Box::new(|receiver: &mut dyn std::any::Any, args: DynMethodArgs| {
                let world = receiver.downcast_mut::<World>()?;
                let target = args.into_iter().next()?.downcast::<ComponentRef>().ok()?;
                match target.validate(world) {
                    // Re-box the VALUE (target was the arg's box): exactly
                    // one Box<dyn Any> layer out, mirroring the caller's.
                    Ok(()) => Some(Box::new(*target) as Box<dyn std::any::Any>),
                    Err(error) => {
                        tracing::warn!("normalize_ref refused {target:?}: {error}");
                        None
                    }
                }
            }),
        },
        DynMethodMetadata {
            name: "describe_ref",
            display_name: "Describe Object Reference".into(),
            category: Some("Scene"),
            params: vec![pulsar_reflection::MethodParameter {
                name: "target",
                type_info: component_ref_type_info(),
            }],
            return_type: Some(MethodReturnType {
                type_info: actor_ref_type_info(),
            }),
            method_type: MethodType::Pure,
            caller: Box::new(|receiver: &mut dyn std::any::Any, args: DynMethodArgs| {
                let world = receiver.downcast_mut::<World>()?;
                let target = args.into_iter().next()?.downcast::<ComponentRef>().ok()?;
                let actor = target.actor();
                if actor.validate(world).is_err() {
                    tracing::warn!("describe_ref: {target:?} is dead");
                    return None;
                }
                let alive = if target.is_valid(world) { "live" } else { "unhydrated" };
                Some(Box::new(format!(
                    "{}@{} ({alive}, index {})",
                    target.class_name,
                    actor.entity(),
                    target.component_index
                )))
            }),
        },
    ]
}

inventory::submit! {
    DynMethodRegistration {
        receiver_name: RECEIVER_NAME,
        methods,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refs::ActorRef;
    use crate::test_support::TestGizmo;
    use pulsar_reflection::DYN_METHOD_REGISTRY;

    /// #642 acceptance shape: a graph passes an object reference INTO a
    /// reflected method and RECEIVES one back, via the real global registry.
    #[test]
    fn component_refs_round_trip_through_dyn_dispatch() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(ActorRef::new(e), "TestGizmo");

        let returned = DYN_METHOD_REGISTRY
            .invoke(RECEIVER_NAME, "normalize_ref", &mut world as &mut dyn std::any::Any, vec![
                Box::new(r.clone())
            ])
            .expect("method registered")
            .expect("valid ref normalizes to itself");
        assert_eq!(returned.downcast_ref::<ComponentRef>(), Some(&r));
    }

    /// The never-panic contract holds at the dyn boundary too: a stale ref
    /// yields `Ok(None)` (refused + warned), not a panic; an arg of the
    /// wrong TYPE also degrades to `None`.
    #[test]
    fn invalid_inputs_degrade_to_none_not_panics() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(ActorRef::new(e), "TestGizmo");
        r.actor().despawn(&mut world);

        let result = DYN_METHOD_REGISTRY
            .invoke(RECEIVER_NAME, "normalize_ref", &mut world as &mut dyn std::any::Any, vec![
                Box::new(r)
            ])
            .expect("registered");
        assert!(result.is_none(), "stale ref must be refused, not dispatched");

        // Wrong argument type: clean None.
        let result = DYN_METHOD_REGISTRY
            .invoke(RECEIVER_NAME, "normalize_ref", &mut world as &mut dyn std::any::Any, vec![
                Box::new(7i32)
            ])
            .expect("registered");
        assert!(result.is_none());
    }

    /// describe_ref produces the human-readable form F's editor surfaces
    /// show for pins ("class@Entity (live, index n)").
    #[test]
    fn describe_ref_names_the_target_for_editor_surfaces() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TestGizmo { charges: 1 });
        let r = ComponentRef::live(ActorRef::new(e), "TestGizmo");

        let described = DYN_METHOD_REGISTRY
            .invoke(RECEIVER_NAME, "describe_ref", &mut world as &mut dyn std::any::Any, vec![
                Box::new(r)
            ])
            .expect("registered")
            .expect("live target describes");
        let text = described.downcast_ref::<String>().unwrap();
        assert!(text.starts_with("TestGizmo@"), "got {text}");
        assert!(text.contains("(live, index 0)"), "got {text}");
    }
}

