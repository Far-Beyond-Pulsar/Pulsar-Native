//! Bridges reflection-typed, [`pulsar_reflection::ComponentRuntimeBehavior`]-
//! implementing components into `pulsar_scenedb::World` (Pulsar-Native#555/
//! #556, Phase B4/B5 of the SceneDB + Helio + reflection/properties-panel
//! unification -- see [Pulsar-Native#561](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/561)).
//!
//! ## What this solves
//!
//! Before this crate, a component's *live* runtime shape was always
//! `serde_json::Value`: `HelioRenderer::sync_scene` re-deserialized every
//! component's JSON into its typed struct on *every rendered frame* (via
//! `apply_runtime_behavior_for_class`), even though `#[register_runtime_behavior]`
//! (Phase B2) already made `ComponentRuntimeBehavior::sync_component` itself
//! typed -- the JSON round trip was purely an artifact of the dispatch
//! boundary, not the trait.
//!
//! `#[register_world_component]` (`engine_class_derive`) lets a component opt
//! into this crate's registry, which provides:
//! - **`hydrate`**: deserialize a component's JSON *once*, when it's
//!   actually edited (`SceneDatabase`'s component-mutation hook), and insert
//!   the typed value into `World` at that entity.
//! - **`dispatch`**: read the typed value already sitting in `World` and call
//!   `ComponentRuntimeBehavior::sync_component` directly -- no JSON, no
//!   `serde_json::from_value` -- on the render hot path.
//! - **`remove`**: drop the typed value when the component is deleted,
//!   disabled, or its owning object is despawned.
//! - **`on_removed`**: the consumer-side teardown counterpart to `hydrate` --
//!   dispatched (with full `ComponentRuntimeContext`) off `World`'s own
//!   attached `ChangeTracker`, which already records every component
//!   removal automatically (`ChangeTracker::drain_component_removals`, no
//!   manual bookkeeping in this crate or its callers), so a component that
//!   created external state in `sync_component` (a Helio light, a cached
//!   GPU actor) can drop it too -- without SceneDB or `World` ever needing
//!   to know that state, or even the concept of a "class", exists. See
//!   [`notify_world_component_removed_by_component_id`]'s doc.
//! - **[`GpuMirrored`]/[`GpuListMirrored`]**: SceneDB-mirrored companion
//!   components, auto-derived by `engine_class_derive` for any `#[property]`
//!   field marked `#[gpu]` (`GpuMirrored`, packed/fixed-size) or
//!   `#[gpu] Vec<T>` (`GpuListMirrored`, var-len -- a separate companion,
//!   deliberately, see that trait's doc for why). `#[register_world_component
//!   (gpu_mirror)]` wires both into the generated `hydrate`/`remove` above.
//!   See [`GpuMirrored`]'s doc for the full design and packing rules.
//!
//! ## Why this crate exists instead of extending `pulsar_reflection` directly
//!
//! `pulsar_reflection` is a separate repository, also used by non-SceneDB
//! contexts (the Blueprint and shader editors reuse its property-reflection
//! machinery), so it deliberately has no dependency on `pulsar_scenedb` --
//! and never will, regardless of how much friction pinning `pulsar_scenedb`
//! itself does or doesn't have day to day (that pin has moved several times
//! without incident since this crate was first written; the constraint here
//! was always about the dependency direction, not about pin friction).
//! Every actual consumer of this crate (`helio_component`, `pulsar_physics`,
//! `engine_backend`, `ui_level_editor`) already depends on both
//! `pulsar_scenedb` and `pulsar_reflection` directly, so a small
//! Pulsar-Native-internal crate sitting alongside them -- itself depending
//! on both -- is a clean fit with no cross-repo coordination needed at all.
//!
//! ## Why a separate registry from `RuntimeBehaviorRegistration`
//!
//! `pulsar_reflection::RuntimeBehaviorRegistration`/`apply_runtime_behavior_for_class`
//! stay exactly as they are (JSON-based) -- they're still the dispatch path
//! for anything that only has JSON on hand (`pulsar_scene::SceneLoader`, and
//! any component that hasn't been migrated onto this registry yet, e.g. the
//! rest of B5's list before it lands). Migration is opt-in and incremental,
//! one component at a time, which is exactly why this is a sibling registry
//! rather than a breaking change to the existing one.

// Re-exported so `#[register_world_component]` (`engine_class_derive`) can
// emit `pulsar_world_registry::inventory::submit! { .. }` in the calling
// crate without that crate needing its own direct `inventory` dependency --
// same pattern `pulsar_reflection` already uses for `RuntimeBehaviorRegistration`.
pub use inventory;

use pulsar_reflection::{ComponentRuntimeContext, EngineClass, RuntimeComponentOwner};
use pulsar_scenedb::{ComponentId, Entity, World};
use serde_json::Value;

