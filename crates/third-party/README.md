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
