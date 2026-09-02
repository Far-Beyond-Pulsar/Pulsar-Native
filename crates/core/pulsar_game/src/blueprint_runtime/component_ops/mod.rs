//! Component-op execution bridge for the bytecode VM.
//!
//! The VM calls component operations (`comp_*::Class::Member` and the #654
//! identity ops, arena ABI in `pbgc::bytecode::comp_ops`) through plain
//! `DispatchFn` pointers that carry only arena addresses — no world, no
//! entity. This module closes that gap with a thread-local execution
//! context:
//!
//! 1. [`run_with_component_context`] installs `{&mut World, Option<Entity>}`
//!    for the duration of one event execution.
//! 2. Blueprint preparation patches comp-op calls to the trampolines in
//!    [`trampolines`] ([`component_op_handlers()`] hands out their
//!    addresses).
//! 3. Each trampoline parses the staged operands and routes through
//!    `pulsar_world_registry`'s reflection dispatcher — the same single
//!    dispatch path native scripts and generated actors use — or, for the
//!    identity ops, through `pulsar_game::script_refs`, the exact helpers
//!    generated Rust calls (one resolution implementation per #654).
//!
//! # Invariants
//!
//! * The context holds a raw `*mut World`. It is sound only because
//!   installation happens inside [`run_with_component_context`], where the
//!   caller provably holds `&mut World` for the whole closure, and nested
//!   installation panics. One thread executes one blueprint event at a
//!   time; the snapshot accessors below copy slot contents rather than
//!   holding borrows so helpers may consult the context while running.
//! * Trampolines are `extern "C"` — they must not unwind. Every failure
//!   (no context, unbound instance, stale target, unknown class, bad
//!   arguments) is logged with its typed error and degrades to a null
//!   output rather than aborting. Graph-visible failure outputs remain
//!   future work; the log carries the typed display.
//! * An instance that is registered but not yet bound to a scene entity
//!   (`entity: None` in the context, #648's binding model) skips component
//!   ops with an error log; graphs without component nodes are unaffected.
//! * Pin-targeted ops (#654) take their `(entity, component_index)` from
//!   the reference operand via `script_refs::resolve_pin_target`; a stale
//!   or class-mismatched reference degrades exactly like any other typed
//!   failure (#519 discipline: wrong-class references can never land an
//!   edit).

pub(crate) mod trampolines;

use pulsar_bp_executor::ComponentOpHandlers;
use pulsar_scenedb::{Entity, World};
use std::cell::RefCell;

/// Handler addresses handed to `BpExecutor::prepare_with_component_ops`.
///
/// A function rather than a `const`: function-pointer-to-usize casts are
/// rejected by const evaluation.
pub fn component_op_handlers() -> ComponentOpHandlers {
    use trampolines::*;
    ComponentOpHandlers {
        get: comp_op_get_trampoline as *const () as usize as u64,
        set: comp_op_set_trampoline as *const () as usize as u64,
        call: comp_op_call_trampoline as *const () as usize as u64,
        get_ref: comp_op_get_ref_trampoline as *const () as usize as u64,
        find_by_stable_id: find_by_stable_id_trampoline as *const () as usize as u64,
        find_by_name: find_by_name_trampoline as *const () as usize as u64,
        object_literal: object_literal_trampoline as *const () as usize as u64,
    }
}

/// The world slice one blueprint event executes against.
struct CompExecContext {
    world: *mut World,
    entity: Option<Entity>,
}

thread_local! {
    static COMP_EXEC_CTX: RefCell<Option<CompExecContext>> = const { RefCell::new(None) };
}

/// RAII guard clearing the thread-local context on scope exit, including
/// unwinds from graph logic running inside the VM loop.
struct CompExecGuard;

impl Drop for CompExecGuard {
    fn drop(&mut self) {
        COMP_EXEC_CTX.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

/// Install the context and run `f`.
///
/// Panics on nested installation: re-entry would alias two `&mut World`
/// borrows behind one context slot.
pub fn run_with_component_context<R>(
    world: &mut World,
    entity: Option<Entity>,
    f: impl FnOnce() -> R,
) -> R {
    COMP_EXEC_CTX.with(|slot| {
        let mut current = slot.borrow_mut();
        assert!(
            current.is_none(),
            "nested blueprint component contexts are not supported"
        );
        *current = Some(CompExecContext { world, entity });
        drop(current);
        let _guard = CompExecGuard;
        f()
    })
}

/// Snapshot of the installed context: `(raw world pointer, bound entity)`.
///
/// Copies the slot contents instead of holding the `RefCell` borrow, so
/// trampoline bodies may consult the context again while running (a
/// pin-targeted op resolving its reference, or a self-targeted
/// `get_component_ref` reading its instance entity).
///
/// SAFETY contract unchanged from [`run_with_component_context`]: the raw
/// pointer is valid only inside the installation closure, where the caller
/// provably holds `&mut World` for the whole body.
pub(super) fn context_snapshot() -> Option<(*mut World, Option<Entity>)> {
    COMP_EXEC_CTX.with(|slot| slot.borrow().as_ref().map(|ctx| (ctx.world, ctx.entity)))
}

/// Run `f` with the installed context's world/entity.
///
/// Returns `None` (after logging) when no context is installed or the
/// running instance is not yet bound to a scene object.
pub(super) fn with_context<R>(
    op: pulsar_bp_executor::CompOpKind,
    class_name: &str,
    member: &str,
    f: impl FnOnce(&mut World, Entity) -> R,
) -> Option<R> {
    let Some((world, entity)) = context_snapshot() else {
        tracing::error!(
            "blueprint {op:?}::{class_name}::{member} ran without a component \
             context — program was not prepared with component handlers"
        );
        return None;
    };
    let Some(entity) = entity else {
        tracing::error!(
            "blueprint {op:?}::{class_name}::{member} ran on an instance not \
             yet bound to a scene object"
        );
        return None;
    };
    // SAFETY: installed by `run_with_component_context`, whose caller held
    // `&mut World` for the whole closure body we are inside of.
    Some(f(unsafe { &mut *world }, entity))
}

/// Run `f` with only the installed context's world (identity ops do not
/// need a bound instance — resolving "door" works from any instance).
pub(super) fn with_world<R>(
    op: pulsar_bp_executor::CompOpKind,
    f: impl FnOnce(&World) -> R,
) -> Option<R> {
    let Some((world, _)) = context_snapshot() else {
        tracing::error!(
            "blueprint {op:?} ran without a component context — program was not \
             prepared with component handlers"
        );
        return None;
    };
    // SAFETY: see `with_context`.
    Some(f(unsafe { &*world }))
}

#[cfg(test)]
mod tests;