/// One registered component class's `World` bridge. Populated by
/// `#[register_world_component]` (`engine_class_derive`) -- not constructed
/// by hand.
pub struct WorldComponentRegistration {
    pub class_name: &'static str,
    /// This class's `pulsar_scenedb::ComponentId`, as a function pointer
    /// rather than a precomputed value -- `component_id::<T>()` isn't
    /// `const fn` (it allocates on a type's first-ever call, via a global
    /// registry), and `inventory::submit!`'s payload has to be const-
    /// evaluable, so `pulsar_scenedb::component_id::<#self_ty>` (the
    /// function item itself, not a call) is what
    /// `#[register_world_component]` actually emits here. Called at lookup
    /// time instead ([`find_by_component_id`]) -- cheap, `component_id::<T>`
    /// caches its own result after the first call regardless of caller.
    ///
    /// This is the identity `World::remove`/`World::despawn` actually record
    /// into the attached `ChangeTracker`'s `component_removals` list
    /// (`SceneDB`, `Entity` + `ComponentId`, no notion of "class name" at
    /// that layer at all). Lets
    /// [`notify_world_component_removed_by_component_id`] translate a
    /// drained removal straight back to this registration with no name
    /// lookup in between -- `World`/SceneDB never need to know a class name
    /// exists, and this crate never needs a second, parallel removal-
    /// tracking mechanism of its own (see that fn's doc).
    pub component_type: fn() -> ComponentId,
    /// Deserialize `data` and insert/overwrite this class's typed component
    /// on `entity`. Called once per edit (from `SceneDatabase`'s component
    /// mutation hook), never from the render loop.
    pub hydrate: fn(&mut World, Entity, &Value) -> Result<(), String>,
    /// Remove this class's typed component from `entity`, if present.
    /// Called when the component is deleted, disabled, or its owning object
    /// is despawned.
    pub remove: fn(&mut World, Entity),
    /// Dispatch `ComponentRuntimeBehavior::sync_component` using the typed
    /// value already in `World` -- no JSON deserialize on this path at all.
    /// Returns `false` (and does nothing) if `entity` doesn't have this
    /// component in `World` (not hydrated yet); callers should fall back to
    /// `pulsar_reflection::apply_runtime_behavior_for_class` in that case,
    /// which still works off the JSON channel unconditionally.
    pub dispatch: fn(
        &World,
        Entity,
        &RuntimeComponentOwner,
        usize,
        &mut dyn ComponentRuntimeContext,
    ) -> bool,
    /// Borrow the typed value already in `World` as `&dyn EngineClass` --
    /// the properties panel's *read* path. No JSON, no throwaway `Default`
    /// instance: this is the one real, live value.
    pub get_as_engine_class: fn(&World, Entity) -> Option<&dyn EngineClass>,
    /// Borrow the typed value already in `World` as `&mut dyn EngineClass`
    /// -- the properties panel's *write* path. Apply a `PropertyMetadata`
    /// setter closure straight to this reference to mutate the one real
    /// value in place; there is no second copy to keep in sync afterward.
    pub get_as_engine_class_mut: fn(&mut World, Entity) -> Option<&mut dyn EngineClass>,
    /// Called when this class's component is going away -- removed from a
    /// still-alive object, disabled, or the object itself despawned -- so
    /// whatever external (non-`World`) state the component's own
    /// `sync_component` created (a Helio light, a GPU actor, a cache entry)
    /// gets torn down too.
    ///
    /// Deliberately *not* a `World`-mutating call: by the time this runs the
    /// typed value may already be gone from `World` (see
    /// `WorldSceneStore::take_pending_component_removals`'s doc for why this
    /// has to be queued at removal time and drained later, with full
    /// `ComponentRuntimeContext`, rather than called inline from wherever
    /// the removal itself happens). This is the missing symmetric half of
    /// `hydrate`: `hydrate` is "this class's data now exists, adopt it";
    /// `on_removed` is "this class's data is gone, drop whatever you built
    /// from it" -- the same component author owns both, and SceneDB/`World`
    /// stays out of the conversation entirely (it doesn't know a `LightId`
    /// or a `Scene` exists). Defaults to a no-op (`register_world_component`
    /// generates one when no `on_removed = ...` override is given) --
    /// correct for any class whose `sync_component` never created
    /// consumer-side state that would otherwise leak.
    pub on_removed: fn(&RuntimeComponentOwner, &mut dyn ComponentRuntimeContext),
}

inventory::collect!(WorldComponentRegistration);

fn find(class_name: &str) -> Option<&'static WorldComponentRegistration> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .find(|r| r.class_name == class_name)
}

/// Same lookup as [`find`], keyed by `ComponentId` instead of class name --
/// what a drained `ChangeTracker::component_removals` entry actually
/// carries (see `WorldComponentRegistration::component_type`'s doc). A
/// linear scan over `inventory::iter`, same as `find` -- the registered-
/// class count is small (dozens, not thousands) and this only runs once
/// per removal event, not per frame per entity, so it isn't worth a
/// memoized `HashMap` until that stops being true.
fn find_by_component_id(component_type: ComponentId) -> Option<&'static WorldComponentRegistration> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .find(|r| (r.component_type)() == component_type)
}

/// Hydrate `class_name`'s typed component from `data` onto `entity`. Returns
/// `Ok(false)` if `class_name` isn't registered here (not migrated yet, or
/// not a real component class) -- not an error, it just means the JSON
/// channel stays authoritative for this class. Returns `Err` only if
/// `class_name` *is* registered but `data` failed to deserialize.
pub fn hydrate_world_component_for_class(
    class_name: &str,
    world: &mut World,
    entity: Entity,
    data: &Value,
) -> Result<bool, String> {
    match find(class_name) {
        Some(registration) => (registration.hydrate)(world, entity, data).map(|()| true),
        None => Ok(false),
    }
}

/// Remove `class_name`'s typed component from `entity`, if that class is
/// registered here. Returns `false` if `class_name` isn't registered.
pub fn remove_world_component_for_class(class_name: &str, world: &mut World, entity: Entity) -> bool {
    match find(class_name) {
        Some(registration) => {
            (registration.remove)(world, entity);
            true
        }
        None => false,
    }
}

