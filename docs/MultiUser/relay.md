# Pulsar Relay

P2P rendezvous, signaling, and relay server for collaborative editing sessions.

## Who This Is For

Pulsar Relay is designed for teams who want **direct peer-to-peer** file synchronization without a central server holding the authoritative copy of project files. It is the right choice when you want every participant to own their own copy of the project on their local machine, with changes synced directly between peers.

Typical users include:

- **Small teams** who want fast, direct file sync without deploying and maintaining a studio server
- **Privacy-conscious users** who prefer their file data never touches a third-party machine
- **Teams behind NAT** who need help discovering each other and punching through firewalls
- **Anyone** who wants the resilience of P2P — if one peer goes offline, the others continue working independently

In P2P mode, there is no single source of truth. Each peer stores the project on their own disk. The relay server only helps peers find each other (signaling) and, when direct connection fails, relays file data through its QUIC transport. Actual file I/O is performed locally by each peer's `P2pFsProvider`.

## How It Works

The relay server provides three services that together enable P2P collaboration:

**Signaling** — Peers connect to the relay via WebSocket to announce their presence, exchange public IP addresses, and negotiate direct connections. The relay maintains session state (who is in the session, their roles, their addresses) and broadcasts peer join/leave events to all participants.

**NAT Traversal** — Most peers sit behind firewalls or NAT routers that block incoming connections. The relay coordinates UDP hole punching by having each peer send probe packets to every other peer's address. Once both peers have "punched" through their respective firewalls, a direct UDP connection opens and the relay steps out of the data path.

**QUIC Relay** — If hole punching fails (e.g., symmetric NAT), peers fall back to routing file data through the relay's encrypted QUIC server. This adds latency but ensures connectivity in all network conditions.

When a peer performs a file operation (read, write, create, delete), their `P2pFsProvider` sends a `SessionMessage` over WebSocket to the relay, which forwards it to the host peer. The host peer performs the actual `std::fs` operation and broadcasts the change to all other peers via the relay.

## Setup

### 1. Install

```bash
cargo install --path crates/pulsar-relay
```

### 2. Configure

Set environment variables or use defaults. No configuration is required to get started:

```bash
export RELAY_HTTP_BIND=0.0.0.0:8080    # WebSocket signaling (default: 127.0.0.1:8080)
export RELAY_QUIC_BIND=0.0.0.0:8443    # QUIC relay (default: 127.0.0.1:8443)
export RELAY_UDP_BIND=0.0.0.0:7000     # NAT hole punching (default: 127.0.0.1:7000)
export RELAY_METRICS_BIND=0.0.0.0:9090 # Prometheus metrics (default: 127.0.0.1:9090)
export RELAY_TLS_CERT_PATH=/path/to/cert.pem  # Optional: custom TLS cert
export RELAY_TLS_KEY_PATH=/path/to/key.pem     # Optional: custom TLS key
export RELAY_LOG_LEVEL=debug            # Optional: log level
export RELAY_MAX_BANDWIDTH=10485760     # Optional: QUIC relay bandwidth limit (bytes/sec)
export RELAY_DB_URL=postgresql://...    # Optional: PostgreSQL for session persistence
```

### 3. Start the server

```bash
cargo run --bin pulsar-relay
# or if installed:
pulsar-relay
```

The server starts four services simultaneously:

| Service | Port | Purpose |
|---------|------|---------|
| HTTP/WS | 8080 | WebSocket signaling for peer discovery and session management |
| QUIC | 8443 | Encrypted relay for P2P file transport (falls back when NAT punching fails) |
| UDP | 7000 | NAT hole punching coordinator |
| Metrics | 9090 | Prometheus metrics endpoint |

By default, the server generates a self-signed TLS certificate for QUIC. For production, provide your own certificates via `RELAY_TLS_CERT_PATH` and `RELAY_TLS_KEY_PATH`.

### 4. Connect from the editor

In the Pulsar editor, start a P2P session by selecting the P2P mode and providing the relay URL:

```
wss://relay.example.com:8080
```

The editor will:

1. Connect to the relay via WebSocket
2. Join or create a session (the first peer becomes the **host** — the peer who owns the project files)
3. Exchange public addresses with other peers via the relay
4. Attempt UDP hole punching for direct connections
5. Fall back to QUIC relay if direct connection fails
6. Swap the filesystem provider to `P2pFsProvider` and begin syncing files

### 5. (Optional) Add persistence

For session recovery across server restarts, configure PostgreSQL:

```bash
export RELAY_DB_URL=postgresql://user:pass@localhost:5432/pulsar_relay
```

Sessions and participant data will be snapshotted to the database. When the server restarts, it restores active sessions from the last snapshot.

## Quick Start

```bash
cargo run --bin pulsar-relay
# Defaults: HTTP=127.0.0.1:8080, QUIC=127.0.0.1:8443, UDP=127.0.0.1:7000
```

