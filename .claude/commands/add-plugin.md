# Add a Plugin

How a project plugin is created: a directory under `plugins/vendor/<dir>` with a descriptor and a root crate. Every object kind, physics, animation, input and the editor tools ship this way. Reference: `.agents/PLUGIN_SYSTEM.md` ("Project plugins"); the small worked example is `plugins/vendor/fog`.

Dynamic `cdylib` editor plugins are a different mechanism (same file, "Dynamic editor plugins"). Component classes: `/add-component`.

---

## The shape

```
plugins/vendor/<dir>/                 its own repository (Plugin_<Kind>), a git submodule
├── pulsar-plugin.toml                the descriptor
├── Cargo.toml                        the ROOT crate (<kind>_plugin): a leaf, with the `editor` feature
├── src/lib.rs                        installed!(...) + `build`; src/runtime.rs, src/editor.rs
├── api/        (optional)            contracts other plugins read: keys, row ABIs, WGSL
├── component/  <kind>_component      SceneDB components, methods, runtime, file formats
├── feature/    helio-pass-*          the Helio feature and passes
├── editor/     <kind>_editor         level editor UI, tools
├── examples/, tests/, ledger.toml, README.md, AGENTS.md
```

Only the root registers anything. The other crates hold code and export plain items (`fog_component::register_columns`, `derivation`, `render_features`); the root's `build` submits them. Other plugins depend only on a plugin's `api` crate, never on its root. An editor tool (`editor_only = true`) is one crate that is its own root.

## Step 1: the descriptor

`pulsar-plugin.toml` (copy `plugins/vendor/fog/pulsar-plugin.toml`):

```toml
id = "pulsar.mykind"
name = "My Kind"
description = "..."
enabled_by_default = true
provides = [
  { id = "pulsar.mykind.classes", kind = "class", members = ["MyComponent"], version = 1 },
]
requires = [
  { id = "pulsar.lights.rows", optional = true },   # a contract, never a plugin id
]
root = "mykind_plugin"
```

- `provides`/`requires` are **contracts** (`kind` = `buffer`, `api`, `class` or `trait`), namespaced by the plugin. An optional requirement nobody provides leaves the plugin running on stand-ins.
- A `ClassSpec::requires` class must be a member of a `class` contract the plugin requires; a use of another plugin's trait needs its `trait` contract; a scene buffer a feature reads needs the provider's `buffer` contract. `plugins/installed/tests/contracts.rs` fails when the descriptor and the Cargo edges disagree.
- Two enabled providers of one contract are a conflict; there is no `replaces =`.

## Step 2: the root crate

`src/lib.rs`, as `plugins/vendor/fog/src/lib.rs`:

```rust
pulsar_plugins::installed!("pulsar-plugin.toml", build);

pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    runtime::build(p);
    #[cfg(feature = "editor")]
    editor::build(p);
}

pub use mykind_component;
pub mod runtime;
#[cfg(feature = "editor")]
pub mod editor;
```

`runtime.rs` (what a game links), grouped by engine seam:

```rust
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.components().class::<MyComponent>(ClassSpec::default());      // /add-component
    p.add(SceneRowsRegistration { origin: p.origin(), name: "mykind.rows", .. });
    p.add(RenderFeatureRegistration { origin: p.origin(), features: .., configure: |c| c });
}
```

Rules (checked by `plugins/installed/tests/installed_set.rs`):

- **Every** registration goes through `p.add(..)` or `p.components()` with `origin: p.origin()`; `add` refuses another origin. The origin is how a disabled plugin's classes, features, settings and editors vanish.
- A build only adds; it never reads a registry.
- A registration linked at compile time (`inventory::submit!` in a plugin crate) instead of added by `build` fails the test, except type attachments that name no origin (reflection method tables, SceneDB GPU mirrors; `.agents/PLUGIN_SYSTEM.md`, "The root crate").
- Name catalog ids kind-prefixed (`fog.rows`, `pulsar.physics.step`).

`editor.rs` (the `editor` feature): the plugin's editor registrations (`CreateEntry`, `EnvironmentListing`, `WorldSettingField`, `PluginEditorProvider`, `StarterContent`, ...). The root `Cargo.toml` makes the editor crate and `ui_level_editor` optional:

```toml
[features]
editor = ["dep:mykind_editor", "dep:ui_level_editor"]
```

A game links the root without `editor`; the editor links it with.

## Step 3: wire it into the workspace

1. Add the plugin as a submodule at `plugins/vendor/<dir>` (`.gitmodules`) and create its repository.
2. Root `Cargo.toml`: its crates in `[workspace] members` and `<kind>_plugin = { path = "plugins/vendor/<dir>" }` (and its other crates) in `[workspace.dependencies]`, as the fog entries.
3. Regenerate the installed set: `UPDATE_INSTALLED_PLUGINS=1 cargo test -p installed_plugins --test installed_set` rewrites the generated block in `plugins/installed/Cargo.toml` from the descriptors. Do not edit that block by hand.
4. A plugin that ships scene data adds `ledger.toml` (checked by `cargo test -p scene_inventory`).

## Step 4: tests

- Unit tests in the root: `registers_its_feature`, and `every_registration_resolves_to_the_plugin` (every contribution with this origin resolves to the plugin id), as `plugins/vendor/fog/src/runtime.rs`.
- Integration tests of the plugin as a game links it: `<dir>/tests/` (`use mykind_plugin as _;` links the descriptor and build).
- A test that needs more than one plugin, or the installed set, belongs to `plugins/installed/tests` (a core or editor crate cannot depend on a plugin, dev dependencies included).
- Reflection metadata: a golden snapshot test in the root (`plugins/vendor/physics_plugin/src/golden_metadata.rs`; regenerate with `PULSAR_UPDATE_SNAPSHOT=1`).
- The leak audit and catalog tests in `plugins/installed/tests` cover every installed class automatically.

```bash
cargo check -p mykind_component -p mykind_plugin --tests
cargo test -p installed_plugins            # installed set, contracts, absent plugins
cargo test -p one_owner                    # every concept has one owner
just arch                                  # layer gate
```

## The layer gate

`just arch` (`architecture/layers.toml`) fails when a kernel crate (`crates/core`, `crates/editor`, `crates/subsystems`) depends on a plugin or a tool, dev dependencies included, or names a plugin's kind beyond the shrink-only `architecture/kind-names.txt`. Core and editor know no kind: what a kind needs from the kernel is a registration type its root submits (`.agents/PLUGIN_SYSTEM.md`, "Where a plugin meets the kernel"). Do not add to `kind-names.txt`.

## Rendering work

Any plugin work that touches Helio or Pulsar's rendering side (features, passes, WGSL, GPU rows, render graphs, object-kind editor tools) must load the `helio-architecture` skill first (`crates/renderer/helio/.agents/skills/helio-architecture/SKILL.md`) and end with its report section.

## Submodules and pins

The plugin is a separate repository, so merge order matters:

1. merge the plugin PR into the plugin repo's default branch;
2. re-pin the submodule here (`git -C plugins/vendor/<dir> fetch origin main && git -C plugins/vendor/<dir> checkout origin/main && git add plugins/vendor/<dir>`);
3. merge the Pulsar-Native PR.

`just check-submodule-pins` fails when a pin is not on its upstream main (a pin waiting on an upstream PR is listed in `.github/submodule-pin-exceptions.txt`). A plugin that needs a Pulsar-Native API change merges the Pulsar PR first and takes the new rev through `just bump-plugin-pins <rev>`; plugins pin Pulsar crates by `rev =` and carry no SceneDB rev (`/bump-deps`).