/// Dispatch `class_name`'s `on_removed` hook -- the consumer-side teardown
/// counterpart to `hydrate`. Returns `false` if `class_name` isn't
/// registered here (nothing to notify; the JSON-only/legacy dispatch path
/// has no consumer-side state to tear down in the first place).
///
/// Callers don't need to check whether `entity` still has this component in
/// `World` first -- `on_removed` only ever needs `owner`'s tag/position, not
/// a live `World` lookup, so it's safe to call after the typed value (or
/// `entity` itself) is already gone. In practice callers reach this class
/// name via [`notify_world_component_removed_by_component_id`] below, which
/// is what a real removal event (`World`'s attached `ChangeTracker`) hands
/// you -- this `class_name`-keyed spelling exists for callers that already
/// have the name some other way (tests, anything working off the JSON/class
/// registry side).
pub fn notify_world_component_removed(
    class_name: &str,
    owner: &RuntimeComponentOwner,
    context: &mut dyn ComponentRuntimeContext,
) -> bool {
    match find(class_name) {
        Some(registration) => {
            (registration.on_removed)(owner, context);
            true
        }
        None => false,
    }
}

/// Dispatch `on_removed` for whichever registered class owns
/// `component_type` -- the direct consumer of a
/// `pulsar_scenedb::ChangeTracker::drain_component_removals()` entry.
///
/// This is the actual removal-detection mechanism (Pulsar-Native#561's
/// "zero dupe state" cleanup): `World::remove`/`World::despawn` already
/// record every component removal into the `SharedChangeTracker` attached
/// at `WorldSceneStore` construction, automatically, for every mutation, no
/// `_tracked` call or manual bookkeeping needed anywhere (same "attach
/// once, every write already knows" shape `#[gpu]` mirroring already uses).
/// A caller (`HelioRenderer`'s sync pass) drains that list once per sync
/// pass and calls this per entry -- SceneDB/`World` never need to know a
/// "class" or "component trait" concept exists at all; this crate is the
/// only place that translates a bare `ComponentId` back into "which
/// registered class is this, and what does removal mean to it".
///
/// Returns `false` if `component_type` isn't a registered class (nothing to
/// notify -- e.g. a plain bookkeeping component like `Parent`/`Transform`
/// with no `#[register_world_component]` at all).
pub fn notify_world_component_removed_by_component_id(
    component_type: ComponentId,
    owner: &RuntimeComponentOwner,
    context: &mut dyn ComponentRuntimeContext,
) -> bool {
    match find_by_component_id(component_type) {
        Some(registration) => {
            (registration.on_removed)(owner, context);
            true
        }
        None => false,
    }
}

/// Dispatch `class_name`'s `ComponentRuntimeBehavior::sync_component`
/// directly off `entity`'s typed `World` value, if that class is registered
/// here and hydrated on `entity`. Returns `false` otherwise -- callers
/// should fall back to `pulsar_reflection::apply_runtime_behavior_for_class`.
pub fn dispatch_world_component_for_class(
    class_name: &str,
    world: &World,
    entity: Entity,
    owner: &RuntimeComponentOwner,
    component_index: usize,
    context: &mut dyn ComponentRuntimeContext,
) -> bool {
    match find(class_name) {
        Some(registration) => (registration.dispatch)(world, entity, owner, component_index, context),
        None => false,
    }
}

/// Borrow `class_name`'s typed value already in `World` as `&dyn EngineClass`
/// for reading -- the properties panel's read path (Pulsar-Native#561).
/// `None` if `class_name` isn't registered here, or `entity` doesn't have
/// this component in `World` yet; callers should fall back to whatever
/// non-live source they have (JSON, a `Default` instance) in that case.
pub fn get_world_component_as_engine_class<'w>(
    class_name: &str,
    world: &'w World,
    entity: Entity,
) -> Option<&'w dyn EngineClass> {
    (find(class_name)?.get_as_engine_class)(world, entity)
}

/// Borrow `class_name`'s typed value already in `World` as
/// `&mut dyn EngineClass` for direct in-place editing -- the properties
/// panel's write path. Apply a property's setter closure straight to this
/// reference: it mutates the one real `World`-resident component, so there
/// is nothing to write back afterward. `None` under the same conditions as
/// [`get_world_component_as_engine_class`].
pub fn get_world_component_as_engine_class_mut<'w>(
    class_name: &str,
    world: &'w mut World,
    entity: Entity,
) -> Option<&'w mut dyn EngineClass> {
    (find(class_name)?.get_as_engine_class_mut)(world, entity)
}

/// Every currently-registered `World`-backed class name. `SceneDatabase`
/// uses this to know which classes to check for removal when an object's
/// component list changes -- a class present in `World` from a previous
/// hydration but no longer in the object's current enabled component list
/// needs `remove` called, and this is how it finds out which classes to
/// even ask about.
pub fn registered_world_component_classes() -> impl Iterator<Item = &'static str> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .map(|r| r.class_name)
}

