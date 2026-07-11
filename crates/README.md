# crates

```
crates/
  core/              Engine fundamentals — ECS, reflection, filesystem, LSP, settings, etc.
  editor/            Editor UI panels — the workspace chrome around the engine (was ui-crates/)
  subsystems/        Integration crates — physics (rapier), rendering, scene graph, friends
  agent-providers/   AI provider crates — one per supported backend (OpenAI, Anthropic, etc.)
  ui/                Vendored WGPUI (gpui-ce) and WGPUI-Component as submodules with path deps
  graphics/          Vendored wgpu fork as a submodule (reference copy; still a git dep)
  third-party/       Vendored smaller deps — pbgc, graphy, pulsar-config as path deps
```
