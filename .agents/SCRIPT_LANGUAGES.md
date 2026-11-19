# Script languages

How a text (or visual) language becomes a scripting language of the engine, as built.
The plan and its history are `.agents/LANGUAGE_PLUGINS_PLAN.md`; the plugin mechanism is
`.agents/PLUGIN_SYSTEM.md`; the step-by-step is the skill `/add-language`.

The engine knows no language. A language is **one plugin** whose root `build` registers one
`Language`. It compiles a class's source to `pulsar_script_vm` bytecode (a `Module`), and may
bring a language server. Blueprints (`pulsar.blueprints`) and TypeScript (`pulsar.typescript`)
are the two shipped scripting languages; Rust (`pulsar.rust`) is a language with a server and no
compiler. Everything below is what a third language relies on.

## The `Language` registration

`pulsar_language::Language` (category `language`; `plugin_editor_api` re-exports it for plugin
authors). One `const` in the plugin's root crate, added with `p.add(LANGUAGE)`:

| Field | Meaning |
|---|---|
| `origin` | `pulsar_plugins::origin!()`: a disabled plugin's language is absent from every registry |
| `id` | the language's identity: the module store's `language` marker (`"typescript"`) |
| `display_name` | `"TypeScript"`: footer, class type column, messages |
| `file_extensions`, `language_ids` | extensions without the dot; the LSP `languageId` per extension when it is not `id` |
| `highlight_id` | a grammar id of the UI highlighter's table (a foundation table the language names, never owns) |
| `class_markers` | files whose presence makes a directory a class of this language (`class.ts`, `graph_save.json`); empty: the language has no classes |
| `config_files` | project files that change the server's configuration (`tsconfig.json`); they gate an eager server start |
| `compiler` | `Option<fn() -> Arc<dyn ScriptLanguage>>`; `None`: editor-only language |
| `server` | `Option<&'static LanguageServerSpec>`; `None`: highlighting only |

`LanguageRegistry::current()` (kernel crate `pulsar_language`, GPUI-free; cached against
`pulsar_plugins::generation()`) answers `for_path`, `class_languages(dir)`, `is_class_dir`,
`compilers`, `with_servers`, `server_for_path`. The host asks it; nothing outside a language's own
plugin names that language. A duplicate id is ignored and logged.

## The `ScriptLanguage` contract

`pulsar_language::ScriptLanguage` (`Send + Sync`), every method defaulted so a language
implements what it has:

- `compile_project(project_root, natives)` (and the `_with_component_events` and `_for_cook`
  variants, which default to it): compile **every class of this language** under the project
  to modules in the project's module store, with no GPUI and no editor state. This is what CI,
  `pulsar package` and the pre-Play build call. Return every problem as a `CompileDiagnostic`
  (`class`, `file`, `location` as `line:column` or a node id, `message`, severity). Any error
  means the project does not build. `natives` is the registry the game links against: resolve
  native calls through it, never through a private list.
- `validate_project(project_root)`: the same checks without writing; `Err` blocks Play.
- `check_source(ctx, path, text)`: diagnostics for one file's **unsaved** text, nothing
  written. The host calls it on an editor task, debounced (400 ms, one in flight per file,
  latest text wins), on open, edit and save, and publishes the result under
  `DiagnosticSource::Compiler(<language id>)` with source name `Pulsar`, beside the server's.
  A file that is not one of the language's compiled sources yields nothing.
  `live_check_enabled()` lets the language gate it on its own plugin setting.
- `compile_target(class_dir)`: `Some("rust")` when a class asks to be generated into the game
  crate as well. The default is "runs as a script".

A language's plugin enumerates classes with `pulsar_class::is_owned_by(dir, id)` and so is
handed only classes that are its alone.

## The module store and one language per class

A compiled class lives in the project's module store (`pulsar_class::ModuleStore`, keyed by class
id): `store.write(&entry, language_id, source_hash, module_json)` writes the module last, so a
reader never sees a module with another source's record, and a `language` marker file naming the
writer. `ModuleStore::for_class_dir(dir)` gives the entry and store of a class directory;
`pulsar_class::modules::source_hash(files)` is the staleness key (include every file the module
depends on, the TypeScript plugin hashes `class.ts` and `class.schema.json`). When a class fails
to compile, `store.remove(&entry.id)`: no module from an older source may be left to run.