// ── Auto-derived GPU mirroring (Pulsar-Native#561) ──────────────────────────
//
// `LightComponent`'s original hand-written `LightGpuData`/`LightGpuRow`
// companion (`helio_component`) proved the pattern by hand: a second,
// `#[gpu]`-mirrored SceneDB component holding only a type's render-relevant,
// `Pod`-safe translation, kept in step by hydrate, so a renderer never has
// to hand-roll a Helio-side cache/sync-by-diff for that data again. This
// section is the same pattern generated automatically by `engine_class_
// derive` for any `#[property]` field marked `#[gpu]` -- `LightComponent`
// itself has since been normalized onto it (its `LightComponentGpuMirror`),
// with no hand-written companion left. See `GpuRepr<T>`'s doc below for the
// (universal -- any `Copy` type mirrors as its own exact bytes, no
// allowlist/denylist/semantic conversion) packing rule, and `GpuHeavy<T>`'s
// doc for the separate handle/heavy-element split, and `engine_class_
// derive`'s own doc for how `#[sub_props]` nesting composes.
//
// ## The GPU upload modes this covers, and where field-level transforms fit
//
// A `#[gpu]` field's bytes reach a real `wgpu::Buffer` through one of a few
// SceneDB-level upload modes, chosen by the field's own shape (never
// something a component author picks by hand):
//
// - **`DirtyTracked`** (the default for a scalar `#[gpu]` field): keeps a
//   full CPU shadow, re-uploads only the rows that actually changed since
//   the last flush. `to_gpu_mirror()` -- and therefore any `engine_class_
//   derive::GpuFieldOverride` (`#[gpu(as = .., with = ..)]`) on that field
//   -- runs once per genuine property edit, never per frame.
// - **`Once`**: uploaded a single time, at first insert, never re-run
//   after. Cheaper still, same "not per frame" property.
// - **Var-len** (`Vec<T>` fields, `GpuListMirrored`): a shared, growable
//   `VarLenGpuPool<GpuRepr<T>>` suballocated per entity, freed/reallocated
//   only when the `Vec`'s length actually changes.
// - **Heavy** (`GpuHeavy<T>`, `GpuUploadSource`): a tiny CPU handle stands
//   in for an arbitrarily large GPU-resident element, uploaded via
//   `upload_element` -- the handle/heavy-element split exists for byte
//   SIZE, a different concern from `as`/`with`'s byte SHAPE.
//
// `#[gpu(as = Type, with = path)]` (`engine_class_derive::GpuFieldOverride`)
// only touches the FIRST of these today (plain scalar `DirtyTracked`/`Once`
// fields) -- it changes what bytes a field's `to_gpu_mirror()` call
// produces, not which of these upload paths carries them. Its performance
// story rides entirely on the mode it's used in: since none of the above
// modes re-run `to_gpu_mirror` per rendered frame, neither does `with` --
// its cost is bounded by edit frequency, not frame rate, regardless of
// which mode the field is on. See `GpuFieldOverride`'s own doc
// (`engine_class_derive`) for the full design and the one real cost it DOES
// have (a wider `as` target type costs more GPU storage/bandwidth per row).

/// A type whose `#[gpu]`-marked `#[property]` fields (`engine_class_derive`)
/// have an automatically-derived, `Pod`, SceneDB-mirrorable translation.
///
/// `engine_class_derive` generates an impl of this for EVERY
/// `#[engine_class(...)]`-processed struct, unconditionally -- including
/// ones with zero `#[gpu]` fields, which get `GpuMirror = NoGpuMirror` (see
/// that type's doc for why this uniform generation, rather than only
/// generating an impl when there's something real to mirror, is what makes
/// `#[sub_props]` composition possible with no special-casing at each
/// nesting level: a containing struct's own generated `to_gpu_mirror` reads
/// `<SubPropsTy as GpuMirrored>::GpuMirror`/`to_gpu_mirror()` for every
/// `#[sub_props]` field unconditionally, and that has to type-check whether
/// or not that particular sub-props group happens to contain any `#[gpu]`
/// leaves this time).
pub trait GpuMirrored {
    /// The `Pod` GPU-mirror type. `NoGpuMirror` when there's nothing to
    /// mirror (no `#[gpu]` fields anywhere in this struct or its
    /// `#[sub_props]`).
    type GpuMirror: pulsar_scenedb::Pod + Send + Sync + 'static;

    /// Translate `self`'s current `#[gpu]`-marked fields (and its
    /// `#[sub_props]` fields' own translations) into `Self::GpuMirror`.
    fn to_gpu_mirror(&self) -> Self::GpuMirror;

    /// Insert `self`'s current GPU mirror onto `entity` -- a `World::insert`
    /// away from being SceneDB-mirrored automatically, same as any other
    /// `#[gpu]` write. A no-op default would be wrong here (every type gets
    /// SOME impl of this trait, including ones with real fields to mirror),
    /// so this is a real, non-overridable default method: `#[engine_class]`
    /// never needs to generate a per-type version of this, only
    /// `to_gpu_mirror` and the associated type above.
    fn sync_gpu_mirror(&self, world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        world.insert(entity, self.to_gpu_mirror());
    }

    /// Drop `entity`'s mirrored `Self::GpuMirror`, if it has one. Harmless
    /// (a plain `World::remove` miss) for a type whose `GpuMirror` is
    /// `NoGpuMirror` and was never actually inserted anywhere -- see
    /// `sync_gpu_mirror`'s doc for why every type still has an impl to call
    /// this through.
    fn remove_gpu_mirror(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        let _ = world.remove::<Self::GpuMirror>(entity);
    }
}