## Ports

| Port | Protocol | Purpose |
|------|----------|---------|
| 8080 | HTTP/WS | REST API + WebSocket signaling |
| 8443 | QUIC | Encrypted P2P relay transport |
| 7000 | UDP | NAT hole punching coordinator |
| 9090 | HTTP | Prometheus metrics endpoint |

## Architecture

```mermaid
graph TB
    subgraph Peers["peers"]
        PeerA["Peer A"]
        PeerB["Peer B"]
    end

    subgraph Relay["pulsar-relay"]
        WS["WebSocket Signaling<br/>Port 8080"]
        QUIC["QUIC Relay<br/>Port 8443"]
        NAT["NAT Traversal<br/>Port 7000"]
        Store["SessionStore + CRDTs<br/>ORSet, RGASeq"]
    end

    PeerA -->|WebSocket signaling| WS
    PeerB -->|WebSocket signaling| WS
    WS -->|P2P negotiation| NAT
    PeerA -->|QUIC direct| PeerB
    WS --> Store

    style WS fill:#f9f,stroke:#333
    style QUIC fill:#bbf,stroke:#333
    style NAT fill:#bfb,stroke:#333
    style Store fill:#fbb,stroke:#333
```

## Two Transport Modes

### Mode 1: Hosted (via pulsar-studio)

```mermaid
graph LR
    A["Editor A"] -->|WebSocket| Studio["pulsar-studio"]
    B["Editor B"] -->|WebSocket| Studio
    Studio --> Disk[("host disk<br/>std::fs")]
```

- All file I/O happens on the host machine
- File changes broadcast via WebSocket to all peers
- No QUIC/NAT traversal needed
- Single source of truth: host's disk

### Mode 2: Peer-to-Peer

```mermaid
graph TB
    subgraph Phase1["Phase 1 — Signaling"]
        A1["Editor A"] -->|WebSocket| Relay["pulsar-relay"]
        B1["Editor B"] -->|WebSocket| Relay
        Relay --> NAT["NAT punch"]
    end

    Phase1 --> Phase2

    subgraph Phase2["Phase 2 — Direct P2P"]
        A2["Editor A"] <-->|QUIC| B2["Editor B"]
    end
```

- Peers discover each other via WebSocket signaling
- NAT traversal via UDP hole punching
- Direct QUIC connection for file data transfer
- pulsar-relay only relays signaling; actual file data goes P2P

## WebSocket Signaling Protocol

### Session Lifecycle

#### 1. Join Session

```json
// Client → Server
{ "type": "join", "session_id": "...", "peer_id": "...", "join_token": "..." }

// Server → Client
{ "type": "joined", "session_id": "...", "peer_id": "...", "participants": [...], "join_token": "..." }
```

#### 2. Peer Joins (broadcast to all)

```json
{ "type": "peer_joined", "session_id": "...", "peer_id": "...", "profile": {...} }
```

#### 3. Leave / Kick

```json
// Client → Server
{ "type": "leave", "session_id": "...", "peer_id": "..." }

// Server → Client (kicked peer)
{ "type": "kicked", "session_id": "...", "reason": "..." }

// Server → Others (broadcast)
{ "type": "peer_left", "session_id": "...", "peer_id": "..." }
```

### P2P Negotiation

```json
// Peer A → Server
{ "type": "p2p_connection_request", "session_id": "...", "peer_id": "...",
  "public_ip": "1.2.3.4", "public_port": 9000 }

// Server → Peer B
{ "type": "p2p_connection_request", "session_id": "...", "from_peer_id": "A",
  "public_ip": "1.2.3.4", "public_port": 9000 }
```

Each peer exchanges their public IP/port with the server, then the server relays the peers' addresses to each other. After this, peers connect directly via QUIC.

### File Sync Messages (Relayed)

| ClientMessage | ServerMessage | Direction | Purpose |
|---------------|---------------|-----------|---------|
| `RequestFileManifest` | `FileManifest` | guest → host | Fetch full file tree |
| `RequestFiles` | `FilesChunk` | guest → host | Request specific files |
| `FileChanged` | `FileChanged` | any → all | Notify file change |
| `RequestProjectTree` | `ProjectTreeResponse` | guest → host | Git project tree |

In P2P mode, the **host** is the peer that owns the project files. The relay server merely forwards these messages between peers.

### Presence Messages

| Type | Description |
|------|-------------|
| `chat` | Chat message relay |
| `cursor_update` | Cursor position relay |
| `ping` / `pong` | Keepalive |

## QUIC Relay

### Server (`QuicServer`)

- Self-signed or custom TLS certificates
- Post-quantum key exchange (X25519MLKEM768)
- Bandwidth-limited stream relay
- Connection tracking + Prometheus metrics
- ALPN protocol: `pulsar-relay`

### P2P Endpoint

