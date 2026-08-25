//! Shared object-reference resolution for blueprint graphs (#654).
//!
//! ONE implementation consumed by BOTH compile targets: the VM's comp-op
//! trampolines (`blueprint_runtime::component_ops`) and PBGC-generated Rust
//! actors, whose emitted source calls these functions by fully-qualified
//! path. Identity handling rides B's object-model types
//! (`pulsar_script_object_model`) — never re-derived here — and world-level
//! StableId/Name lookups come from `engine_backend::scene`, the one home of
//! those component definitions.
//!
//! # JSON domain
//!
//! Graph values travel as JSON blobs (D1/D4), so this module is JSON in/out.
//! Reference shapes are exactly the #642 marshalling rules:
//!
//! * `ActorRef` → packed-bits number (`Entity::bits()`);
//! * `ComponentRef` → object with `entity` (bits), `class_name`,
//!   `component_index`.
//!
//! A parity test below pins these shapes against B4's registry serializers,
//! so graph blobs and reflection-marshalled values stay interchangeable.
//!
//! # Failure contract (#641/#B3)
//!
//! Every function validates before it resolves and logs the TYPED error
//! (`ScriptRefError` / `ResolveRefError`) under the calling node's context
//! label, degrading to JSON `Null`. No panics on stale, lost, or malformed
//! references — a broken reference must never take down the tick loop.

use pulsar_script_object_model::{ActorRef, ComponentRef, ResolveRefError, ScriptRefError};
use pulsar_scenedb::{Entity, World};
use serde_json::{json, Value};

/// Build a `ComponentRef` JSON for `(actor, class_name, component_index)`,
/// validating actor liveness first. Null + typed log when the actor is gone.
pub fn component_ref_json(
    world: &World,
    actor: Entity,
    class_name: &str,
    component_index: u32,
    context: &str,
) -> Value {
    let actor = ActorRef::new(actor);
    if let Err(error) = actor.validate(world) {
        log_typed(context, &error);
        return Value::Null;
    }
    let reference = actor.component(class_name, component_index);
    json!({
        "entity": reference.entity.bits(),
        "class_name": reference.class_name,
        "component_index": reference.component_index,
    })
}

/// Resolve an authored reference literal to its CURRENT entity and emit the
/// ComponentRef JSON with live bits (#639 lazy resolution — literals stage
/// stable ids precisely so reloads/undo-redo keep them meaningful).
pub fn object_literal_json(
    world: &World,
    stable_id: &str,
    class_name: &str,
    component_index: u32,
    context: &str,
) -> Value {
    match resolve_stable_id(world, stable_id) {
        Ok(entity) => json!({
            "entity": entity.bits(),
            "class_name": class_name,
            "component_index": component_index,
        }),
        Err(error) => {
            log_typed(context, &error);
            Value::Null
        }
    }
}

/// Resolve a scene object by its StableId; emits an `ActorRef` (packed-bits
/// number). The needle arrives in the graph's JSON domain (a string).
pub fn find_object_by_stable_id(world: &World, needle: &Value, context: &str) -> Value {
    find_object(world, needle, context, |world, id| {
        engine_backend::scene::entity_with_stable_id(world, id)
    })
}

/// Resolve a scene object by display name; emits an `ActorRef`
/// (packed-bits number). First name match wins (see
/// `engine_backend::scene::first_entity_named`).
pub fn find_object_by_name(world: &World, needle: &Value, context: &str) -> Value {
    find_object(world, needle, context, |world, name| {
        engine_backend::scene::first_entity_named(world, name)
    })
}

fn find_object(
    world: &World,
    needle: &Value,
    context: &str,
    lookup: impl Fn(&World, &str) -> Option<Entity>,
) -> Value {
    let Some(needle_text) = needle.as_str() else {
        log_typed(
            context,
            &ScriptRefError::Marshalling {
                context: context.to_string(),
                message: "resolver needle is not a string".to_string(),
            },
        );
        return Value::Null;
    };
    match lookup(world, needle_text) {
        Some(entity) => json!(entity.bits()),
        None => {
            let lost = ResolveRefError::ReferenceLost { stable_id: needle_text.to_string() };
            log_typed(context, &lost);
            Value::Null
        }
    }
}

/// Turn a wired `component_ref` pin value into the `(entity,
/// component_index)` a comp op should address, refusing references whose
/// class does not match the op (#519 discipline: a Light ref can never land
/// a Door edit). `None` after logging — callers degrade to null/skip.
pub fn resolve_pin_target(
    world: &World,
    reference: &Value,
    expected_class: &str,
    context: &str,
) -> Option<(Entity, u32)> {
    // A DANGLING bit pattern must never reach a validated accessor (B3's
    // debug assert treats that as raw-id abuse), so screen it during parse.
    let parsed = parse_component_ref(reference);
    let component_ref = match parsed {
        Ok(reference) => reference,
        Err(error) => {
            log_typed(context, &error);
            return None;
        }
    };
    if component_ref.class_name != expected_class {
        let mismatch = ScriptRefError::ClassMismatch {
            expected: expected_class.to_string(),
            found: component_ref.class_name.clone(),
            component_index: component_ref.component_index,
            entity: component_ref.entity,
        };
        log_typed(context, &mismatch);
        return None;
    }
    if let Err(error) = component_ref.validate(world) {
        log_typed(context, &error);
        return None;
    }
    Some((component_ref.entity, component_ref.component_index))
}

