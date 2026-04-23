# Multi-stage Docker build for Pulsar Host

# Stage 1: Builder
FROM rust:1.88-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./

# Copy every workspace member's Cargo.toml and a matching stub src file
# so Cargo can resolve the full workspace without needing real source
RUN --mount=type=bind,target=/src \
    find /src/crates /src/ui-crates -maxdepth 2 -name 'Cargo.toml' | while read f; do \
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
RUN mkdir -p crates/multiuser_host/src && \
    echo "fn main() {}" > crates/multiuser_host/src/main.rs && \
    cargo build --release -p pulsar-host && \
    rm -rf crates/multiuser_host/src

# Copy actual source code
COPY crates/multiuser_host/src crates/multiuser_host/src

# Build the application
RUN touch crates/multiuser_host/src/main.rs && \
    cargo build --release -p pulsar-host

# Stage 2: Runtime
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -u 1000 -g users pulsar

# Copy binary from builder
COPY --from=builder /app/target/release/pulsar-host /usr/local/bin/pulsar-host

# Set ownership
RUN chown pulsar:users /usr/local/bin/pulsar-host

# Switch to non-root user
USER pulsar

# Expose ports
# 9090 - HTTP API
EXPOSE 9090

# Set environment variables
ENV RUST_LOG=info
ENV PULSAR_HOST_BIND=0.0.0.0:9090

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/pulsar-host", "--version"] || exit 1