**One language per class**, owned by the host (`pulsar_class::language`): a class has the one
language whose `class_markers` are present. Two markers raise `ClassProblem::MultipleLanguages`
at scan; a module written by another language than the one the registry resolved is
`ModuleFromOtherLanguage`. Both reach the Problems drawer (source `Classes`), the pre-Play
validation and the packager (which fails the build). A language never checks the other's markers
and never names another language.

## Class markers

`class_markers` is how the host finds classes with no per-language code: `is_class_dir`, the file
manager tree (a class folder of a language with no editor of its own for the folder is one item
and opens its first marker), `ui_common` file types, the viewport's placement check, the
packager's "has source but no compiled module" message and `pulsar_traits::class_name_of_asset`.
A language with source files other than the marker (`class.schema.json`) keeps them in the class
directory.

## The server spec and its settings

`LanguageServerSpec` (`pulsar_language`, plain data and function pointers, one `static` in the
plugin): `id`, `display_name`, `find`/`locate` (the executable: the `path` setting, project-local,
`PATH`, a managed install; `LocateError::missing` is a status with what to do, never a silent
no-op), `roots` (`RootStrategy`: `ProjectRoot`, `NearestMarker`, `OutermostMarker`, `Custom`: one
server per root), `initialization_options` and `configuration` (also the answer to
`workspace/configuration`), `client_capabilities`, `hooks` (`progress`, `notification`,
`readiness`: `AllProgressEnded | Signal | Immediate`, `quick_fixes`), `commands`
(`ServerCommandSpec`: the editor's "Language" menu), `prepare_workspace` (written before the
server starts), `installer` (`ServerInstaller`: a managed install the person approves, never
silent) and `eager`. The process is the host's (`pulsar_lsp::LanguageServers`: transport, ids,
documents, status, restart); a plugin never owns a process.

The standard settings group is **derived** from every `Language` with a server
(`pulsar_settings::plugin::SettingsGroups`, owner path `<plugin id>/<server id>`): `enabled`,
`path`, `args`, `initialization_options`, `trace`, `start`. The plugin adds nothing for those.
Its own settings are a `PluginSettings { origin, id: "", schema }` (the TypeScript plugin's is
`compiler_diagnostics`), read through `pulsar_settings::plugin::handle(origin, "")`. Disabling the
plugin withdraws the group.

## Diagnostics sources

`DiagnosticsStore` holds diagnostics by file and source: `Server(server id)` from
`publishDiagnostics` (empty clears), `Compiler(language id)` from `check_source`. They are shown
side by side, never de-duplicated: a server accepts constructs the compiler rejects, so a file is
green in one and red in the other by design. Class problems (`Classes`) and Play problems are
drawer-direct sources. Stopping a server or disabling a plugin clears exactly that source's
entries and announces the files.

## What the VM can and cannot express

A module is functions over **typed registers** with explicit jumps (`Instr`: `Const`, `Move`,
`Unary`, `Binary`, `Jump`, `Branch`, `Call`, `CallNative`, `LoadVar`, `StoreVar`, `SelfEntity`,
`Now`, `Wait`, `Return`, `Collection`), plus per-instance **variables**, **events** and
**subscriptions** (`module.rs` is the reference; `verify.rs` the rules).

- Types (`Type`): `Unit`, `Bool`, `Int` (`i64`), `Float` (`f64`), `Str`, `Entity`,
  `Component(name)` (a liveness-checked reference to a component on some entity),
  `Object(name)` (a registered value type such as `Vec3`), `List(T)`, `Map(K, V)` with a
  `bool`/`int`/`string` key, `Tuple(..)` (what a native with several results returns).
  Collections have value semantics.
- **No closures, no user-defined structs or classes, no references to functions, no
  exceptions, no recursion limit beyond the VM's depth cap, no dynamic dispatch, no strings
  as indexable sequences.** A function is a module function (called by index) or a native
  (called by import). State is the class's variables. Engine types are used only through their
  natives (`Vec3.new_`, `v.add(w)`); there are no operators on `Object` except equality when the
  type registers it (`script_value_ops!`).
