# Pulsar Relay — Production Multiplayer Editing Service

**Pulsar Relay** is a production-grade Rust service that provides rendezvous, relay, NAT traversal, and session coordination for Pulsar's collaborative editing features.

## Features

- 🚀 **NAT Traversal**: UDP hole punching, TCP simultaneous open, and QUIC relay fallback
- 🔐 **Security**: Ed25519 signatures, Noise protocol E2E encryption, JWT authentication
- 📊 **Observability**: Prometheus metrics, OpenTelemetry traces, structured JSON logging
- 💾 **Persistence**: PostgreSQL sessions storage, S3 snapshot storage
- ⚡ **High Performance**: Async Rust with Tokio, QUIC transport, optimized relay
- 🎯 **Production Ready**: Health checks, graceful shutdown, horizontal scaling, TLS
- 📡 **CRDT Support**: Built-in OR-Set and RGA for collaborative data structures

## Architecture

```
┌─────────────────────┐
│   HTTP Admin API    │ :8080
│  /health /metrics   │
│  /v1/sessions       │
└──────────┬──────────┘
           │
┌──────────┴──────────┐
│   QUIC Relay        │ :8443
│   (TURN-like)       │
└──────────┬──────────┘
           │
┌──────────┴──────────┐
│   UDP Hole Punch    │ :7000
│   Coordinator       │
└──────────┬──────────┘
           │
    ┌──────┴──────┐
    │  Postgres   │
    │     S3      │
    └─────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.78+ (stable)
- PostgreSQL 14+ (optional)
- S3-compatible storage (optional)

### Build

```bash
cargo build --release
```

### Run

```bash
# With default configuration
cargo run --release

# With custom configuration
PULSAR_HTTP_BIND=0.0.0.0:8080 \
PULSAR_QUIC_BIND=0.0.0.0:8443 \
PULSAR_DATABASE_URL=postgresql://user:pass@localhost/pulsar \
PULSAR_S3_BUCKET=my-snapshots \
cargo run --release
```

## Configuration

Configuration can be provided via environment variables, CLI flags, or config file:

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `PULSAR_HTTP_BIND` | HTTP server bind address | `0.0.0.0:8080` |
| `PULSAR_QUIC_BIND` | QUIC relay bind address | `0.0.0.0:8443` |
| `PULSAR_UDP_BIND` | UDP hole punch bind address | `0.0.0.0:7000` |
| `PULSAR_DATABASE_URL` | PostgreSQL connection URL | None (optional) |
| `PULSAR_S3_BUCKET` | S3 bucket for snapshots | None (optional) |
| `PULSAR_S3_REGION` | S3 region | None (optional) |
| `PULSAR_JWT_SECRET` | JWT signing secret | **CHANGE IN PRODUCTION** |
| `PULSAR_LOG_LEVEL` | Log level | `info` |
| `PULSAR_OTLP_ENDPOINT` | OpenTelemetry OTLP endpoint | None (optional) |

### CLI Flags

```bash
pulsar-relay --help
```

### Config File

Create `config.json` or `config.toml`:

```toml
http_bind = "0.0.0.0:8080"
quic_bind = "0.0.0.0:8443"
udp_bind = "0.0.0.0:7000"
database_url = "postgresql://user:pass@localhost/pulsar"
s3_bucket = "pulsar-snapshots"
s3_region = "us-east-1"
max_sessions = 10000
relay_bandwidth_limit = 10485760  # 10 MB/s
log_level = "info"
```

## API Endpoints

### Health & Metrics

- `GET /health` — Full health check
- `GET /health/liveness` — Liveness probe
- `GET /health/readiness` — Readiness probe
- `GET /metrics` — Prometheus metrics (text format)
- `GET /metrics/json` — Metrics in JSON format

### Session Management

- `POST /v1/sessions` — Create new session
  ```json
  {
    "host_id": "user123",
    "metadata": {}
  }
  ```

- `POST /v1/sessions/{id}/join` — Join session
  ```json
  {
    "join_token": "...",
    "peer_id": "user456"
  }
  ```

- `POST /v1/sessions/{id}/close` — Close session
- `GET /v1/sessions/{id}` — Get session info

### WebSocket Signaling

- `GET /v1/signaling` — WebSocket endpoint for rendezvous

## Docker

### Build Image

```bash
docker build -t pulsar-relay:latest .
```

### Run Container

```bash
docker run -p 8080:8080 -p 8443:8443 -p 7000:7000/udp \
  -e PULSAR_JWT_SECRET=your-secret-here \
  pulsar-relay:latest
