# Plugin system

Pulsar has two kinds of plugin, which meet at one identity and one catalog:

- **Project plugins**: every plugin under `plugins/vendor/<dir>` with a
  `pulsar-plugin.toml` (every object kind, animation, physics, input, the editor tools).
  They link statically into the editor (`plugins/installed`) and, except editor tools, into
  games; a project enables some of them in its `Pulsar.toml`. This is how every
  first-party plugin ships today.
- **Dynamic editor plugins**: native `cdylib`s loaded at startup and never unloaded (the
  permanent library pattern below). They join the catalog in phase 4 of
  `PLUGIN_API_REFACTOR_PLAN.md`.

## Project plugins

### Identity and enablement (`pulsar_plugins`)

`pulsar-plugin.toml` beside the plugin's crates:

```toml
id = "pulsar.fog"                 # the one stable identity
name = "Fog"
description = "Global and local fog media ..."
enabled_by_default = true
requires = []                     # plugins it cannot work without
root = "fog_plugin"               # its root crate: the package in this directory
# editor_only = true              # an editor tool: a game never links it
```

A project lists what it enables (`[plugins] "pulsar.fog" = false` in `Pulsar.toml`);
`pulsar_plugins::resolve` turns that and the descriptors into the enabled set and
`activate_project` makes it the process's set. Every registration names its origin
(`pulsar_plugins::origin!()`, the submitting package); registries iterate
`pulsar_plugins::registered::<T>()`, which skips a registration whose origin is a disabled
plugin's root. A disabled plugin is absent: its classes, features, settings and editors are
not registered.

### The root crate

Each plugin has one root crate, the package at its directory, and it is the **one place the
plugin submits what it contributes**: every origin-gated registration, every component
class, and the descriptor. The plugin's other crates hold the code and register nothing:

| Crate | Holds | Depends on |
|---|---|---|
| `api` (`helio-<kind>-api`) | contracts other plugins read: keys, row ABIs, WGSL | Helio core |
| `feature` (`helio-pass-*`) | the Helio feature and passes | core, other kinds' api crates |
| `component` (`<kind>_component`) | SceneDB components, methods, runtime, file formats | engine crates, api, feature |
| `editor` (`<kind>_editor`) | level editor UI, tools, thumbnails | component, editor crates |
| **root** (`<kind>_plugin`) | **the registrations**: `src/lib.rs`, `src/runtime.rs`, `src/editor.rs` | all of the above; editor ones behind `editor` |

Rules:

- The root is a leaf: no crate depends on it but `plugins/installed` (generated) and a
  generated game. Other plugins depend only on a plugin's `api` crate.
- The editor crate and editor-only dependencies are optional, behind the root's `editor`
  feature. A game links the root without it; the editor (`installed_plugins` with
  `editor`) links it with.
- A plugin's component classes use `#[world_component(...)]` at the type; the root
  registers them with `pulsar_world_registry::submit_world_component!`.
  (`#[register_world_component]` registers at the type and is for the engine's own
  components.)
- Type attachments stay with their type: SceneDB's `GpuMirrorRegistration`,
  `GpuClearRegistration` and `HandleLedgerRegistration`, reflection's method tables and
  property hints, derive output. They name no origin and act only on entities of their
  type, which a disabled plugin's unregistered class never has. An api crate holds none (it
  holds no statics); its attachments move to the root.
- An editor tool (`editor_only = true`) is one crate, its own root; physics and input are
  single crates whose registrations live in `src/registrations.rs`.
- `plugins/installed/tests/installed_set.rs` fails when a plugin registers outside its root
  or when a linked contribution comes from a plugin crate that is not a root.

Why one root: one file lists everything a plugin brings; every registration's origin is the
root, so origin-to-plugin is exact; the host gets the plugin's registrations as one batch
(phase 4 makes that batch explicit for dynamic plugins). This is Bevy's `Plugin::build`
shape over Unreal's descriptor-and-modules model: the descriptor says what the plugin is and
whether a project has it, the root says what it contributes.