- Waiting: `Wait { seconds }` suspends the call in game time; a function that waits is
  resumed by the host with every frame and register intact. There are no other latent
  operations in the instruction set (the `latent` natives and events build on it).
- A handler is a `unit` function whose parameters are a **prefix** of the event's fields.
- Imports carry the full native signature; the linker checks them again against the registry.
- Variables carry a stable `id` and the module a `class_version`: state carries over a reload by
  id (`migrate`), by name without one. A language that supports renames keeps the ids itself (the
  TypeScript plugin's `class.schema.json`).

The TypeScript subset (`plugins/vendor/typescript/compiler`, crate docs) is the reference for
what a text language maps onto this: one class per file, typed fields as variables, methods as
functions, `async` + `await wait(s)` as `Wait`, `await this.m()` as a call that may wait,
`let`/`const` as registers, `if`/`while`/`for`/`break`/`continue`, `x as int|number|string` as
unary conversions, `ns.fn(..)` and `v.method(..)` as native calls, `T.new_(..)` constants as
`Constant::Value`. It rejects, with a message, everything the VM cannot express (closures,
generics, imports, object literals, `switch`, `try`) rather than ignoring it.

## `ModuleBuilder`: building the module

`pulsar_script_vm::module_builder` (re-exported at the crate root). It is in the VM crate, not a
sibling, because it is the module format's other half: a format change and the builder's rules
change in one commit, it needs `Module`, `verify`, `NativeRegistry` and `TypeRegistry` and
nothing else, and so it adds no crate, no layer-gate entry and no dependency a plugin did not
already have (a language plugin already depends on `pulsar_script_vm` for `Module`). It has no
syntax, no GPUI and no plugin in it.

- `ModuleBuilder::new(name, &natives)`: constant pool (`constant`, de-duplicated), imports
  (`import(name)`, de-duplicated, signature from the registry), variables
  (`add_variable(name, ty, default, id)`: the id is the field's stable identity),
  `set_class_version`, events (`declare_event`) and subscriptions (`subscribe`), the registry
  queries (`native`, `natives`, `engine_type`).
- Functions: `declare_function(name, params, ret, exported) -> FunctionHandle` reserves the index
  before any body exists (calls may name later functions); `begin_function(handle)` gives a
  `FunctionBuilder` whose parameters are registers `0..n`; `finish_function(handle, f)` checks
  that every label is bound, every loop closed and every jump lands on an instruction.
- `FunctionBuilder`: `new_reg(ty)`, `reg_type`, `emit(instr)`, `here`, `mov`, `ret`, `wait`,
  `unary`/`binary` (allocate the result register by the verifier's rules, `None` when the
  operator does not apply), `unreachable()` (a jump to itself: gives a function whose every path
  returned an end for jumps to land on); **labels** (`new_label`, `label_here`, `bind`, `jump`,
  `branch`: forward references are patched at `bind`); **loops** (`enter_loop(break_to,
  continue_to)`, `break_jump`, `continue_jump`, `exit_loop`, `in_loop`); **scopes** of named
  locals (`push_scope`, `pop_scope`, `declare_local`, `bind_local`, `declared_in_scope`,
  `lookup` giving a `Local { reg, ty, mutable }`); **debug info** (`set_loc(SourceLoc)`: every
  emitted instruction is recorded under the current location; `SourceLoc::at(file, line,
  column)`).
- Operations over both: `load_constant`, `call(f, handle, args)` and `call_native(f, name,
  args)` (arity and argument types checked, result register allocated by the return type, `None`
  for `unit`), `load_var`, `store_var` (type checked). Methods of values and components are
  natives whose first parameter is the receiver: pass it first.
- `binary_result(op, ty)` and `unary_result(op, ty)`: the result type the verifier (and, for
  equality, the linker) accepts, so a front end can type-check before it emits.
- `finish()` returns the **verified** `Module` or a `BuildError`. The builder reports what makes a
  module wrong; the front end reports its own diagnostics, with positions, for source mistakes
  (check first, then call the builder, as the TypeScript lowering does).

A worked example (the whole back half of a front end), from `tests/module_builder.rs`, which
compiles an s-expression language and runs it on the VM:

```rust
let mut module = ModuleBuilder::new("Toy", &natives);
let max = module.declare_function("max", vec![Type::Int, Type::Int], Type::Int, true)?;

let mut f = module.begin_function(max);
let (a, b) = (f.param_reg(0), f.param_reg(1));
f.set_loc(SourceLoc::at("toy.lisp", 1, 1));
let less = f.binary(BinOp::Lt, a, b)?.expect("an int comparison");   // (< a b)
let (then, otherwise, end) = (f.new_label(), f.new_label(), f.new_label());
let result = f.new_reg(Type::Int)?;
f.branch(less, then, otherwise);
f.bind(then);      f.mov(result, b); f.jump(end);
f.bind(otherwise); f.mov(result, a);
f.bind(end);
f.ret(Some(result));
module.finish_function(max, f)?;

let module: Module = module.finish()?;     // verified
```

The TypeScript lowering (`lower.rs`) is a client of exactly this API: the oxc tree is walked, each
expression is type-checked with the script type system, and the builder is called. Its emitted
modules are pinned byte for byte (JSON and binary) by
`plugins/vendor/typescript/compiler/tests/golden.rs` against `tests/golden/*.module.txt`:
`UPDATE_GOLDEN=1` re-records after an intended change, and the diff is the review.

## A language plugin's layout

```
plugins/vendor/<dir>/                 its own repository (Plugin_<Name>), a submodule
├── pulsar-plugin.toml                id, enabled_by_default, root, editor_only = true
├── Cargo.toml                        the root crate (<name>_plugin) and the compiler as a path dependency
├── compiler/                         <name> front end -> Module through ModuleBuilder; GPUI-free, no plugin dependency
├── src/lib.rs                        installed!(..), `LANGUAGE`, `build` (p.add(LANGUAGE), PluginSettings)
├── src/language.rs                   the ScriptLanguage impl (compile_project, check_source, validate_project)
├── src/compile.rs                    class dirs, ModuleStore writes, schema/identity files
├── src/server.rs, locate.rs, install.rs   (optional) the LanguageServerSpec, discovery, managed install
└── tests/                            plugin and server tests
```

The root crate is an editor tool (`editor_only = true`): a game runs the compiled modules and
never links the language. It depends on `pulsar_language`, `pulsar_plugins`, `pulsar_class`,
`pulsar_settings`, `plugin_editor_api` and `pulsar_script_vm`, and registers only through `build`.
The compiler crate depends on `pulsar_script_vm` (and its parser) only, so tools can use it
without the plugin.

## Testing

- The front end: a golden test per language (compile fixtures, compare to checked-in modules),
  tests for every diagnostic, and a run on the VM (`Program::link`, `Vm`) for behaviour. A parity
  test against an existing language's module is the strongest oracle
  (`blueprint_parity.rs`).
- The plugin: `compile_project` writes modules and a recompile after a failing edit removes the
  stale one; a class another language owns is not compiled.
- The assembly, in `plugins/installed/tests` (the only place that links plugins and kernel
  together: `languages.rs`, `typescript_language.rs`): the language is registered, its markers make
  class dirs, disabling the plugin removes the language, markers, server, settings group and the
  diagnostics of both sources. Regenerate the installed block with
  `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set`.
- A packager check like `pulsar_package/tests/typescript_compile.rs`: a project with a class of
  the language builds headless with the language linked.

## The layer gate

`just arch` (`architecture/layers.toml`): the kernel (`crates/core`, `crates/editor`,
`crates/subsystems`) depends on no plugin or tool, dev dependencies included, and names no
language: `[kinds]` carries language terms (`typescript`, `tsserver`, `tsconfig`, `class ts`,
`oxc`, `rust analyzer`, `rustup`, `graph save`, the compile-to-Rust target) and
`architecture/kind-names.txt` is shrink-only. So a language's name, parser, marker files and server
live in its plugin; a test that needs the plugin and the kernel together belongs to
`plugins/installed/tests`. The builder adds none of this: it names no language. Add the new
language's own terms to `[kinds]` only after running the scanner against the tree, and never to
hide a hit.
