# Multi-stage Docker build for Pulsar MultiEdit

# Stage 1: Builder
FROM rust:1.78-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/multiuser_server/Cargo.toml crates/multiuser_server/
COPY crates/multiuser_server/build.rs crates/multiuser_server/
COPY crates/multiuser_server/proto crates/multiuser_server/proto

# Create dummy main to cache dependencies
RUN mkdir -p crates/multiuser_server/src && \
    echo "fn main() {}" > crates/multiuser_server/src/main.rs && \
    cargo build --release -p pulsar-multiedit && \
    rm -rf crates/multiuser_server/src

# Copy actual source code
COPY crates/multiuser_server/src crates/multiuser_server/src

# Build the application
RUN touch crates/multiuser_server/src/main.rs && \
    cargo build --release --bin pulsar-multiedit

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
COPY --from=builder /app/target/release/pulsar-multiedit /usr/local/bin/pulsar-multiedit

# Set ownership
RUN chown pulsar:users /usr/local/bin/pulsar-multiedit

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
  CMD ["/usr/local/bin/pulsar-multiedit", "--version"] || exit 1

# Run the application
ENTRYPOINT ["/usr/local/bin/pulsar-multiedit"]