/// Wraps ANY `Copy` type for GPU mirroring, treating its exact Rust memory
/// representation as the GPU-side bytes -- no classification, no allowlist
/// of "recognized" types, no semantic conversion (a `bool` stays its own
/// 1-byte representation; an enum stays whatever bytes its own `#[repr]`
/// gives it). This is the ONLY thing standing between an arbitrary
/// `#[gpu]`-marked field and SceneDB's `Pod` bound -- see [`Self`]'s safety
/// note below for exactly what it does and doesn't guarantee.
///
/// # Why this exists (Pulsar-Native#561: the auto-mirror generator's field
/// support must be universal, not a hand-maintained list of "supported"
/// shapes)
///
/// `pulsar_scenedb::Pod`'s own safety contract is stricter than this needs:
/// it requires all-zero bytes (and by extension, in how it's actually used
/// elsewhere in SceneDB, ANY byte pattern) to be a valid value of `T`,
/// because SOME of its use sites hand out `&[T]` over raw, possibly-
/// zeroed-or-arbitrary memory and let safe code read it back directly. A
/// `#[gpu]`-mirrored component field is not that use site: `World::insert`
/// only ever writes bytes copied out of an ALREADY-VALID, live `T` the
/// caller is holding -- there is no "hand out uninitialized memory as
/// `&Self`" step anywhere in that path. What crosses to the GPU is real
/// data, once; what happens to those bytes after that (a shader reads
/// them, a compute pass writes new ones back) is the consumer's problem to
/// interpret correctly, not something the Rust type system can, or needs
/// to, verify on the way there.
///
/// # Safety (for the blanket `unsafe impl Pod for GpuRepr<T>` below)
///
/// This is a deliberate, narrower contract than `Pod`'s doc literally
/// states -- `GpuRepr<T>` is sound to WRITE (copy an existing `T`'s bytes
/// out) for any `Copy` `T`, unconditionally. It is only sound to READ BACK
/// arbitrary bytes as a live `T` (e.g. `World::get`, `readback_row` after a
/// compute shader wrote something) if whatever produced those bytes
/// produced a valid `T` in the first place -- exactly as true of any GPU
/// buffer read in any graphics API, and exactly the "bytes are just bytes,
/// the consumer interprets them" contract this type exists to make
/// explicit rather than pretend doesn't apply.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuRepr<T: Copy>(pub T);

// SAFETY: see this type's own doc above -- a deliberate, narrower contract
// than a literal reading of `Pod`'s doc comment, scoped to the write-only
// GPU-mirror use case this type exists for.
unsafe impl<T: Copy + Send + Sync + 'static> pulsar_scenedb::Pod for GpuRepr<T> {}

impl<T: Copy> std::ops::Deref for GpuRepr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Copy> std::ops::DerefMut for GpuRepr<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T: Copy> From<T> for GpuRepr<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

/// The `GpuMirror` for a type with no `#[gpu]`-marked fields anywhere (the
/// common case -- most components have none). Zero-sized, so a struct that
/// composes one in via `#[sub_props]` (`engine_class_derive`'s generated
/// mirror struct embeds `<SubPropsTy as GpuMirrored>::GpuMirror` per
/// sub-props field, unconditionally) pays nothing for a sub-props group
/// that happens to contribute no real data this time.
///
/// Deliberately a dedicated type, not `()`: `()` is a general-purpose Rust
/// primitive with its own broader meaning, and giving "nothing to mirror"
/// its own named type keeps a `GpuMirror = NoGpuMirror` reader unambiguous
/// about which of the trait's two cases they're looking at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoGpuMirror;

unsafe impl pulsar_scenedb::Pod for NoGpuMirror {}

/// The var-len-mirrored counterpart to [`GpuMirrored`], for `#[gpu] Vec<T>`
/// `#[property]` fields.
///
/// Deliberately a SEPARATE trait/companion component, not a second
/// `Vec`-typed field folded into [`GpuMirrored::GpuMirror`]: SceneDB's own
/// `#[derive(SceneStore)]` forks a struct onto ONE of two completely
/// different codegen paths depending on whether it has any `Vec<T>`
/// `#[gpu]` field -- the packed, one-buffer-per-struct layout `GpuMirror`
/// relies on is explicitly unsupported on that OTHER path (a `Vec<T>`
/// field's length varies per row, so "one packed record" has no meaning).
/// Cramming a list field into the same mirror struct as the scalar leaves
/// would silently downgrade EVERY scalar field on that component from one
/// packed buffer + one write back to the classic one-buffer-per-field
/// split, for the whole struct, just because one field happened to need a
/// list. Two independently-mirrored companion components (both riding the
/// SAME entity -- a component with both a packed scalar `#[gpu]` field and
/// a `#[gpu] Vec<T>` field gets one of each, `{Name}GpuMirror` and
/// `{Name}GpuListMirror`, entirely separate types) keeps the packed half
/// fully packed regardless of whether the list half is even present.
///
/// `engine_class_derive` generates an impl of this for EVERY
/// `#[engine_class(...)]`-processed struct, unconditionally -- same
/// "always some impl, `NoGpuMirror` when there's nothing" shape
/// `GpuMirrored` uses, for the same reason (so a future composition of
/// list fields through `#[sub_props]` nesting can rely on every struct
/// having one to ask, the same way scalar composition already does).
/// `GpuListMirror` reuses [`NoGpuMirror`] for the trivial case too -- it's
/// equally valid as "nothing to mirror" regardless of which trait is
/// asking, and a plain (non-`#[gpu]`) `World` component needs no `Pod`-ness
/// at all, so there's no reason for a second sentinel type.
pub trait GpuListMirrored {
    /// The var-len-bearing `#[derive(SceneStore)]` companion type. Note:
    /// NOT `Pod` (a `Vec<T>`-holding struct never is) -- only
    /// `Send + Sync + 'static`, same as any ordinary `World` component.
    type GpuListMirror: Send + Sync + 'static;

