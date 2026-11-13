# Add a Component

How a world component class is created and how it works: a typed value in SceneDB's `World`, registered by its plugin's root, with an optional lifecycle (`construct`, `begin_play`, `tick`, `end_play`) and script-callable methods. References: `.agents/COMPONENT_LIFECYCLE_PLAN.md` (the "As built" paragraphs), `.agents/ECS.md`, `.agents/PLUGIN_SYSTEM.md` ("Component classes"). Worked examples: `plugins/vendor/fog` (data only), `plugins/vendor/physics_plugin` (lifecycle), `plugins/installed/tests/fixtures/lifecycle_plugin` (smallest lifecycle). The plugin itself: `/add-plugin`.

---

## 1. Define the class (the plugin's `component` crate)

```rust
use engine_class_derive::{engine_class, world_component};

#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
pub struct MyComponent {
    #[property]
    pub enabled: bool,
    // ...
}

// A world class, registered by the plugin's root (not here).
#[world_component]
impl MyComponent {}
```

`#[engine_class]` gives reflection (properties, the inspector); `#[world_component]` makes it a `World` class named after the type, **without registering it** (`crates/core/engine_class_derive/src/lib.rs`). `#[register_world_component]` registers at the type and is for the engine's own components only.

## 2. Register it from the plugin root

In the root's `build` (`plugins/vendor/fog/src/runtime.rs`):

```rust
p.components().class::<MyComponent>(ClassSpec {
    requires: &["OtherComponent"],                       // placed together
    systems: &[STEP],                                    // per-frame work
    actor: Some(ActorTable::of::<MyComponent>()),        // lifecycle
    gpu: None,                                           // source rows + join
    presentation: Presentation::default(),
});
```

`ClassSpec` (`crates/core/engine_backend/src/scene/class_spec.rs`) is `Default`; use `..ClassSpec::default()`. Each field:

