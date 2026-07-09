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
| `.agents/ECS.md` | Archetype ECS — `World`, `Query`, `Schedule` |
| `.agents/THEMING.md` | Theme JSON schema, syntax highlighting, window backgrounds |

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
```

See `.agents/` for detailed docs on each subsystem.
