# Debug Compile Errors

Patterns for the compile errors that come up most often in this codebase.

---

## "multiple different versions of crate `X`"

```
note: there are multiple different versions of crate `graphy` in the dependency graph
  944b114 ... this is the expected type
  d18837c ... this is the found type
```

**Cause:** Two copies of a shared crate are in the graph: usually a git `rev =` dependency that the
root `[patch]` tables don't redirect to the in-tree copy.

**Fix:**
- Check the `[patch."https://github.com/Far-Beyond-Pulsar/<repo>"]` tables in the root
  `Cargo.toml`. They point git references to Pulsar-Native, Graphy, Pulsar-Reflection, WGPUI and
  others at the submodules; without them Cargo builds a second copy.
- Find the git pins in the plugins:
  ```bash
  grep -rn 'rev =' --include=Cargo.toml plugins/vendor
  ```
  `just check-plugin-pins` checks the Pulsar-Native revs; `just bump-plugin-pins` rewrites them.
- `cargo tree -i <crate> -e normal` shows which package pulls in each copy.

---

## Blueprint compile: "No event nodes found in graph"

The Rust compile target (the default) needs at least one event node (Begin Play, Tick, a custom
event…). Add one, or switch the toolbar to the VM target.

## Blueprint compile: "… Use the VM compile target for this class."

`pulsar_script_codegen::actor::generate_actor` refuses classes with component-scoped event
subscriptions: an exported Rust actor has no runtime event context to resolve that component
reference. Switch the class to the VM target.

## Blueprint compile: "data flow analysis failed: …"

The validation stage (`run_validation_stage` in the plugin's
`src/features/compilation/compiler.rs`) ran Graphy's data-flow analysis on the macro-expanded graph
and found a broken connection or an unknown node type. A node type must exist in the `pulsar_std`
catalogue (`pulsar_std::get_all_nodes()`), be a `native::<name>` native, or be one of the compiler's
own node kinds.

---

## "file not found for module `ExampleClass`"

```
error[E0583]: file not found for module `ExampleClass`
  --> src/classes/mod.rs:10:1
```

**Cause:** `src/classes/mod.rs` declares a class that no longer has a directory with a `mod.rs`,
e.g. after a class was deleted or renamed, or a Rust-target compile appended `pub mod <Name>;` for
a class whose directory later moved.

**Fix:** Run Build Core (or PIE). `ensure_core_bootstrap` in
`crates/core/engine_backend/src/services/core_project_builder.rs` rebuilds `src/classes/mod.rs`
from the class directories on disk. Or delete the stale line by hand.

---

## Generated project fails against old engine crates

A generated game's `Cargo.toml` takes its dependencies and the full `[patch]` table from the
workspace (`crates/core/engine_backend/build.rs` bakes them in). If the project's `Cargo.lock`
still holds older revisions, run `cargo update` in the project, or use the "Update, Build & Run"
build configuration, the default configuration that includes the Update step.

---

## "panic in a function that cannot unwind" / BorrowMutError in GPUI

```
thread 'main' panicked at 'BorrowMutError'
note: panicked at 'panic in a function that cannot unwind'
```

**Cause:** `cx.listener` borrows the entity mutably. Inside the listener, calling `.update(cx, ...)` on the same entity causes a re-entrant borrow panic. At the ObjC event boundary this becomes non-unwindable.

**Fix:** Use `cx.defer` to escape the borrow before updating:
```rust
cx.listener(|this, _, _window, cx| {
    let val = this.field.clone();
    let entity = this.other_entity.clone();
    cx.defer(move |cx| {
        entity.update(cx, |other, cx| {
            other.handle(val, cx);
        });
    });
})
```

If you need `&mut Window` inside the defer:
```rust
let handle = window.window_handle();
cx.defer(move |cx| {
    let _ = cx.update_window(handle, |_, window, cx| { ... });
});
```

---

## "revision XXXX not found"

```
error: failed to get `graphy` as a dependency of package `blueprint_compiler`
  revision 2e17bb904ecf5eaef7d8d9fa8c7b8a0dfedfd7e8 not found
```

**Cause:** A SHA was written incorrectly (truncated, wrong, or reconstructed from memory).

**Rule:** Take SHAs from git, never from memory: `git rev-parse HEAD` after pushing, or
`git -C <submodule> rev-parse HEAD`.

Verify a SHA exists:
```bash
git cat-file -t <SHA>   # prints "commit" if valid
```

---

## "expected function, found macro `println`"

```
error[E0423]: expected function, found macro `println`
```

**Cause:** `pulsar_std` exports a function named `println` (a Blueprint node). Without
`use pulsar_std::*;` (or `pulsar_std::println`) in scope, the compiler sees the built-in
`println!` macro and rejects a call without `!`.

**Fix:** Import it, or call it by path: `pulsar_std::println(message)`.

---

## "type mismatch in function arguments" on cx.spawn

```
error[E0631]: type mismatch in function arguments
   cx.spawn(async move |cx: AsyncApp| {
```

**Cause:** GPUI's `App::spawn` takes `async move |cx: &mut AsyncApp|` (borrowed), not owned
`AsyncApp`; `Context<T>::spawn` takes `async move |this: WeakEntity<T>, cx: &mut AsyncApp|`.
(`TestAppContext::spawn` is the exception: it passes `AsyncApp` by value.)

**Fix:**
```rust
// Wrong:
cx.spawn(async move |cx: AsyncApp| { ... })

// Correct:
cx.spawn(async move |async_app: &mut AsyncApp| { ... })
```

For timers inside this context, use GPUI's executor, not smol's:
```rust
// Wrong (wrong executor):
smol::Timer::after(Duration::from_millis(250)).await;

// Correct:
async_app.background_executor().timer(Duration::from_millis(250)).await;
```