### A complete example

`plugins/vendor/fog`, trimmed: the component crate defines the class and the functions, the
editor crate the UI, and the root composes the registrations.

```rust
// fog/component/src/component.rs: the classes, defined at their types.
use engine_class_derive::{engine_class, world_component};

#[engine_class]
pub struct GlobalFogComponent { /* properties */ }
#[engine_class]
pub struct LocalFogVolumeComponent { /* properties */ }

// World classes, registered by the plugin's root (not here).
#[world_component]
impl GlobalFogComponent {}
#[world_component]
impl LocalFogVolumeComponent {}

// fog/component/src/lib.rs: what the root registers, as plain public items.
pub fn register_columns(store: &mut SceneGpuStore, device: &Arc<wgpu::Device>) { /* ... */ }
pub fn derivation(device: &wgpu::Device, keys: RowJoinKeys) -> Box<dyn SceneDerivation> { /* ... */ }
pub fn render_features() -> helio_feature_api::FeatureSet {
    helio_feature_api::FeatureSet::new().with(helio_pass_volumetric_fog::FogFeature)
}
```

```toml
# fog/Cargo.toml: the root crate.
[package]
name = "fog_plugin"
autoexamples = false            # `examples/` is the plugin's examples crate

[features]
# The editor's registrations and the crates they need; a game links the root without it.
editor = ["dep:fog_editor", "dep:ui_level_editor"]

[dependencies]
pulsar_plugins = { workspace = true }
inventory = { workspace = true }
fog_component = { workspace = true }
fog_editor = { workspace = true, optional = true }
engine_backend = { path = "../../../crates/core/engine_backend", default-features = false, features = ["render"] }
pulsar_world_registry = { workspace = true }
ui_level_editor = { workspace = true, optional = true }
```

```rust
// fog/src/lib.rs: the root. Its docs list everything the plugin contributes.
//! Fog (`pulsar.fog`): the plugin's root crate, the one place it registers
//! what it contributes. [`runtime`]: the classes, the scene rows, the
//! feature. `editor`: the environment mixer's entries.

// The descriptor: linking the root installs the plugin.
pulsar_plugins::installed!("pulsar-plugin.toml");

// Hosts and tests reach the plugin's crates through its root.
pub use fog_component;
#[cfg(feature = "editor")]
pub use fog_editor;

pub mod runtime;
#[cfg(feature = "editor")]
pub mod editor;
```

```rust
// fog/src/runtime.rs: what a game links, grouped by engine seam.
use engine_backend::scene::SceneRowsRegistration;
use engine_backend::subsystems::render::RenderFeatureRegistration;
use fog_component::{GlobalFogComponent, LocalFogVolumeComponent};

// ── Component classes ──
pulsar_world_registry::submit_world_component!(GlobalFogComponent, LocalFogVolumeComponent);

// ── Rendering ──
// The media's source rows and the row join that places a local volume at its owner.
inventory::submit! {
    SceneRowsRegistration {
        origin: pulsar_plugins::origin!(),     // the root: the gate's key
        name: "fog.rows",                      // the catalog id, kind-prefixed
        register_columns: fog_component::register_columns,
        derivation: fog_component::derivation,
    }
}

// The volumetric medium and its composite.
inventory::submit! {
    RenderFeatureRegistration {
        origin: pulsar_plugins::origin!(),
        features: fog_component::render_features,
        configure: |config| config,
    }
}
```

```rust
// fog/src/editor.rs: what the editor adds (the `editor` feature).
use fog_editor::level_editor;
use ui_level_editor::extend::{CreateEntry, EnvironmentListing};

// The environment mixer's global fog. Behaviour (and any label using the editor
// crate's `rust_i18n::t!`, which resolves to the calling crate) is a named function
// in the editor crate.
inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_global_fog",
        menu_label: None,
        environment_label: Some("Global Fog"),
        quick_add: None,
        offered: None,
        order: 410,
        create: level_editor::add_global_fog,
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "GlobalFogComponent",
        label: "Global Fog",
        order: 10,
        create_entry: Some("environment_global_fog"),
    }
}
```

