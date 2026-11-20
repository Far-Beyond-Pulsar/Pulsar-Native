# Add a Language

How a scripting language (and optionally its language server) is added: one plugin whose root registers one `Language`, with a front end that compiles a class's source to a script `Module` through `ModuleBuilder`. Reference: `.agents/SCRIPT_LANGUAGES.md` (the contract, the builder, what the VM can express). The plugin mechanics are `/add-plugin`; the shipped examples are `plugins/vendor/typescript` (compiler + server) and `plugins/vendor/rust` (server only).

---

## Step 1: decide what the language is

- Which **marker file** makes a directory a class (`class.foo`)? Nobody else may use it.
- Which **VM types** does it need? The VM has `int`, `float`, `bool`, `string`, `Entity`, components, registered value types, lists, maps, tuples, and no closures or structs (`.agents/SCRIPT_LANGUAGES.md`). Write down the subset and what each construct lowers to before writing code; reject the rest with a message.
- Does it carry a **server**? Which executable, how it is found, what roots it runs in.

## Step 2: the plugin

Copy the shape of `plugins/vendor/typescript` (`/add-plugin` for the descriptor, `editor_only = true`, no `provides`):

```
plugins/vendor/foo/
├── pulsar-plugin.toml          id = "pulsar.foo", root = "plugin_foo", editor_only = true
├── Cargo.toml                  root crate plugin_foo; compiler as a path dependency
├── compiler/                   foo_compiler: front end -> Module (GPUI-free, depends on pulsar_script_vm only)
└── src/{lib,language,compile}.rs
```

`src/lib.rs`:

```rust
pulsar_plugins::installed!("pulsar-plugin.toml", build);

pub const LANGUAGE: plugin_editor_api::Language = plugin_editor_api::Language {
    origin: pulsar_plugins::origin!(),
    id: "foo",
    display_name: "Foo",
    file_extensions: &["foo"],
    language_ids: &[],
    highlight_id: "foo",          // a grammar id of the highlighter's table
    class_markers: &["class.foo"],
    config_files: &[],
    compiler: Some(script_language),
    server: None,                 // or Some(&server::SPEC)
};

pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(LANGUAGE);
    // p.add(PluginSettings { origin: p.origin(), id: "", schema: settings::schema });   // own settings
}
```

Name no other language anywhere: the one-language-per-class rule is the host's (`pulsar_class::language`).

## Step 3: the front end, through `ModuleBuilder`

In `compiler/`: parse, type-check against the script types and `natives`, and lower with `pulsar_script_vm::module_builder` (`ModuleBuilder`, `FunctionBuilder`, labels, loops, `call_native`, `add_variable`, `set_loc`, `finish()` which verifies). Rules:

1. Check first, build second: report source mistakes with positions from your own diagnostics; the builder's `BuildError` is for module-level problems (map it to an internal-error diagnostic).
2. Resolve native calls only through the registry you are handed; check argument types with `binary_result`/`unary_result` and the native signature before emitting.
3. Give every variable a stable id (keep it across renames in a file next to the source, as `class.schema.json` does) and set `class_version`; without ids state migrates by name.
4. Record debug info (`SourceLoc::at(file, line, column)`) for every statement.
5. Never emit what the VM cannot run; reject it.

## Step 4: the `ScriptLanguage`

`src/language.rs` implements `pulsar_language::ScriptLanguage`: `compile_project` (every class that is this language's alone: `pulsar_class::is_owned_by(dir, "foo")`; write with `ModuleStore::for_class_dir(dir)` and `store.write(&entry, "foo", source_hash, json)`; `store.remove(&entry.id)` when a class fails), `validate_project`, and `check_source` for live diagnostics of unsaved text (source `Pulsar`; return `line:column` locations). Gate the live check on a plugin setting if it is costly.

## Step 5: the server (optional)

`LanguageServerSpec` as a `static` (`server.rs`): `find`/`locate` (setting, project-local, `PATH`, an approved managed install; a missing server is `LocateError::missing` with what to do), `roots`, `initialization_options`/`configuration`, hooks (`readiness`, `progress`), `prepare_workspace`. The host runs the process; the standard settings group (`enabled`, `path`, `args`, `initialization_options`, `trace`, `start`) is derived from the registration.

## Step 6: wire and test

1. Add the plugin to the workspace (`Cargo.toml` members and dependency), then regenerate the installed block: `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`.
2. Front end: a golden test (fixture classes against checked-in modules, as `typescript/compiler/tests/golden.rs`), a test per diagnostic, behaviour on the VM (`Program::link`, `Vm`), a parity test against another language's module if one exists.
3. Plugin: `compile_project` writes modules; a failing recompile removes the stale module; a class owned by another language is not compiled.
4. Assembly in `plugins/installed/tests` (the only place that links plugins with the kernel): the language is registered, its marker makes class dirs, disabling the plugin removes the language, markers, server, settings group and diagnostics (see `languages.rs`, `typescript_language.rs`). A packager test like `pulsar_package/tests/typescript_compile.rs`.
5. `cargo check --workspace --all-targets`, `cargo clippy --no-deps` on touched crates, `just arch`.

## Checklist

- [ ] `Language` registered once, from the root `build`; no other language named in the plugin
- [ ] Class marker unique; classes found with no per-language host code
- [ ] Front end builds through `ModuleBuilder`; `finish()` verifies; golden test
- [ ] Stable field ids and `class_version`; debug info on every statement
- [ ] `compile_project`, `validate_project` (and `check_source`) implemented; stale modules removed
- [ ] Server (if any): locate order, missing is a status, managed install is approved never silent
- [ ] Installed block regenerated; `plugins/installed/tests` assembly test; disable test
- [ ] Layer gate clean; language terms in `[kinds]` only after scanning; docs: `.agents/CRATES.md` table row
- [ ] Submodule: own repository `Plugin_<Name>`, pin to its `main` (`.agents/CRATES.md`, "Submodule pins")