```

## Kubernetes Deployment

### Deploy

```bash
kubectl apply -f kubernetes/
```

### Configuration

Edit `kubernetes/deployment.yaml` to configure:
- Database URL (via secret `pulsar-secrets`)
- S3 bucket and region
- Resource limits
- Replica count

### Scaling

The service includes HorizontalPodAutoscaler that scales based on:
- CPU utilization (target: 70%)
- Memory utilization (target: 80%)
- Active sessions (target: 1000 per pod)

```bash
kubectl get hpa pulsar-relay-hpa
```

## Monitoring

### Prometheus Metrics

Key metrics exposed on `/metrics`:

- `pulsar_sessions_active` — Active session count
- `pulsar_sessions_total` — Total sessions created
- `pulsar_relay_bytes_total` — Total relay bandwidth
- `pulsar_hole_punch_success_total` — Successful hole punches
- `pulsar_hole_punch_duration_seconds` — Hole punch latency
- `pulsar_p2p_success_ratio` — P2P connection success rate

### Tracing

Configure OpenTelemetry endpoint:

```bash
export PULSAR_OTLP_ENDPOINT=http://jaeger:4317
```

### Logging

Structured JSON logs with correlation IDs:

```json
{
  "timestamp": "2025-01-08T12:00:00Z",
  "level": "info",
  "target": "pulsar_relay::session",
  "span": {"name": "create_session"},
  "session_id": "abc123",
  "message": "Session created"
}
```

## Database Schema

```sql
CREATE TABLE sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) UNIQUE NOT NULL,
    host_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE TABLE snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    snapshot_id VARCHAR(255) UNIQUE NOT NULL,
    session_id VARCHAR(255) NOT NULL,
    s3_key VARCHAR(1024) NOT NULL,
    s3_bucket VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    compressed BOOLEAN NOT NULL DEFAULT false,
    hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);
```

## Security

### TLS Configuration

Provide TLS certificates:

```bash
export PULSAR_TLS_CERT=/path/to/cert.pem
export PULSAR_TLS_KEY=/path/to/key.pem
```

Or let the service generate self-signed certificates (development only).

### JWT Secrets

**IMPORTANT**: Change the default JWT secret in production:

```bash
export PULSAR_JWT_SECRET=$(openssl rand -base64 32)
```

### Ed25519 Server Key

Generate a persistent server key:

```bash
# Generate key (use a proper Ed25519 tool)
export PULSAR_SERVER_ED25519_KEY=$(base64 < key.bin)
```

## Development

### Run Tests

```bash
cargo test
```

### Run with Debug Logging

```bash
RUST_LOG=debug cargo run
```

### Format Code

```bash
cargo fmt
```

### Run Clippy

```bash
cargo clippy -- -D warnings
```

### Security Audit

```bash
cargo audit
```

## Performance

Tested configuration:
- **Sessions**: 10,000+ concurrent
- **Relay throughput**: 10 GB/s aggregate
- **P2P success rate**: >85% (varies by NAT type)
- **Hole punch latency**: <500ms p95
- **Memory**: ~2GB @ 10k sessions

## Troubleshooting

### Health Check Fails

```bash
curl http://localhost:8080/health
```

Check logs for database/S3 connectivity issues.

### QUIC Connection Fails

Ensure UDP port 8443 is open and reachable.

### Hole Punching Fails

- Check NAT type compatibility (symmetric NAT is hardest)
- Verify UDP port 7000 is accessible
- Review `pulsar_hole_punch_*` metrics

## License

See root LICENSE file.

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests and linters
5. Submit a pull request

## Support

- Issues: https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues