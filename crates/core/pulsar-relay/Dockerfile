# Multi-stage Docker build for Pulsar Relay
# Builds only the pulsar-relay binary from the workspace.

# Stage 1: Builder
FROM rust:1.98-slim@sha256:17d1ba895198f9934c6314ec5346a0d5115372f3243390c3d731e242f35c2f27 AS builder

WORKDIR /app

ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/core/pulsar-relay/Cargo.toml crates/core/pulsar-relay/
COPY crates/core/pulsar-multiplayer-core/Cargo.toml crates/core/pulsar-multiplayer-core/
RUN mkdir -p crates/core/pulsar-relay/src crates/core/pulsar-multiplayer-core/src \
    && echo "fn main() {}" > crates/core/pulsar-relay/src/main.rs \
    && echo "" > crates/core/pulsar-multiplayer-core/src/lib.rs \
    && cargo build --release -p pulsar-relay --bin pulsar-relay 2>/dev/null || true \
    && rm -rf crates/core/pulsar-relay/src crates/core/pulsar-multiplayer-core/src

# Copy the rest of the workspace
COPY . .

# Build only the relay binary
RUN cargo build --release -p pulsar-relay --bin pulsar-relay

# Stage 2: Runtime
FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -g 1000 pulsar && useradd -r -u 1000 -g pulsar pulsar

# Copy binary from builder
COPY --from=builder /app/target/release/pulsar-relay /usr/local/bin/pulsar-relay

# Set ownership
RUN chown pulsar:pulsar /usr/local/bin/pulsar-relay

# Switch to non-root user
USER pulsar

# Expose ports
# 8080 - HTTP admin API
# 8443 - QUIC relay
# 7000/udp - UDP hole punching
EXPOSE 8080 8443 7000/udp

# Set environment variables
ENV RUST_LOG=info
ENV PULSAR_HTTP_BIND=0.0.0.0:8080
ENV PULSAR_QUIC_BIND=0.0.0.0:8443
ENV PULSAR_UDP_BIND=0.0.0.0:7000

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/pulsar-relay", "--version"] || exit 1

# Run the application
ENTRYPOINT ["/usr/local/bin/pulsar-relay"]