    /// Translate `self`'s current `#[gpu] Vec<T>` fields into
    /// `Self::GpuListMirror`. Reallocates its `Vec`s every call (a fresh
    /// translation, not an in-place update) -- fine for how this is
    /// actually invoked (event-driven, on a genuine property edit via
    /// hydrate, not once per frame).
    fn to_gpu_list_mirror(&self) -> Self::GpuListMirror;

    /// Insert `self`'s current list mirror onto `entity`. Real, non-
    /// overridable default -- same reasoning as `GpuMirrored::
    /// sync_gpu_mirror`.
    fn sync_gpu_list_mirror(&self, world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        world.insert(entity, self.to_gpu_list_mirror());
    }

    /// Drop `entity`'s mirrored `Self::GpuListMirror`, if it has one.
    fn remove_gpu_list_mirror(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        let _ = world.remove::<Self::GpuListMirror>(entity);
    }
}

/// Marks a `#[property]` field as SceneDB's existing handle/heavy-element
/// split (`pulsar_scenedb::gpu::GpuUploadSource`) -- `T` stays a lightweight
/// CPU-side handle (an asset ID, typically 4-16 bytes); the actual
/// GPU-resident payload (`T::Element`, arbitrarily large -- mesh data,
/// texture data, whatever) lives in its own separately-registered buffer,
/// produced by `T::upload_element` only when the handle itself changes, not
/// re-uploaded on every frame.
///
/// # Why this exists (as a type, not a macro argument)
///
/// SceneDB's own `#[derive(SceneStore)]` has supported this split for a
/// while, via `#[gpu(mirror = Once, heavy)]` on the handle field -- but
/// `engine_class_derive`'s higher-level `#[property] #[gpu]` layer had no
/// way to reach it at all before this type existed, short of teaching it a
/// THIRD macro argument alongside `#[gpu]` itself. A proc macro can't ask
/// "does this field's type implement `GpuUploadSource`" (that's a trait-
/// resolution question, which happens after macro expansion, not during
/// it) -- so unlike plain scalar/`Vec<T>` `#[gpu]` fields (detected purely
/// from their own already-existing shape), a heavy field needs SOME
/// syntactic marker for the derive to pattern-match on, the same way
/// `Vec<T>` itself is a syntactic marker. Wrapping the handle in its own
/// type signature (`pub mesh: GpuHeavy<MeshHandle>` -- no attribute at
/// all) is that marker, detected by `engine_class_derive` the exact same
/// way `Vec<T>` already is (a field-type shape check, not a new attribute
/// argument to learn).
///
/// # Reflection
///
/// `GpuHeavy<T>` is NOT stripped away in the user's own struct (unlike
/// `GpuRepr<T>`, which only ever appears in an auto-generated companion,
/// invisible to the source struct) -- it stays the field's real, live type,
/// so it must itself be usable everywhere a `#[property]` field is: cloned
/// by the property getter/setter, and `Reflectable` for `type_info()`. Both
/// delegate straight through to `T` (below) -- the properties panel/editor
/// treats a `GpuHeavy<T>` field exactly as if it were a plain `T`, matching
/// `Deref`/`DerefMut`'s existing transparency. `engine_class_derive` unwraps
/// to the bare handle type `T` when generating the companion mirror struct
/// (SceneDB's `#[gpu(mirror = Once, heavy)]` applies to the handle itself,
/// not to any wrapper around it).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct GpuHeavy<T>(pub T);

impl<T> std::ops::Deref for GpuHeavy<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T> std::ops::DerefMut for GpuHeavy<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T> From<T> for GpuHeavy<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

// Delegates every operation straight to `T` -- see this type's own doc for
// why a `GpuHeavy<T>` `#[property]` field should read, in the editor, as
// nothing more than a plain `T` field. `type_info()` in particular returns
// `T`'s own registered runtime type info (not a distinct "GpuHeavy<T>"
// shape) -- `generate_property_metadata`'s generated code (`engine_class_
// derive`) only ever uses it descriptively (what kind of value/editor is
// this), never to decide how to downcast a value -- every actual get/set
// downcasts against the field's own literal declared type (`GpuHeavy<T>`),
// which stays internally consistent regardless of what `type_info()` says.
impl<T: pulsar_reflection::Reflectable + Clone> pulsar_reflection::Reflectable for GpuHeavy<T> {
    fn type_info() -> &'static pulsar_reflection::RuntimeTypeInfo
    where
        Self: Sized,
    {
        T::type_info()
    }

    fn serialize(&self, serializer: &mut dyn pulsar_reflection::TypeSerializer) -> pulsar_reflection::ReflectResult<()> {
        self.0.serialize(serializer)
    }

    fn deserialize(deserializer: &mut dyn pulsar_reflection::TypeDeserializer) -> pulsar_reflection::ReflectResult<Self>
    where
        Self: Sized,
    {
        Ok(Self(T::deserialize(deserializer)?))
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(self.clone())
    }
}