fn parse_component_ref(reference: &Value) -> Result<ComponentRef, ScriptRefError> {
    const BAD: &str = "expected a ComponentRef ({entity, class_name, component_index})";
    let object = reference.as_object().ok_or_else(|| marshalled(BAD))?;
    let bits = object
        .get("entity")
        .and_then(Value::as_u64)
        .ok_or_else(|| marshalled(BAD))?;
    if bits == Entity::DANGLING.bits() {
        return Err(marshalled("reference carries the DANGLING entity sentinel"));
    }
    let class_name = object
        .get("class_name")
        .and_then(Value::as_str)
        .ok_or_else(|| marshalled(BAD))?
        .to_string();
    let component_index = object
        .get("component_index")
        .and_then(Value::as_u64)
        .ok_or_else(|| marshalled(BAD))?;
    u32::try_from(component_index)
        .map(|index| ComponentRef { entity: Entity::from_bits(bits), class_name, component_index: index })
        .map_err(|_| marshalled("component_index does not fit u32"))
}

fn marshalled(message: &str) -> ScriptRefError {
    ScriptRefError::Marshalling {
        context: "component_ref operand".to_string(),
        message: message.to_string(),
    }
}

fn resolve_stable_id(world: &World, stable_id: &str) -> Result<Entity, ResolveRefError> {
    engine_backend::scene::entity_with_stable_id(world, stable_id)
        .ok_or_else(|| ResolveRefError::ReferenceLost { stable_id: stable_id.to_string() })
}

fn log_typed(context: &str, error: impl std::fmt::Display) {
    tracing::error!("blueprint {context}: {error}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::scene::WorldSceneStore;
    use pulsar_reflection::RUNTIME_TYPE_REGISTRY;

    /// A two-object scene shaped like an editor-hydrated level (`spawn`
    /// attaches StableId + Name exactly like hydration does).
    fn scene() -> (WorldSceneStore, Entity, Entity) {
        let mut store = WorldSceneStore::new();
        let door = store.spawn(Some("door".into()), "Front Door", None).expect("spawn door");
        let lamp = store.spawn(Some("lamp".into()), "Red Lamp", None).expect("spawn lamp");
        (store, door, lamp)
    }

    /// #654/#642 parity: graph-domain reference JSON is byte-identical to
    /// what B4's registry serializers produce for the same values.
    #[test]
    fn json_shapes_match_the_reflection_marshalling_rules() {
        let (store, door, _lamp) = scene();
        let world = store.world();

        let actor = ActorRef::new(door);
        let via_registry = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&actor)
            .expect("ActorRef registered");
        assert_eq!(json!(door.bits()), via_registry);

        let reference = actor.component("Light", 2);
        let via_registry = RUNTIME_TYPE_REGISTRY
            .serialize_json_for_any(&reference)
            .expect("ComponentRef registered");
        assert_eq!(
            component_ref_json(world, door, "Light", 2, "parity"),
            via_registry
        );
    }

    #[test]
    fn resolvers_find_objects_and_report_lost_targets_typed() {
        let (store, door, _lamp) = scene();
        let world = store.world();

        assert_eq!(
            find_object_by_name(world, &json!("Front Door"), "find/name"),
            json!(door.bits())
        );
        assert_eq!(
            find_object_by_stable_id(world, &json!("lamp"), "find/id"),
            find_object_by_stable_id(world, &json!("lamp"), "find/id")
        );
        assert!(!find_object_by_stable_id(world, &json!("lamp"), "x").is_null());

        // Misses and non-string needles degrade to null (typed log inside).
        assert!(find_object_by_stable_id(world, &json!("ghost"), "find/id").is_null());
        assert!(find_object_by_name(world, &json!(7), "find/name").is_null());
    }

    #[test]
    fn literals_resolve_at_runtime_not_compile_time() {
        let (store, _door, lamp) = scene();
        let world = store.world();

        let resolved = object_literal_json(world, "lamp", "Light", 1, "literal");
        assert_eq!(
            resolved,
            json!({ "entity": lamp.bits(), "class_name": "Light", "component_index": 1 })
        );
        // Lost target: typed failure, null result — never a silent rebinding.
        assert!(object_literal_json(world, "demolished", "Light", 0, "literal").is_null());
    }

    #[test]
    fn pin_targets_validate_liveness_and_class_match() {
        let (store, door, lamp) = scene();
        let world = store.world();

        // "VmProbe" is registered by the component_ops test inventory;
        // unregistered classes must refuse validation (B's contract).
        let good = component_ref_json(world, lamp, "VmProbe", 0, "mk");
        assert_eq!(resolve_pin_target(world, &good, "VmProbe", "set"), Some((lamp, 0)));

        // #519 discipline: a ref of one class cannot feed another class's op.
        assert!(resolve_pin_target(world, &good, "Door", "set").is_none());

        // Stale reference (despawned actor) degrades without panicking.
        let stale = json!({
            "entity": 9_999_999u64 << 32,
            "class_name": "VmProbe",
            "component_index": 0,
        });
        assert!(resolve_pin_target(world, &stale, "VmProbe", "set").is_none());

        // Malformed operands are marshalling failures, never panics.
        assert!(resolve_pin_target(world, &json!(42), "VmProbe", "set").is_none());
        assert!(resolve_pin_target(world, &json!({"entity": 1}), "VmProbe", "set").is_none());
        assert!(resolve_pin_target(
            world,
            &json!({"entity": u64::MAX, "class_name": "VmProbe", "component_index": 0}),
            "VmProbe",
            "set",
        )
        .is_none());

        // A live ref round-trips its index so instance 2 stays addressable.
        let second = component_ref_json(world, door, "VmProbe", 2, "mk");
        assert_eq!(resolve_pin_target(world, &second, "VmProbe", "call"), Some((door, 2)));
    }
}
