# Scripting Epic — Agent Pipeline Context

Master context for the six sequential implementation agents (A→F) building
Pulsar-Native issue #516 to the finished product. Epic: #633. Every agent
MUST read this file first, then its own letter's section, then the GitHub
issues for its letter.

## Architecture target (invariant across all agents)

Gameplay logic — Rust crates or Blueprint graphs, compiled to VM bytecode or
native Rust via PBGC — manipulates the world exclusively through lightweight
references (`ActorRef` / `ComponentRef`) that resolve into **SceneDB**
(`pulsar_scenedb::World`). SceneDB is the only runtime world state; editor,
renderer, PIE, standalone games, and scripts share it. All property/method
dispatch funnels through Pulsar-Reflection metadata + caller closures.

Key upstream facts (verified Aug 2026):

- SceneDB pin `c761890`: Subsystems API merged (`Subsystem::simulate_a/b`,
  `SubsystemRegistry::dispatch`, `SceneDb::step()` — already called every
  frame by `HelioRenderer` at `renderer.rs:1562`).
- Subscriptions shipped (#47): `subscribe_id(entity, ComponentId)` → batched
  `take_component_change_events`; `Mut` guard fires events only on real
  writes.
- Reflection has method metadata + caller closures
  (`MethodMetadata.caller`, `REGISTRY.get_method`) but zero consumers yet.
- `Entity` = packed slot|generation u64, slots recycled on despawn;
  `StableId(String)` is save/load identity (`world_store.rs:14-22`).
- The properties panel (#519/#575) established conventions this epic adopts:
  component identity `(class_name, component_index)`;
  live-typed-vs-metadata routing; subscribe-once/cache-until-signaled UI.

## Execution order and inter-agent contracts

Agents run strictly in sequence. Each appends a handoff report to the end of
this file under `## Handoff: <letter>` describing exactly what landed, what
was stubbed, and what downstream letters must know. Each agent must ALSO read
the previous handoffs before starting.

- **A (one world)** delivers: play mode reading/hydrating SceneDB; TickLoop on
  the shared store; lights/mesh residency improvements; PIE ABI v2 direction.
  Downstream promise: there exists ONE world handle pattern
  (`Arc<RwLock<WorldSceneStore>>` / `SceneDb`) that B–F code against.
- **B (object model)** delivers: `ActorRef`/`ComponentRef` value types,
  StableId↔Entity resolution service, never-panic error taxonomy,
  Reflectable identities. Downstream promise: one crate/path C, D, E, F use
  for all identity handling.
- **C (reflection core)** delivers: `invoke_component_method(...)` dispatcher
  in `pulsar_world_registry` + unified JSON⇄Any⇄bytes marshalling + metadata
  audit. Downstream promise: D/E call THIS api, never bespoke dispatch.
- **D (VM path)** delivers: comp_* opcodes, executor context reaching the
  world, per-entity dispatcher instances, level.json bindings format.
  Downstream promise: F can assume graphs compile+run against live state in
  the VM target.
- **E (Rust path)** delivers: sourcegen actors using live World via C's API,
  signature-drift fix + CI compile check, script-crate authoring UX.
  Downstream promise: generated code style is stable for F's node UX.
- **F (editor UX)** delivers: cross-object reference pins/nodes surviving
  codegen, bidirectional palette filtering, validation stage + Problems
  panel. Consumes everything above.

## Quality bar (NON-NEGOTIABLE, every agent)

1. **File organization**: one concern per module. Target < ~300 lines/file;
   split earlier when a file accumulates distinct responsibilities. New
   crates follow the repo flat-layout convention (see `.agents/UI_CRATES.md`,
   `.agents/FILE_MANAGER.md`). Name modules by domain nouns, not "utils"
   dumping grounds.
2. **Structure**: public API surfaces get `///` doc comments stating purpose
   and invariants (repo style); internals stay `pub(crate)` unless needed.
   Errors are typed enums with `thiserror`-style Display, never `String`.
   No god-objects: pass narrow contexts, not whole app state.
3. **Consistency**: mimic surrounding code (naming, GPUI patterns, existing
   registries). Reuse before inventing: subscriptions, `Mut` guard hooks,
   `pulsar_world_registry` bridges, #519/#575 panel conventions.
4. **Tests**: every new module gets unit tests next to it; integration tests
   for cross-crate contracts listed in each issue's acceptance criteria.
   Regression tests reference their issue number in the test name/doc.
5. **Verification before handoff**: `just check` clean; `just test` no NEW
   failures (pre-existing: two `toggle_button.rs` doctests fail on main;
   workspace clippy has pre-existing warnings — do not fix unrelated ones,
   but touched files must be warning-clean).
6. **Commits**: one commit per issue (`#NNN: imperative-mood summary`),
   on branch `scripting-epic`. Never push. Leave the tree green.
7. **Honesty**: if an issue cannot be completed, stop cleanly, mark the
   remaining checklist items clearly, and document why in the handoff —
   a smaller finished slice beats a sprawling half-broken one.

## Repository folder inventory (agents have NO prior knowledge of these)

All work happens in **`D:\GitHub\Pulsar-Native`** (canonical checkout,
branch `scripting-epic`). Reference repos are **READ-ONLY** — never push,
never edit their files. If an upstream change seems required, build a thin
local shim inside Pulsar-Native instead and record the needed upstream
change in your handoff.

| Path | Role | Relevance |
|---|---|---|
| `D:\GitHub\Pulsar-Native` | The engine. ALL code changes here. | Primary |
| `D:\GitHub\Pulsar-Native\crates\renderer\helio` | Helio renderer **submodule** (at `2d7960e9`, has uncommitted user change — do NOT touch submodule pointers or commit inside it) | Read-only reference |
| `D:\GitHub\Pulsar-Native\plugins\vendor\*` | Vendored plugin checkouts (blueprint_editor, shader_editor, …). Separate git repos compiled into the build. Commit inside them only when a signature change forces it; parent pointers stay untouched. | As needed (D/F) |
| `D:\GitHub\SceneDB` | `pulsar_scenedb` source. Local main `2ceb8a9` is AHEAD of the workspace pin `c761890` (Cargo.toml:94) — read newer code for context but write against the pinned API. | Read-only reference |
| `D:\GitHub\pulsar_reflection` | Reflection registry (`EngineClass`, `MethodMetadata.caller`, `DYN_METHOD_REGISTRY`). Local `745ee78` == the pinned rev. | Read-only reference |
| `D:\GitHub\Template-Blank` | Sample game-project template ("mygame") consumed by core_project_builder scaffolding. | E3 |

Explicitly NOT relevant (do not explore): `Nightly`, `WGPUI*`/`wgpui-p*`,
`gpui-component`, `blog*`, `Pulsar-Installer`, `Helio-cpp`, `helio-ffi`,
`Helio_Standalone_Demo`, `helio-imgui-test`, `Quasar`, `Studio`, `Nebula`,
`Research`, `db`, `mc`, `test`, `wiremann`, `crab_tunnel`, `Solid3D`,
`PiFrost-Infra`, `Documents-Migrator`, `profiling`, `Plugin_*`,
`ue4-sample-project`, `UI`, `far-beyond-pulsar.github.io`,
`tristanpoland.github.io`.

Environment notes for agents:

- Windows + PowerShell. No `head`, no ripgrep CLI — use the Grep/Glob/Read
  tools. Quote paths with spaces. Long cargo builds: pass generous timeouts.
- `gh` CLI is authenticated; fetch your letter's issues with
  `gh issue view <n> --repo Far-Beyond-Pulsar/Pulsar-Native --comments`.
- Pre-existing failures you must NOT chase: two `toggle_button.rs` doctests
  fail on clean main; workspace clippy warns on many untouched files.
- The helio submodule shows as modified (`2d7960e9`) — that is the USER's
  intentional change. Leave it out of every commit.
