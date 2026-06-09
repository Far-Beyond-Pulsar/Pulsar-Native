# Multi-User Editing

This directory documents the three core components of Pulsar's collaborative editing infrastructure.

## Architecture Overview

```mermaid
graph TB
    subgraph Editor["Pulsar Native Editor (per peer)"]
        subgraph engine_fs["engine_fs — virtual filesystem"]
            LocalFs["LocalFsProvider<br/>(local disk)"]
            RemoteFs["RemoteFsProvider<br/>(studio HTTP)"]
            P2pFs["P2pFsProvider<br/>(P2P WS)"]
        end
    end

    Editor -->|cloud+pulsar://<br/>(studio host)| Studio["pulsar-studio<br/>Axum HTTP + WS<br/>Port 7700"]
    Editor -->|cloud+pulsar://<br/>(relay)| Relay["pulsar-relay<br/>Signaling + QUIC<br/>Port 8080 / 8443 / 7000"]
    Editor -->|P2P session| Core["pulsar-multiplayer-core<br/>SessionChannel abstraction"]

    Studio --> Disk[("local disk<br/>std::fs")]
    Relay --> P2P["direct QUIC<br/>peer-to-peer"]
```

## Three Deployment Modes

| Mode | Provider | Infrastructure | Description |
|------|----------|---------------|-------------|
| **Local** | `LocalFsProvider` | None | All file I/O via `std::fs` on the local machine |
| **Hosted (Studio)** | `RemoteFsProvider` | `pulsar-studio` | All file I/O via HTTP to a self-hosted `pulsar-studio` server |
| **Peer-to-Peer** | `P2pFsProvider` | `pulsar-relay` | All file I/O via `SessionChannel` over a P2P session |

## Components

- **[README](./README.md)** — This file, architecture overview
- **[studio.md](./studio.md)** — Pulsar Studio: self-hosted file server and session host
- **[relay.md](./relay.md)** — Pulsar Relay: P2P rendezvous, signaling, and QUIC relay

## Key Crates

| Crate | Location | Role |
|-------|----------|------|
| `engine_fs` | `crates/engine_fs/` | Virtual filesystem abstraction with three provider implementations |
| `pulsar-studio` | `crates/pulsar-studio/` | HTTP + WebSocket server for hosted collaborative projects |
| `pulsar-relay` | `crates/pulsar-relay/` | P2P signaling server, NAT traversal, QUIC relay |
| `pulsar_multiplayer_core` | `crates/pulsar-multiplayer-core/` | Session/transport abstractions and protocol types |

## Virtual Filesystem Routing

All file I/O goes through `engine_fs::virtual_fs`, which maintains a global provider singleton:

1. **Default**: `LocalFsProvider` — direct disk access
2. **Cloud path** (`cloud+pulsar://host/proj_id/...`): `RemoteFsProvider` — HTTP to pulsar-studio
3. **P2P session**: `P2pFsProvider` — `SessionChannel` over WebSocket to pulsar-relay

The provider is swapped via `engine_fs::virtual_fs::set_provider()` when opening a project.

## File Change Events

All providers emit `FsEvent` messages via `engine_fs::events::emit()`. In hosted mode, pulsar-studio broadcasts file changes to all connected WebSocket clients. In P2P mode, file changes are sent to all peers via the `SessionChannel`.