/// The heavy/handle-split counterpart to [`GpuMirrored`]/[`GpuListMirrored`],
/// for `#[gpu] GpuHeavy<T>` `#[property]` fields (`T: pulsar_scenedb::gpu::
/// GpuUploadSource`).
///
/// A third, independently-mirrored companion component -- same reasoning
/// [`GpuListMirrored`]'s doc gives for why var-len fields aren't folded into
/// [`GpuMirrored::GpuMirror`]: SceneDB's own derive rejects `#[gpu(heavy)]`
/// inside a packed struct outright (a packed buffer's element is the
/// struct's own interleaved record, not any one field's `GpuUploadSource::
/// Element`), so a heavy field can't share the packed scalar mirror's
/// struct any more than a `Vec<T>` field can. Unlike the list mirror,
/// though, `GpuHeavyMirror` IS `Pod` -- it holds the lightweight handle(s)
/// themselves (SceneDB's fixed, non-packed `#[gpu(mirror = Once, heavy)]`
/// path), never the heavy `Element` data, which lives in its own
/// separately-registered buffer entirely.
pub trait GpuHeavyMirrored {
    /// The `Pod` handle-holding companion type. `NoGpuMirror` when there's
    /// no `GpuHeavy<T>` field anywhere in this struct.
    type GpuHeavyMirror: pulsar_scenedb::Pod + Send + Sync + 'static;

    /// Translate `self`'s current `GpuHeavy<T>`-typed fields into
    /// `Self::GpuHeavyMirror` -- a plain handle copy, never a call into
    /// `GpuUploadSource::upload_element` (SceneDB's own `write_gpu_columns_
    /// at_row` does that, only when the handle's dirty-tracked slot is
    /// actually written).
    fn to_gpu_heavy_mirror(&self) -> Self::GpuHeavyMirror;

    /// Insert `self`'s current heavy mirror onto `entity`. Real,
    /// non-overridable default -- same reasoning as `GpuMirrored::
    /// sync_gpu_mirror`.
    fn sync_gpu_heavy_mirror(&self, world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        world.insert(entity, self.to_gpu_heavy_mirror());
    }

    /// Drop `entity`'s mirrored `Self::GpuHeavyMirror`, if it has one.
    fn remove_gpu_heavy_mirror(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
        let _ = world.remove::<Self::GpuHeavyMirror>(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq, Default, serde::Deserialize)]
    struct TestComponent {
        value: i32,
    }

    // Minimal hand-written `EngineClass` impl -- in real components this
    // comes from `#[derive(EngineClass)]`, but this test struct exists only
    // to exercise `WorldComponentRegistration`'s plumbing, not the
    // reflection macro.
    impl EngineClass for TestComponent {
        fn class_name() -> &'static str {
            "TestComponent"
        }
        fn get_properties(&self) -> Vec<pulsar_reflection::PropertyMetadata> {
            Vec::new()
        }
        fn create_default() -> Box<dyn EngineClass> {
            Box::new(Self::default())
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
        fn clone_boxed(&self) -> Box<dyn EngineClass> {
            Box::new(self.clone())
        }
    }

