# Pulsar-Native

A next-generation Rust game engine where the editor is the runtime and the
runtime is the editor. For the full context, see `.agents/`.

## Quick index

| File | What |
|---|---|
| `.agents/ROADMAP.md` | Project philosophy, core insights, architecture overview |
| `.agents/CRATES.md` | Complete crate layout — core, editor, subsystems, agents, vendored |
| `.agents/ENGINE_LIFECYCLE.md` | InitGraph startup, EngineBackend, Subsystem lifecycle |
| `.agents/PLUGIN_SYSTEM.md` | Permanent DLL pattern, plugin SDK, loading, registries |
| `.agents/FILESYSTEM.md` | `engine_fs::virtual_fs` — local, remote, P2P providers |
| `.agents/REFLECTION.md` | Type system — `Reflectable`, `RuntimeTypeInfo`, `EngineClass` |
| `.agents/ECS.md` | SceneDB's archetype ECS — `World`, queries, journals, `Schedule`, actors |
| `.agents/SCENEDB_MIGRATION.md` | Scene data as built: SceneDB ownership, component instances, data flow, observers, renderer, architecture checks |
| `.agents/FILE_MANAGER.md` | `ui_file_manager` — flat crate layout, modules, conventions |
| `.agents/THEMING.md` | Theme JSON schema, syntax highlighting, window backgrounds |
| `.agents/UI_CRATES.md` | UI crate conventions — flat layout, `components/`/`handlers`/`utils` pattern |

## Project data (`.pulsar/`)

| Path | What |
|---|---|
| `.pulsar/import_db.json` | Linked source imports: source, hash, native asset, importer (`asset_import`) |
| `.pulsar/import_options.json` | Import options per native asset (`engine_fs::import_options`) |
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
