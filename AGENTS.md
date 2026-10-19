# Pulsar-Native

A next-generation Rust game engine where the editor is the runtime and the
runtime is the editor. For the full context, see `.agents/`.

## Quick index

| File | What |
|---|---|
| `.agents/ROADMAP.md` | Project philosophy, core insights, architecture overview |
| `.agents/CI.md` | CI workflows, cost rules, how to trigger the full cross-platform run |
| `.agents/CRATES.md` | Complete crate layout — core, editor, subsystems, agents, vendored |
| `.agents/ENGINE_LIFECYCLE.md` | InitGraph startup, EngineBackend, Subsystem lifecycle |
| `.agents/PLUGIN_SYSTEM.md` | Permanent DLL pattern, plugin SDK, loading, registries |
| `.agents/FILESYSTEM.md` | `engine_fs::virtual_fs` — local, remote, P2P providers |
| `.agents/REFLECTION.md` | Type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass` |
| `.agents/ECS.md` | SceneDB's archetype ECS — `World`, queries, journals, `Schedule`, actors |
| `.agents/SCENEDB_MIGRATION.md` | Scene data as built: SceneDB ownership, component instances, data flow, observers, renderer, architecture checks |
| `.agents/FRAMES.md` | Parent-relative transforms, GPU hierarchy resolve (as built), and the planned two-tier frame design for sublevels/portals |
| `.agents/LENS.md` | Camera lens — graph placement, components, element kinds, presets, prescription bank, masks, overscan, user WGSL hook |
| `.agents/LAYOUT.md` | Layout persistence — `.pulsar/layout.json` v2 slots, resolve/validate/commit restore, restorers + dormant tabs, zoom, chrome, pop-outs |
| `.agents/FILE_MANAGER.md` | `ui_file_manager` — flat crate layout, modules, conventions |
| `.agents/THEMING.md` | Theme JSON schema, syntax highlighting, window backgrounds |
| `.agents/UI_CRATES.md` | UI crate conventions — flat layout, `components/`/`handlers`/`utils` pattern |
| `.agents/MEMORY.md` | How memory is held and bounded (`Residency`, texture size cap), settings, and the Mission Control Memory Explorer |

## Project data (`.pulsar/`)

| Path | What |
|---|---|
| `.pulsar/import_db.json` | Linked source imports: source, hash, native asset, importer, and for a rigged model the other files it made (`asset_import`) |
| `.pulsar/layout.json` | Per-user editor layout: dock tree, zoom, drawers, window geometry, pop-outs, one slot per project window (`ui_core::app::layout`, see `.agents/LAYOUT.md`); gitignored |
| `.pulsar/import_options.json` | Legacy import options; linked entries migrate into `.pulsar/import_db.json`, while convert-in-place entries remain a fallback |
| `.pulsar/trash/` | Sources replaced by a convert-in-place import, recoverable |

## Workspace

```toml
default-members = ["crates/core/engine"]
members = ["crates/core/*", "crates/editor/*", "crates/subsystems/*", "crates/agent-providers/*"]
```

## Key commands

```
just check        cargo check
just build        cargo build -p pulsar_engine
just test         cargo test --workspace
just clippy       cargo clippy --workspace -- -D warnings
just submodule-init   git submodule update --init --recursive
just check-submodule-pins   every submodule pinned to a commit on its upstream main
```

Submodule changes merge dependency-first: merge the dependency PR into its
main, re-pin the submodule to that main commit, then merge the Pulsar PR (see
"Submodule pins" in `.agents/CRATES.md`).

See `.agents/` for detailed docs on each subsystem.
