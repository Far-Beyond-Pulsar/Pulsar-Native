# Debug Compile Errors

Patterns for the compile errors that come up most often in this codebase.

---

## "multiple different versions of crate `X`"

```
note: there are multiple different versions of crate `graphy` in the dependency graph
  944b114 ... this is the expected type
  d18837c ... this is the found type
```

**Cause:** One or more repos still pin the old rev of a shared dep.

**Fix:**
```bash
# Find every rev= pin for graphy and pbgc across all repos
grep -r "graphy\|pbgc" \
  ~/Documents/GitHub/Pulsar-Native \
  ~/Documents/GitHub/Plugin_Blueprints \
  --include="Cargo.toml" | grep "rev ="
```

Find the stale SHA and update it to match the current head. Then cascade up via `/bump-deps`.

Also check the `[patch]` sections in `Pulsar-Native/Cargo.toml` — they redirect git refs to local paths. Without them, Cargo resolves a second copy of every Pulsar-Native crate.

---

## "cannot find function `X` in this scope" inside `mod logic`

```
error[E0425]: cannot find function `print_number` in this scope
  --> src/classes/MyBP/events/events.rs:53:68
```

**Cause:** The generated `mod logic { }` block inside `events.rs` is missing `use pulsar_std::*;`. Rust sub-modules don't inherit `use` statements from their parent module.

**Fix:** In `PBGC/src/project.rs`, the `mod logic { }` template must include:
```rust
mod logic {{
    #[allow(unused_imports)]
    use super::super::super::vars::*;
    #[allow(unused_imports)]
    use pulsar_std::*;
    ...
```
After fixing, push PBGC and cascade revs. Also patch the already-generated `events.rs` in the project file directly for immediate testing.

---

## "the trait bound `&MyStruct: EngineClass` is not satisfied"

```
error[E0277]: the trait bound `&classes::MyBP::events::events::MyBP: EngineClass` is not satisfied
   = note: required for the cast from `Box<&MyBP>` to `Box<(dyn EngineClass + 'static)>`
```

**Cause:** `clone_boxed(&self)` does `Box::new(self.clone())`. Without `#[derive(Clone)]`, `self.clone()` resolves to `<&T>::clone` which returns `&T`, not `T`. So `Box<&T>` doesn't satisfy `Box<dyn EngineClass>`.

**Fix:** Add `Clone` to the derive in the generated struct:
```rust
#[derive(EngineClass, Clone)]
pub struct MyBP {}
```
In the PBGC template (`project.rs`), the line should read `#[derive(EngineClass, Clone)]`.

---

## "file not found for module `ExampleClass`"

```
error[E0583]: file not found for module `ExampleClass`
  --> src/classes/mod.rs:10:1
```

**Cause:** `src/classes/mod.rs` declares a class that no longer has a directory. Happens when a class was deleted or renamed in the editor.

**Fix — for the project:** Delete the stale line from `src/classes/mod.rs`.

**Fix — in the generator:** `ensure_core_bootstrap` in `core_project_builder.rs` now regenerates `classes/mod.rs` from disk on every Build Core press (scans which subdirectories have a `mod.rs`). The old code only created it if missing.

---

## "unresolved import `pulsar_game::blueprint_runtime`"

```
error[E0432]: unresolved import `pulsar_game::blueprint_runtime`
  --> src/engine_main.rs:10:18
```

**Cause:** The project's `Cargo.lock` is pinned to an old Pulsar-Native commit that predates `blueprint_runtime` being added to `pulsar_game`.

**Fix:** Run `cargo update` in the project root. Build Core now does this automatically before `cargo build`.

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

**Rule:** Always use `git rev-parse HEAD` immediately after `git push`. Never type a SHA manually.

```bash
git push && git rev-parse HEAD   # copy this output into Cargo.toml
```

Verify a SHA exists:
```bash
git cat-file -t <SHA>   # prints "commit" if valid
```

---

## "expected function, found macro `println`"

```
error[E0423]: expected function, found macro `println`
  --> events.rs:59:68
```

**Cause:** `pulsar_std` exports a function named `println` (not the std macro). Without `use pulsar_std::*;` in scope, the compiler sees the built-in `println!` macro instead and rejects the call-without-`!`.

**Fix:** Same as the missing `use pulsar_std::*;` fix above.

---

## "type mismatch in function arguments" on cx.spawn

```
error[E0631]: type mismatch in function arguments
   cx.spawn(async move |cx: AsyncApp| {
```

**Cause:** GPUI's `App::spawn` in this codebase takes `async move |cx: &mut AsyncApp|` (borrowed), not owned `AsyncApp`.

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
