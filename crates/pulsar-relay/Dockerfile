# Multi-stage Docker build for Pulsar MultiEdit

# Stage 1: Builder
FROM rust:1.96-slim@sha256:26abcef3d79b8d890c4ceb17093154573e1f6479cf6dd7c1450043b8458350f6 AS builder

WORKDIR /app

ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/pulsar-relay/build.rs crates/pulsar-relay/
COPY crates/pulsar-relay/proto crates/pulsar-relay/proto

# Copy every workspace member's Cargo.toml and a matching stub src file
# so Cargo can resolve the full workspace without needing real source
RUN --mount=type=bind,target=/src \
    find /src/crates /src/ui-crates /src/agent-providers -maxdepth 2 -name 'Cargo.toml' | while read f; do \
      rel="${f#/src/}"; \
      dir="/app/$(dirname $rel)"; \
      mkdir -p "$dir/src"; \
      cp "$f" "/app/$rel"; \
      if grep -q '^\[\[bin\]\]' "$f"; then \
        echo 'fn main() {}' > "$dir/src/main.rs"; \
      else \
        touch "$dir/src/lib.rs"; \
      fi; \
    done

# Create dummy main to cache dependencies
RUN mkdir -p crates/pulsar-relay/src && \
    echo "fn main() {}" > crates/pulsar-relay/src/main.rs && \
    cargo build --release -p pulsar-relay && \
    rm -rf crates/pulsar-relay/src

# Copy actual source code
COPY crates/pulsar-relay/src crates/pulsar-relay/src

# Build the application
RUN touch crates/pulsar-relay/src/main.rs && \
  cargo build --release -p pulsar-relay --bin pulsar-relay

# Stage 2: Runtime
FROM debian:bookworm-slim@sha256:0104b334637a5f19aa9c983a91b54c89887c0984081f2068983107a6f6c21eeb

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -u 1000 -g users pulsar

# Copy binary from builder
COPY --from=builder /app/target/release/pulsar-relay /usr/local/bin/pulsar-relay

# Set ownership
RUN chown pulsar:users /usr/local/bin/pulsar-relay

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
