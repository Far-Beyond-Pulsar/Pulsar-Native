//! Handle semantics for script authors and engine contributors (#641).
//!
//! This module is documentation; it compiles to nothing. It is THE one page
//! defining how `ActorRef`/`ComponentRef` behave at language boundaries,
//! so scripts, generated code (E), the VM (D), and graph pins (F) all share
//! one contract instead of each guessing.
//!
//! # References are validated per access
//!
//! A reference is a plain value (`Copy`/`Clone`, no allocation, no locks).
//! It carries NO authority: every accessor re-checks its target against the
//! world it is handed, right then. Consequences:
//!
//! - **Store them freely.** Keep refs in components, arenas, VM frames, or
//!   save files (as [`crate::resolution::SerializedComponentRef`] for those)
//!   for as long as you like -- a stale ref is data, not a dangling pointer.
//! - **Stale is normal.** If the target was despawned since you took the
//!   ref, accessors return `Err(ScriptRefError::ReferenceDespawned)`. That
//!   is an ordinary result, not an error condition in your code.
//! - **No cross-object writes, ever.** Entity slots are recycled with
//!   generation bumps; a ref whose generation no longer matches is refused
//!   before any storage is touched. The property test suite churns
//!   spawn/despawn/reuse under held refs and asserts this holds.
//!
//! # Never panic, never misaddress
//!
//! Every accessor returns a typed error ([`crate::errors::ScriptRefError`])
//! instead of panicking or silently writing somewhere else:
//!
//! | Situation | Result |
//! |---|---|
//! | Target despawned / slot recycled / id never existed here | `ReferenceDespawned` |
//! | Actor alive, class not hydrated on it | `ComponentMissing` |
//! | Index addresses another class's record | `ClassMismatch` |
//! | No instance at that index | `InstanceMissing` |
//! | Class never registered for World residency | `UnregisteredClass` |
//! | Property/method name not in reflection metadata | `UnknownProperty` / `UnknownMethod` |
//! | Value didn't survive JSON⇄typed marshalling | `Marshalling` |
//!
//! The single deliberate exception to "never assert":
//! [`pulsar_scenedb::Entity::DANGLING`] reaching an accessor trips a
//! debug-build assertion (release still returns the typed error). DANGLING
//! is a sentinel that should be stopped at FFI/glue boundaries -- if it
//! reaches here, raw ids crossed without conversion, which is exactly what
//! this contract exists to catch.
//!
//! # Identity rules
//!
//! - An actor's identity WITH one world lifetime is its packed `Entity`
//!   (slot + generation). Across save/load, undo/redo, or editor sessions
//!   it is its `StableId` string -- see [`crate::resolution`].
//! - A component instance's identity is `(class_name, component_index)` --
//!   the same convention as the properties panel (Pulsar-Native#519).
//!   Index 0 (or the instance store's first enabled index) is the
//!   live-typed value living in the World; other indexes are that
//!   instance's own serialized record.
//! - Method calls always dispatch on the live-typed value; duplicates share
//!   their class behavior.
//!
//! # Crossing boundaries
//!
//! | Boundary | Form |
//! |---|---|
//! | Rust ↔ Rust through this crate | `ActorRef` / `ComponentRef` values |
//! | Reflection args/returns (`Box<dyn Any>`) | boxed concrete ref types (see [`crate::reflect`]) |
//! | Raw FFI / dylib boundaries | `Entity::bits()` u64 ONLY; convert back with `Entity::from_bits` and treat the next accessor's validation as the trust boundary |
//! | Save files / graphs at rest | [`crate::resolution::SerializedComponentRef`] (`stable_id`, not entity bits) |