    fn test_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
        world.get::<TestComponent>(entity).map(|c| c as &dyn EngineClass)
    }

    fn test_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
        // `World::get_mut` returns `Mut<'_, T>` (SceneDB's GPU dirty-mark
        // guard) as of the pulsar_scenedb rev this workspace pins post-
        // 2026-08-15 -- `.into_inner()` extracts the raw reference, same
        // fix as `engine_class_derive`'s generated `get_as_engine_class_mut`
        // shim (this hand-written fn mirrors what that macro emits).
        world.get_mut::<TestComponent>(entity).map(|c| c.into_inner() as &mut dyn EngineClass)
    }

    fn test_hydrate(world: &mut World, entity: Entity, data: &Value) -> Result<(), String> {
        let parsed: TestComponent = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
        world.insert(entity, parsed);
        Ok(())
    }

    fn test_remove(world: &mut World, entity: Entity) {
        let _ = world.remove::<TestComponent>(entity);
    }

    fn test_on_removed(_owner: &RuntimeComponentOwner, _context: &mut dyn ComponentRuntimeContext) {}

    fn test_dispatch(
        world: &World,
        entity: Entity,
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _context: &mut dyn ComponentRuntimeContext,
    ) -> bool {
        world.get::<TestComponent>(entity).is_some()
    }

    inventory::submit! {
        WorldComponentRegistration {
            class_name: "TestComponent",
            component_type: pulsar_scenedb::component_id::<TestComponent>,
            hydrate: test_hydrate,
            remove: test_remove,
            dispatch: test_dispatch,
            get_as_engine_class: test_get,
            get_as_engine_class_mut: test_get_mut,
            on_removed: test_on_removed,
        }
    }

    struct DummyContext {
        subsystems: pulsar_reflection::Subsystems,
    }

    impl ComponentRuntimeContext for DummyContext {
        fn subsystems_mut(&mut self) -> &mut pulsar_reflection::Subsystems {
            &mut self.subsystems
        }
        fn project_root(&self) -> &std::path::Path {
            std::path::Path::new(".")
        }
        fn report_error(&mut self, _message: String) {}
    }

    fn dummy_context() -> DummyContext {
        DummyContext { subsystems: pulsar_reflection::Subsystems::new() }
    }

    fn dummy_owner(props: &HashMap<String, Value>) -> RuntimeComponentOwner<'_> {
        RuntimeComponentOwner {
            scene_object_id: "test",
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props,
        }
    }

    #[test]
    fn registered_test_component_is_discoverable() {
        assert!(registered_world_component_classes().any(|c| c == "TestComponent"));
    }

    #[test]
    fn hydrate_dispatch_remove_round_trip() {
        let mut world = World::new();
        let entity = world.spawn();
        let props = HashMap::new();
        let mut ctx = dummy_context();

        // Not hydrated yet -- dispatch is a clean no-op, not a panic.
        assert!(!dispatch_world_component_for_class(
            "TestComponent", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));

        let hydrated = hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 42}),
        )
        .unwrap();
        assert!(hydrated);
        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 42 }));

        assert!(dispatch_world_component_for_class(
            "TestComponent", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));

        assert!(remove_world_component_for_class("TestComponent", &mut world, entity));
        assert!(world.get::<TestComponent>(entity).is_none());
    }

    #[test]
    fn live_get_mut_edits_the_one_real_world_value_directly() {
        let mut world = World::new();
        let entity = world.spawn();

        // Not hydrated yet -- no live value to edit.
        assert!(get_world_component_as_engine_class_mut("TestComponent", &mut world, entity)
            .is_none());
        assert!(get_world_component_as_engine_class("TestComponent", &world, entity).is_none());

        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 1}),
        )
        .unwrap();

        // Mutate through the `&mut dyn EngineClass` path -- this must be the
        // same storage `world.get::<TestComponent>` sees afterward, not a
        // copy: no serialize/deserialize anywhere in this path.
        {
            let instance =
                get_world_component_as_engine_class_mut("TestComponent", &mut world, entity)
                    .expect("hydrated component should be live-accessible");
            let concrete = instance.as_any_mut().downcast_mut::<TestComponent>().unwrap();
            concrete.value = 99;
        }

        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 99 }));
        let read_back = get_world_component_as_engine_class("TestComponent", &world, entity)
            .expect("component should still be live-accessible for reading");
        assert_eq!(
            read_back.as_any().downcast_ref::<TestComponent>(),
            Some(&TestComponent { value: 99 })
        );
    }

    #[test]
    fn notify_removed_dispatches_the_registered_hook() {
        // `on_removed` deliberately takes no `World`/`Entity` at all (see its
        // doc) -- the only way to observe it fired is a side channel.
        thread_local! {
            static FIRED: std::cell::Cell<bool> = std::cell::Cell::new(false);
        }
        fn recording_on_removed(_owner: &RuntimeComponentOwner, _context: &mut dyn ComponentRuntimeContext) {
            FIRED.with(|f| f.set(true));
        }

        // A distinct component TYPE (not just a distinct class name) --
        // `component_type` must be unique per registration for the
        // by-ComponentId lookup test below to mean anything.
        #[derive(Clone)]
        struct NotifyRemovedTestComponent2;

        // A second registration, distinct class name, so this test's
        // `inventory::submit!` doesn't collide with `TestComponent`'s.
        inventory::submit! {
            WorldComponentRegistration {
                class_name: "NotifyRemovedTestComponent",
                component_type: pulsar_scenedb::component_id::<NotifyRemovedTestComponent2>,
                hydrate: test_hydrate,
                remove: test_remove,
                dispatch: test_dispatch,
                get_as_engine_class: test_get,
                get_as_engine_class_mut: test_get_mut,
                on_removed: recording_on_removed,
            }
        }

        let props = HashMap::new();
        let owner = dummy_owner(&props);
        let mut ctx = dummy_context();

        assert!(!FIRED.with(|f| f.get()), "must not have fired before notify is called");
        assert!(notify_world_component_removed("NotifyRemovedTestComponent", &owner, &mut ctx));
        assert!(FIRED.with(|f| f.get()), "notify must invoke the registered on_removed hook");

        assert!(!notify_world_component_removed("NotRegistered", &owner, &mut ctx));

        FIRED.with(|f| f.set(false));
        assert!(notify_world_component_removed_by_component_id(
            pulsar_scenedb::component_id::<NotifyRemovedTestComponent2>(),
            &owner,
            &mut ctx,
        ));
        assert!(FIRED.with(|f| f.get()), "the ComponentId-keyed lookup must find the same registration");

        // A ComponentId nothing registered (this test's own bare marker
        // type) must be a clean no-op, not a panic.
        struct NeverRegistered;
        assert!(!notify_world_component_removed_by_component_id(
            pulsar_scenedb::component_id::<NeverRegistered>(),
            &owner,
            &mut ctx,
        ));
    }

    #[test]
    fn unregistered_class_is_a_clean_no_op_everywhere() {
        let mut world = World::new();
        let entity = world.spawn();
        let props = HashMap::new();
        let mut ctx = dummy_context();

        assert_eq!(
            hydrate_world_component_for_class(
                "NotRegistered", &mut world, entity, &serde_json::json!({})
            )
            .unwrap(),
            false
        );
        assert!(!remove_world_component_for_class("NotRegistered", &mut world, entity));
        assert!(!dispatch_world_component_for_class(
            "NotRegistered", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));
    }

    #[test]
    fn hydrate_surfaces_malformed_json_as_an_error() {
        let mut world = World::new();
        let entity = world.spawn();

        let err = hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!("not an object"),
        )
        .unwrap_err();
        assert!(!err.is_empty());
        // A failed hydrate must not leave a half-inserted component behind.
        assert!(world.get::<TestComponent>(entity).is_none());
    }

    #[test]
    fn hydrate_overwrites_a_previous_value() {
        let mut world = World::new();
        let entity = world.spawn();

        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 1}),
        )
        .unwrap();
        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 2}),
        )
        .unwrap();

        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 2 }));
    }
}
