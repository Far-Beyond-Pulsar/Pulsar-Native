# pulsar-multiplayer-core

Shared protocol types, session model and transport abstractions for multiplayer collaboration.

- Owns: the session message protocol, authentication, replication, the session and participant model, and the transport channel trait.
- Entry points: the `prelude` module (`SessionMessage`, `SessionChannel`, `Replicator`, `SessionAuth`, `Role`, `SessionInfo`).
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
- See: [`.agents/FILESYSTEM.md`](../../../.agents/FILESYSTEM.md) (the peer-to-peer provider).