```rust
QuicServer::create_p2p_endpoint(bind_addr, allow_insecure).await
```

- Client-side QUIC endpoint for P2P connections
- Uses platform CA roots by default (secure)
- `allow_insecure` option for dev with self-signed certs
- Skips server certificate verification when insecure mode is enabled

### Stream Relay

Each QUIC connection supports up to 100 bidirectional streams. Streams relay data between peers:

```mermaid
graph LR
    A["Peer A"] -->|send stream| Relay["pulsar-relay"]
    Relay -->|send stream| B["Peer B"]
    A2["Peer A"] <--|recv stream| Relay2["pulsar-relay"]
    Relay2 <--|recv stream| B2["Peer B"]
```

## NAT Traversal

### UDP Hole Punching (`UdpHolePuncher`)

1. Each peer sends their public address to the relay via WebSocket
2. Relay exchanges addresses between peers
3. Peers send probe packets to each other's addresses
4. Once both peers have "punched" through their NAT, direct UDP communication is possible

### TCP Simultaneous Open (`TcpSimultaneousOpen`)

Alternative NAT traversal method using TCP simultaneous connection establishment.

## CRDT Data Structures

| Structure | Type | Purpose |
|-----------|------|---------|
| `ORSet` | Observed-Remove Set | Conflict-free replicated set (for collaborative data) |
| `RGASeq` | Replicated Growable Array | Conflict-free replicated sequence (for ordered data / documents) |

These are general-purpose CRDTs for collaborative editing, not filesystem-specific. They are relayed between peers via `StateUpdate` messages.

## Persistence

- PostgreSQL for session snapshots
- S3 for session data persistence
- Enables session recovery across server restarts

## Authentication

JWT-based auth with role management:

| Role | Permissions |
|------|-------------|
| `Host` | Full access, can kick peers |
| `Editor` | Read/write access |
| `Observer` | Read-only access |

## Configuration

| Env Var | Default | Description |
|---------|---------|-------------|
| `RELAY_HTTP_BIND` | `127.0.0.1:8080` | HTTP/WS bind address |
| `RELAY_QUIC_BIND` | `127.0.0.1:8443` | QUIC bind address |
| `RELAY_UDP_BIND` | `127.0.0.1:7000` | UDP hole punching bind |
| `RELAY_METRICS_BIND` | `127.0.0.1:9090` | Prometheus metrics bind |
| `RELAY_TLS_CERT_PATH` | — | Custom TLS cert path |
| `RELAY_TLS_KEY_PATH` | — | Custom TLS key path |
| `RELAY_LOG_LEVEL` | `info` | Log level |
| `RELAY_MAX_BANDWIDTH` | — | Relay bandwidth limit (bytes/sec) |
| `RELAY_DB_URL` | — | PostgreSQL connection string |

## Key Files

| File | Purpose |
|------|---------|
| `src/lib.rs` | Crate root, re-exports |
| `src/config.rs` | Configuration management |
| `src/http_server.rs` | HTTP API + WebSocket signaling server |
| `src/transport/quic.rs` | QUIC server + P2P endpoint |
| `src/rendezvous/sync_protocol.rs` | Wire protocol types (ClientMessage, ServerMessage) |
| `src/rendezvous/session.rs` | Session lifecycle management |
| `src/nat.rs` | UDP hole punching + TCP simultaneous open |
| `src/crdt/mod.rs` | ORSet and RGASeq CRDT implementations |
| `src/session.rs` | SessionStore (session management) |
| `src/auth.rs` | JWT authentication and role management |
| `src/persistence/` | PostgreSQL + S3 persistence layer |
| `src/metrics.rs` | Prometheus metrics |
| `src/health.rs` | Kubernetes health check endpoints |
| `src/telemetry.rs` | OpenTelemetry tracing |

## Relationship to Other Components

| Component | Relationship |
|-----------|-------------|
| `engine_fs::P2pFsProvider` | Uses `pulsar_multiplayer_core::transport::SessionChannel` to communicate with pulsar-relay |
| `pulsar_multiplayer_core` | Provides `SessionChannel` trait, protocol types (`SessionMessage`), session types (`SessionInfo`, `Role`, `SessionMode`) |
| `pulsar-studio` | Alternative to P2P: hosted mode uses HTTP + WebSocket directly, no relay needed |

## P2P Session Mode

When `SessionMode::P2P { relay_url }` is active:

1. `P2pFsProvider` wraps a `SessionChannel` (WebSocket connection to pulsar-relay)
2. File operations (`read_file`, `write_file`, etc.) send `SessionMessage` variants over the channel
3. The relay forwards file-sync messages to the host peer
4. The host peer's `P2pFsProvider` performs actual file I/O via `std::fs`
5. File changes are emitted to all peers via the relay

The relay does **not** perform any filesystem operations itself — it only relays messages between peers who hold the actual files.