- **requires**: classes attached with it as their own component-instance entities when it is placed (editor Add Component, a script's attach). The required class must come from a plugin whose `class` contract this plugin requires; loading never places.
- **systems**: `System::new("pulsar.mykind.step", run).after(&[COMPONENTS])`, ordered by labels around the engine's lifecycle step (`pulsar_world_registry::systems::COMPONENTS`); run while the class is registered. Physics' `STEP` is the model (`registrations.rs`).
- **actor**: the lifecycle table (next section). Registered as an `ActorRegistration` (category `component.lifecycle`, origin the plugin root), gated with the class: a disabled plugin's table is not handed out or driven.
- **gpu**: `ClassRows { name, register_columns, derivation }`, the class's source rows and their join (needs the `render` feature of `engine_backend`).
- **presentation**: its own inspector card, or a facade (one card and one script component over itself and what it requires; `plugins/vendor/character`).

A disabled plugin's instances load as `UnresolvedComponent`s with their data kept.

## 3. Lifecycle: `Actor` and `WorldActor`

`crates/core/pulsar_component_lifecycle/src/lib.rs`. Pick one form per class (the table can mix per event: `ActorTable` fields are public).

| Event | When | Delivered in |
|---|---|---|
| `construct` | component added or changed, coalesced to once per frame per instance | editor and play |
| `begin_play` | an enabled instance is first seen by a running session | play |
| `tick` | each frame, every live enabled instance | play |
| `end_play` | disabled, removed, owner despawned, or session end | play |

**`Actor`** (`&mut self`, no world; the framework holds the component's row, so a callback cannot re-enter a world borrow). Register with `ActorTable::of::<T>()`:

```rust
impl Actor for MyComponent {
    fn enabled(&self) -> bool { self.enabled }                       // disabled = ended
    fn construct(&mut self, cx: &mut ActorCx<'_>) {}                 // idempotent, cheap
    fn begin_play(&mut self, cx: &mut ActorCx<'_>) {}
    fn tick(&mut self, cx: &mut ActorCx<'_>, dt: f32) {}
    fn end_play(cx: &mut ActorCx<'_>) {}                             // no self: it is gone
}
```

**`WorldActor`** (`&mut World` and the instance entity, no component borrow; for classes that must see siblings, like physics). Register with `ActorTable::of_world::<T>()`:

```rust
impl WorldActor for RigidBodyComponent {
    fn begin_play(world: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {}
    fn end_play(world: &mut World, cx: &mut ActorCx<'_>) {}   // cx.instance = Some(..); the component may be gone
}
```

Facts that bite:

- `Actor::end_play` sees only the owner and the session. `WorldActor::end_play` gets the world; the component is present at a session end or disable and **gone** after a removal or despawn, so `world.get::<T>(instance)` can be `None`. State a class needs at the end belongs in session resources keyed by owner.
- A write `construct` makes to its own component does not re-run it; a restore, a big undo or a level load is one rescan (each live instance constructs once). `cx.construct` says why (`ConstructReason::{Changed, Added, Rescan}`) and which properties changed when the writer named them.
- `begin_play` is on the first frame the session ticks the instance, so it sees the whole level.
- Tick order within a frame is deterministic: by type name, then entity order.
- `ActorCx` fields: `entity` (the owner), `instance`, `events` (the session's `EventHub`), `session`; helpers `cx.session()` and `cx.kind()`.
- Class-level native handlers for events: `ActorTable::handler_events` plus `event` (generated by `#[register_component_runtime]` with `#[bp_handler]` methods, which expands to an `ActorTable`; one lifecycle per class, by convention).

### Session scope

`cx.session()` is the `SessionScope` (`session.rs`):

- **kind**: `SessionKind::{Editor, PlayInEditor, Standalone, Headless}`. Only `construct` runs in `Editor`; `kind.is_playing()` says whether a session plays.
- **settings**: the level's `world_settings` as JSON, read-only (`cx.session().setting("gravity")`); a plugin declares its keys as a `WorldSettingField` in its editor registrations. Read at `begin_play`: a mid-play change is not seen (physics' deviation).
- **resources**: a bag by type (`get_or_insert_with`, `resource`, `resource_mut`, `insert_resource`, `remove_resource`). Per-session state lives here, never in a global: physics' `PhysicsSession` (the simulation thread) is created by the first `begin`, removed by the last `end`, and its `Drop` joins the thread. The session end drops every resource after the `end_play`s.

Physics is the reference: `plugins/vendor/physics_plugin/src/{lifecycle.rs, registrations.rs, runtime.rs}`.

## 4. Never a reflected method

Lifecycle is a registry of its own, **never** `#[component_methods]`/reflected methods, so it is not a Blueprint node by construction:

- `pulsar_component_lifecycle` depends on SceneDB, the event hub and the plugin gate only, never `pulsar_reflection` or the script VM (its `crate_edges` test pins this).
- A reflected method or script native named `construct`, `begin_play`, `tick` or `end_play` (`RESERVED_METHODS`) is a leak: `pulsar_world_registry::audit::find_lifecycle_problems` fails it, because the reflection registry has no hidden flag. `plugins/installed/tests/lifecycle.rs` runs it over every installed class. Name script-facing methods differently (`step`, `start`, ...).

## 5. Methods the scripts call

```rust
use pulsar_scenedb::component_methods;

#[component_methods]                       // one impl block per class; overloads are disallowed
impl MyComponent {
    #[reflect_method(side_effect_free, category = "My")]
    fn value(&self) -> f32 { self.value }

    #[world_method(category = "My")]
    fn set_value(world: &mut World, entity: Entity, value: f32) -> Result<(), String> { .. }
}
```

(`crates/core/pulsar_scene_model/src/transform_script.rs`, `crates/core/pulsar_script_vm/tests/world.rs`.) `#[reflect_method]` takes `&self`; `#[world_method]` takes the world and the entity. They reach the script VM and Blueprint palettes through the reflection registry and `World::call_component_method`, so keep names and categories stable: a golden snapshot test diffs the reflected surface (`plugins/vendor/physics_plugin/src/golden_metadata.rs`, `PULSAR_UPDATE_SNAPSHOT=1` to regenerate on purpose).

## 6. Events the component raises

A class declares its events and offers them to scripts with a class-scoped, link-time `ComponentEventRegistration { class_name, descriptor, declaration }` (`pulsar_world_registry`): its Gamma descriptor and its script-side declaration. Physics declares each in the component's file (`declare_events!`) and submits them in `registrations.rs`; `#[component_events(class = "..")]` generates the typed ones for a trait. Events publish on the owner's entity channel, so a script on the object hears its own components'. Name them under the plugin's namespace (`physics::ColliderComponent.collision_enter`).

## 7. Profiler scopes

Work a lifecycle callback or system starts is recorded with `profiling::profile_scope!("<Plugin>::<Phase>")` at its top (physics: `"Physics::Tick"`, `"Physics::Frame"`, `"Physics::Publish"`); a service thread names itself with `profiling::set_thread_name("Pulsar-<Plugin>")` and is registered in the engine's thread registry (`.agents/EDITOR_TASKS.md`).

## 8. Tests

- Lifecycle order and the gate: copy `plugins/installed/tests/fixtures/lifecycle_plugin` and `plugins/installed/tests/lifecycle.rs` (a fixture plugin with a probe class; order on disable, removal, despawn; plugin gate; session kinds; construct coalescing).
- Drive a class in a unit test with `pulsar_world_registry::{tick_live_components, ComponentRuntimeState, end_session}`; physics' `runtime.rs` tests are the model (two sessions do not share state, the last component ends the thread).
- Table lookups: `ActorIndex` tests in `pulsar_component_lifecycle`.
- The leak audit runs for every installed class once your plugin is installed (`/add-plugin`); `just arch` for layering.

```bash
cargo test -p installed_plugins --test lifecycle
cargo test -p pulsar_component_lifecycle -p pulsar_world_registry
```
