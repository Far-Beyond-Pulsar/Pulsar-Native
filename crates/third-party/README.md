# third-party

Vendored third-party dependency repos. Some are used as path deps, others
as git deps with local reference copies.

| Submodule | Path dep? | Purpose |
|---|---|---|
| `pbgc/` | Yes | Pulsar Blueprint Graph Compiler |
| `graphy/` | Yes | General graph data model and compiler infra |
| `toolbelt/` | No (git) | Tool registry and macros (has its own workspace) |
| `psgc/` | No (git) | Pulsar Shader Graph Compiler (has its own workspace) |
| `pulsar-config/` | Yes | High-performance config management |

## pulsar-reflection (vendored)

The monorepo builds Pulsar-Reflection from `pulsar-reflection/` (workspace
`[patch]` path overrides); the root `Cargo.toml` git pins name the upstream rev
it is based on. Their difference is committed as
`pulsar-reflection/UPSTREAM.patch` and checked in CI
(`scripts/reflection-drift.sh check`), so drift is never silent.

To bump upstream: change the two Pulsar-Reflection `rev` pins in the root
`Cargo.toml`, run `just reflection-drift-update`, and review the patch diff
(removed lines are what upstream now has, added lines are what is still
local). Changing the vendored sources also needs `just reflection-drift-update`.
The goal is an empty patch, at which point the vendored copy can be dropped for
the plain git dependency.
