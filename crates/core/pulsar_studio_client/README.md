# pulsar_studio_client

The WebSocket client of the studio relay: the multiuser session protocol and connection, with no UI or engine dependency.

- Owns: the client and server message types, peer profiles and identity, connection status, and the client connection.
- Entry points: `MultiuserClient`, `ClientMessage`, `ServerMessage`, `ConnectionStatus`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): none.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: UI and engine crates (its Cargo description: no UI or engine dependency).
- See: [`.agents/MULTIUSER_EDITING_SPEC.md`](../../../.agents/MULTIUSER_EDITING_SPEC.md).
