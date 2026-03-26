# Pulsar Host

Dedicated multi-user project server for Pulsar Engine game studios.

## Overview

`pulsar-host` is a self-hosted collaboration server that stores Pulsar Engine project files and coordinates real-time editing sessions between multiple team members. It exposes a REST API consumed by the Pulsar launcher and an editor WebSocket channel for live collaboration.

## Quick Start

```bash
# Build
cargo build -p pulsar-host --release

# Run with defaults (port 7700, data stored in ./pulsar-host-data/)
./target/release/pulsar-host

# Custom configuration
./target/release/pulsar-host \
    --port 7700 \
    --data-dir /srv/pulsar \
    --server-name "Studio A Server" \
    --auth-token "mysecrettoken" \
    --max-projects 50
```

## REST API

All endpoints live under `/api/v1/`.

| Method   | Path                              | Auth | Description                          |
|----------|-----------------------------------|------|--------------------------------------|
| `GET`    | `/api/v1/info`                    | No   | Server metadata & live stats         |
| `GET`    | `/api/v1/projects`                | Opt  | List all projects                    |
| `POST`   | `/api/v1/projects`                | Yes  | Create a new project                 |
| `GET`    | `/api/v1/projects/:id`            | Opt  | Get single project detail            |
| `POST`   | `/api/v1/projects/:id/prepare`    | Yes  | Warm up a project for editing        |
| `DELETE` | `/api/v1/projects/:id`            | Yes  | Archive / remove a project           |
| `WS`     | `/api/v1/projects/:id/session`    | Yes  | Join a live collaborative session    |

### Authentication

Pass a `Bearer <token>` header.  If the server is started without `--auth-token` all requests are treated as authenticated.

### `GET /api/v1/info` response

```json
{
  "server_name": "Studio A Server",
  "version": "0.1.0",
  "active_users":  3,
  "active_projects": 2,
  "uptime_seconds": 3600,
  "max_projects": 50
}
```

### `GET /api/v1/projects` response

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "Main Game",
    "description": "Primary game project",
    "status": "running",
    "user_count": 3,
    "last_modified": "2025-01-15T10:30:00Z",
    "size_bytes": 1073741824,
    "owner": "alice"
  }
]
```

`status` is one of: `idle | preparing | running | error`

### WebSocket session messages

After upgrading to WebSocket on `/api/v1/projects/:id/session`, messages are JSON-encoded `WsMessage` envelopes:

```
client → server:  { "type": "ping" }
server → client:  { "type": "pong" }
server → client:  { "type": "user_joined",  "user": "alice" }
server → client:  { "type": "user_left",    "user": "alice" }
server → client:  { "type": "state_patch",  "patch": { ... } }
```

## Architecture

```
src/
├── main.rs          Entry point: load config, build AppState, start Axum server
├── config.rs        CLI arguments (Clap) and runtime Config struct
├── state.rs         AppState: shared Arc wrapping all managers + config
├── auth.rs          Axum middleware: validates optional Bearer token
├── api/
│   ├── mod.rs       Assembles the Router
│   ├── info.rs      GET /api/v1/info
│   ├── projects.rs  CRUD /api/v1/projects + /prepare
│   └── sessions.rs  WS  /api/v1/projects/:id/session
├── projects/
│   ├── mod.rs
│   ├── types.rs     ProjectRecord, ProjectStatus
│   └── manager.rs   ProjectManager: load/save projects.json, prepare/stop
└── sessions/
    ├── mod.rs
    ├── types.rs     SessionHandle, ConnectedUser, WsMessage
    └── manager.rs   SessionManager: active sessions, user counts, broadcast
```