The kind's integration tests (`<kind>/tests/`) are the root's: they test the plugin as a
game links it (`use fog_plugin as _;` links its registrations).

### Starter content

A plugin that offers downloadable content (a pack of assets) registers a
`ui_level_editor::extend::StarterContent` from its root's `editor.rs`, not a
Create entry (Create adds one object to the level). It declares the pinned
archive (`archive_url`, `sha256`, `max_bytes`), the licence shown to the author
(`license`, `license_url`), the project-relative `destination`, and `install`:
a function from the verified archive to files written under the destination
(through `engine_fs::virtual_fs`). The shell owns the rest: download on a
worker thread with progress and cancel, the checksum, the phase
(`Downloading`/`Verifying`/`Installing`/`Done`/`Failed`/`Cancelled`) the Tools >
Starter Content menu and the end notification read, and failure reporting. It is
listed in the catalog as `editor.starter_content`. The shell knows no pack: the
first consumer is the sky's CC0 panoramas (`.agents/SKY.md`). Tests:
`extend/starter_content.rs` (checksum, cancel, limit, failure, success against a
loopback server) and `plugins/vendor/sky/tests/starter_content.rs`.

### What a plugin contributes: the catalog

Every registration type summarizes itself (`pulsar_plugins::Contribution`: a category such
as `render.features` or `editor.create_entry`, a stable id, a label, its origin) and is
declared with `pulsar_plugins::registration!(T)`, which also lists it as a
`ContributionSource`. So the catalog reads every registration type linked into the binary
without depending on any of their crates, and the summaries come from the registrations
themselves.

`plugin_manager::catalog::PluginCatalog::snapshot()` joins:

