# crates

```
crates/
  core/              Engine platform crates: project, settings, filesystem, scripting runtime and
                     VM, Blueprint std library, packaging, networking, the engine binary and backend
  editor/            Editor UI crates (ui_*): the shell, panels and tools around the engine
  subsystems/        Integration crates bridging the engine to external runtimes (currently pulsar_scene)
  agent-providers/   AI chat core and tools, plus one crate per backend (OpenAI, Anthropic, Gemini, ...)
  ui/                WGPUI, wgpui-base and wgpui-component (UI framework, git submodules, path deps)
  renderer/          Helio, the renderer (git submodule)
  external/          Shared Far-Beyond-Pulsar repos used as path deps: pulsar-profiling, pulsar-reflection
  third-party/       Vendored smaller deps: graphy, psgc, pulsar-config, toolbelt
  graphics/          Only a README: wgpu comes from crates.io, nothing is vendored here
```

Each directory has its own README where it lists its crates. Plugins live
outside `crates/`, under `plugins/vendor/`. The layer rules (core and editor
depend on no plugin) are in `architecture/layers.toml`.
