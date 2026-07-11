# .agents — Pulsar-Native documentation

```
.agents/
  INDEX.md           ← This file
  ROADMAP.md         Project philosophy, core insights, position
  CRATES.md          Crate layout — all 80+ crates organized by category
  ENGINE_LIFECYCLE.md Startup, InitGraph, EngineBackend, subsystems
  PLUGIN_SYSTEM.md   Plugin architecture — permanent DLL, SDK, registries
  FILESYSTEM.md      Virtual filesystem — engine_fs, providers, asset index
  REFLECTION.md      Type system — Reflectable, RuntimeTypeInfo, EngineClass
  ECS.md             Archetype ECS — World, Query, Schedule, Actor
  THEMING.md         Theme JSON schema, syntax highlighting, window backgrounds
```

This directory contains detailed reference documentation for each major
subsystem in the Pulsar engine. The root `AGENTS.md` provides a short
index and workspace commands.

These files are written from the source code, not from design docs. They
describe how the systems actually work today.