- each installed plugin's descriptor, whether the project enables it, its root crate, and
  its contributions (a render feature registration carries each feature and the nodes it
  registered: `CatalogPlugin::feature_nodes`, from Helio's `NodeRegistry::feature_nodes`);
- `unattributed`: registrations no plugin root submitted (the engine's own), each with its
  origin as provenance;
- `problems`: two registrations of one category with one id from one origin.

`pulsar_plugins::contributions()` lists every linked registration;
`active_contributions()` lists those the registries hand out now (with a plugin disabled,
none of its). A new registration type implements `Contribution` beside its definition and
uses `registration!`; `Registration` is never implemented by hand.

The registration matrix, the raw `inventory` paths and the plugin-to-plugin edges are in
`PLUGIN_API_REFACTOR_PLAN.md` (the audit, sections 0-3; its review, section 9; phase 1 as
built, section 11), which is also the roadmap.

## Dynamic editor plugins

Plugins built as native Rust `cdylib` DLLs are loaded at startup and **never unloaded**.
The DLL plugin system spans two primary crates.

### Architecture

```
plugin_editor_api (SDK)         plugin_manager (host)
┌──────────────────────┐        ┌─────────────────────────┐
│  EditorPlugin trait  │        │  PermanentLibrary       │
│  EditorHandle trait  │        │  FileTypeRegistry       │
│  export_plugin! macro│        │  EditorRegistry         │
│  PluginMetadata      │        │  BuiltinEditorRegistry  │
│  FileTypeDefinition  │        │  PluginToolBridge       │
│  PluginError         │        │  global() accessor      │
│  StatusbarButtonDef  │        └─────────────────────────┘
└──────────────────────┘
         │                           │
         └─────── DLL boundary ──────┘
```

### The permanent library pattern

The engine wraps `libloading::Library` in `ManuallyDrop` so `dlclose` /
`FreeLibrary` is never called. This is the central safety guarantee:

```rust
pub struct PermanentLibrary {
    library: ManuallyDrop<Library>,   // NEVER dropped
    path: PathBuf,
    sha256: [u8; 32],
}
```

Because the library is never unloaded:
- `Arc<T>` can cross the DLL boundary safely (drop glue stays resident)
- Trait object vtables remain valid
- Function pointers remain valid
- No weak-reference bureaucracy needed

The cost: plugins occupy virtual memory until process exit (~2-10 MB each).

### The plugin contract

Every plugin DLL exports two FFI symbols (generated by `export_plugin!`):

```rust
#[no_mangle]
pub unsafe extern "C" fn _plugin_create(
    theme_ptr: *const c_void,
) -> &'static mut dyn EditorPluginFull {
    // Leaks Box<dyn EditorPluginFull> — intentional, never freed
    Box::leak(Box::new(__PluginExport(MyPlugin::default())))
}

#[no_mangle]
pub extern "C" fn _plugin_version() -> VersionInfo {
    VersionInfo::current()
}
```

`VersionInfo` carries the engine version (major.minor.patch) and a FNV-1a hash
of the rustc semver. The engine checks this before loading — mismatched rustc
versions are rejected because type layouts may differ across compiler versions.

### Key traits

**`EditorPlugin`** — the base trait every plugin implements:
```rust
pub trait EditorPlugin: Send + Sync {
    fn metadata(&self) -> PluginMetadata;
    fn file_types(&self) -> Vec<FileTypeDefinition>;
    fn editors(&self) -> Vec<EditorMetadata>;
    fn on_load(&mut self) {}          // One-time init after DLL load
    fn version_info(&self) -> VersionInfo { VersionInfo::current() }
}
```

**`EditorPluginEditor`** — provides editor factories by registering with
`EditorFactoryRegistry` at startup:
```rust
pub trait EditorPluginEditor: EditorPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry);
}
```

**`EditorHandle`** — the per-editor lifecycle, wrapped in a GPUI `Element`:
```rust
pub trait EditorHandle: Send + Sync + 'static {
    fn init(&mut self, editor_id: &EditorId, file_path: &Path,
            window: &mut Window, cx: &mut App) -> Result<(), PluginError>;
    fn render_frame(&self, ctx: &mut EditorFrameCtx) -> AnyElement;
    fn teardown(&mut self, window: &mut Window, cx: &mut App) {}
}
```

Extension traits (all have default implementations):
- **`EditorPluginStatusbar`** — `statusbar_buttons()`, `accepted_drop_kinds()`
- **`EditorPluginAi`** — `ai_tools()`, `execute_ai_tool()`, `capabilities_for_file()`
- **`EditorPluginComponents`** — custom `EngineClass` definitions and factories
- **`EditorPluginSubsystems`** — custom engine `Subsystem`s

**`EditorPluginFull`** is the combined super-trait that the DLL exports:
```rust
pub trait EditorPluginFull: EditorPlugin + EditorPluginEditor
    + EditorPluginStatusbar + EditorPluginAi
    + EditorPluginComponents + EditorPluginSubsystems {}
```

### Plugin components and the World

A plugin's `EditorPluginComponents::component_factories` hands the host a
`Box<dyn EngineClass>` factory, enough for the property panel. A typed World
component also needs its `#[register_world_component]` registration in the
host's registry. A plugin statically links its own copy of the world crates
(`pulsar_reflection`, `pulsar_scenedb`, `pulsar_scene_model`,
`pulsar_world_registry`), so on its own that registration lands in
registries the host never reads.

The world crates share their process-wide state the way WGPUI shares gpui
across plugins (`crates/ui/wgpui/src/shared_runtime.rs`), Pulsar-Native#1083:

- Each crate has a `runtime` module: a `#[repr(C)]` table of function
  pointers into the copy that owns the state (component ids, counters,
  registries), and `attach`, which points this copy at another copy's
  table and hands it this copy's `inventory` registrations.
- `pulsar_world_registry::runtime::WorldRuntimes` gathers the four tables.
  `export_plugin!` exports `_plugin_attach_world_runtime`; the plugin
  manager calls it with the editor's tables before `_plugin_create`.
  Reflection attaches first, then SceneDB, the scene model and the registry.
- Each table carries an ABI fingerprint (version and layouts). A plugin
  built from other world-crate sources or features refuses the editor's
  tables, and the loader rejects it (`WorldRuntimeRefused`). A plugin built
  before #1083 exports no entry point; it loads, and its classes stay local
  (the loader warns).
- Component ids follow type name and layout, not `TypeId`, so a plugin
  built in another cargo invocation shares them.
- A plugin uses typed access only on its own component types. It reaches
  engine types by class name through the registries, because a `TypeId`
  differs between builds. This is how WGPUI hands plugins the theme.
- New process-wide state in a world crate goes through its runtime. The
  `world_state` tests in SceneDB and Pulsar-Reflection, and
  `scene_inventory`'s `world_crate_state_goes_through_the_runtime`, fail on
  an unlisted `static`, `thread_local!` or `inventory::collect!`.

The UI framework is unchanged. Tests:
`crates/core/plugin_manager/tests/world_component_plugins.rs` (fixture
plugins, one built separately) and `plugin_loading.rs`.

### Plugin loading

1. `PluginManager::new()` — creates empty registries
2. `register_all_builtin_editors()` — register in-process editors first
3. `load_plugins_from_dir("plugins/vendor/")` — walk directory for `.so`/`.dylib`/`.dll`
4. For each candidate:
   a. Open file, hash it (SHA-256 from open fd for TOCTOU protection)
   b. `libloading::Library::new()` → wrap in `ManuallyDrop`
   c. Call `_plugin_version()` → `VersionInfo::is_compatible()` check
   d. Call `_plugin_attach_world_runtime(world_runtimes)`; a refusal rejects the plugin
   e. Call `_plugin_attach_event_bus(bus)`
   f. Call `_plugin_create(theme_ptr)` → get `&'static mut dyn EditorPluginFull`
   g. Call `on_load()`, register file types, editors, statusbar buttons
5. `initialize_global(manager)` — store in `OnceCell`

### Built-in vs DLL plugins

Built-in editors implement `BuiltinEditorProvider` and are registered at
startup. They follow the same trait pattern as DLL plugins but live in-process.
The `BuiltinEditorRegistry` collects them, and `PluginManager` unifies both
sources behind the same API.

DLL plugin registrations overwrite built-in entries for the same file type,
allowing DLL plugins to replace built-in behavior.

### File type resolution

Given a file path, the engine:

1. Extract extension → `FileTypeRegistry::get_file_type_for_extension()`
2. FileTypeId → `EditorRegistry::get_editor_for_file_type()`
3. EditorId → `PluginManager` → find owning plugin
4. `plugin.create_editor(editor_id, path, window, cx)` → `Arc<dyn PanelView>`

Support for `FolderBased` file types (e.g., `MyClass.class/` directories with
a marker file) is handled by `FileTypeDefinition.structure`.

### Panel restorers

A saved layout names a panel by file or by a restore key `{ kind, data }`
(`PanelState::with_restore`). A restorer, `kind -> fn(data, window, cx) ->
Option<Arc<dyn PanelView>>`, rebuilds the latter. A plugin registers its
restorers next to its editor factories, in `register_editors`:
`registry.register_restorer("my.panel", |data, window, cx| ...)`. There is no new
FFI symbol and no `EditorPluginFull` vtable change: the registry is host-owned
(a plugin links its own copy of `plugin_editor_api`, so a global there would be
private to the plugin), the host collects the entries after `register_editors`
and merges them into the one `plugin_manager::panel_restorers()`. Built-in code
calls `panel_restorers().register(..)` directly. Layout restore resolves a tab
as: live singleton by `panel_name`, restorer by kind, file editor; a tab whose
kind or file type has no registration is kept *dormant* and written back so it
returns when the plugin does (`ui_core/src/app/layout/dormant.rs`). See
`plugin_editor_api::panel_restore`.

### Statusbar buttons

Plugins return `Vec<StatusbarButtonDefinition>` from `EditorPluginStatusbar`.
Buttons have a position (`Left`/`Right`), icon, tooltip, priority, and action
(`OpenEditor`, `ToggleDrawer`, `Custom` callback). The toolbar is drawn by
`ui_core::render::render_statusbar()`.

### AI tool bridge

`PluginToolBridge` collects AI-accessible tools from all loaded plugins and
built-in providers. Tools are registered per file type and can be executed by
the agent chat system via `execute_ai_tool()`. The bridge also provides a
filesystem sandbox (`FsContext::read_only(project_root)`) that restricts
tool file access to within the project directory.

### Writing a plugin

```rust
use plugin_editor_api::prelude::*;

#[derive(Default)]
struct MyPlugin;

impl EditorPlugin for MyPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new("com.example.my-plugin", "My Plugin", "0.1.0", "me")
    }
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![FileTypeDefinition::standalone("myext", ".myext", "My File", IconName::File)]
    }
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata::new("my-editor", "My Editor", vec!["myext"])]
    }
}

impl EditorPluginEditor for MyPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        registry.register::<MyEditorHandle>("my-editor", "myext");
    }
}

export_plugin!(MyPlugin);
```

The plugin is compiled as:
```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

### Crate map for plugin developers

A plugin's `Cargo.toml` needs:
```toml
[dependencies]
plugin_editor_api = { git = "https://github.com/Far-Beyond-Pulsar/Pulsar-Native" }
gpui-ce = { git = "https://github.com/Far-Beyond-Pulsar/WGPUI" }
ui = { git = "https://github.com/Far-Beyond-Pulsar/WGPUI-Component" }
```

All must be compiled against the same rustc version as the engine binary.

### Import companions

A plugin can offer extra work for an import without the engine knowing what
it is. `plugin_editor_api::import_companion` defines `ImportCompanion`:
`offer(&ImportProbe)` (shown as a checkbox in the import configurator, only
when the plugin has something to say about the sources) and
`run(&ImportCompanionContext)` (after the native asset exists). The data both
ways is neutral: the host passes `SourceSurface`s (a mesh slot, its name and
the image files found for it, by role); the plugin returns `CompanionOutput`s
(files written, optionally "default for slot N").

`asset_import::register_companion` is the host-owned registry (a plugin links
its own copy of the SDK, so a registry there would be private). Built-in
providers register at startup in `ui_core`; the shader plugin registers
`shader_editor_plugin::material_import_companion()`, which writes one
`.material` graph per surface with images. Accepted companion ids are stored in
`ImportRecord::companions`, so a re-import runs them again. Rules: a companion
never overwrites a file that exists, and its outputs only fill mesh slots that
have no default material. Not yet wired for DLL plugins (no FFI entry point).

### Manifests and plugin dependencies

Every `export_plugin!` library also exports `_plugin_manifest` (a
`PluginManifest`: id, name, version, `requires`, `optional`). The host reads
it before any plugin constructor runs. `PluginManager::prepare_plugins_in_dir`
returns plugins in dependency order (`plugin_manager::dependencies::plan_load_order`):
a plugin loads after everything it requires, a plugin with a missing,
out-of-range, cyclic or failed dependency comes back as
`PluginManagerError::DependencyUnmet` with the reason, and plugins with no
dependencies keep directory order. `install_prepared_plugin` re-checks that
each required plugin is installed, so a dependency that was ordered first but
failed to install still blocks its dependents. A library built before
manifests (no `_plugin_manifest`) installs first and cannot be depended on.

`_plugin_create` returns `Option<&'static mut dyn EditorPluginFull>` and
`_plugin_manifest` an `Option`: a panic in a plugin constructor is caught
(`catch_unwind`) and reported as a failed load instead of unwinding across
`extern "C"`.

A plugin declares dependencies by overriding `EditorPlugin::manifest`, or
writes against the registrar API (`plugin_editor_api::registrar`: `Plugin`,
`PluginBuilder`, `export_plugin_v2!`), which presents itself to the host through
the traits above. Roadmap: `PLUGIN_API_REFACTOR_PLAN.md` (phase 4 brings dynamic plugins
onto the root and the catalog).
