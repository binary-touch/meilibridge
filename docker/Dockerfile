# Multi-stage build for optimal image size
# Build stage
FROM rust:1.82-bookworm AS builder

# Add labels for metadata
ARG BUILD_DATE
ARG VERSION
ARG VCS_REF
LABEL org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.title="MeiliBridge" \
      org.opencontainers.image.description="Real-time PostgreSQL to Meilisearch synchronization" \
      org.opencontainers.image.authors="MeiliBridge Contributors" \
      org.opencontainers.image.vendor="MeiliBridge" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.source="https://github.com/binary-touch/meilibridge" \
      org.opencontainers.image.documentation="https://github.com/binary-touch/meilibridge/blob/main/README.md" \
      org.opencontainers.image.licenses="MIT"

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /usr/src/meilibridge

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Build dependencies - this is cached as long as manifests don't change
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src target/release/build target/release/deps/*.d

# Copy source code
COPY src ./src

# Build the application (using cached dependencies)
RUN cargo build --release

# Runtime stage
# Using Ubuntu 24.04 (Noble) which has better security updates
FROM ubuntu:24.04

# Prevent interactive prompts during package installation
ENV DEBIAN_FRONTEND=noninteractive

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    libpq5 \
    gettext-base \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -r meilibridge && \
    useradd -r -g meilibridge -d /home/meilibridge -s /sbin/nologin -c "MeiliBridge user" meilibridge && \
    mkdir -p /home/meilibridge && \
    chown -R meilibridge:meilibridge /home/meilibridge

# Note: Ubuntu 24.04 includes security patches for known vulnerabilities.
# The zlib CVE-2023-45853 affects MiniZip functionality, not core zlib.

# Create necessary directories
RUN mkdir -p /etc/meilibridge /var/lib/meilibridge /var/log/meilibridge && \
    chown -R meilibridge:meilibridge /etc/meilibridge /var/lib/meilibridge /var/log/meilibridge

# Copy binary from builder
COPY --from=builder /usr/src/meilibridge/target/release/meilibridge /usr/local/bin/meilibridge

# Copy entrypoint script
COPY ./docker/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Copy default configuration (optional)
# COPY config.example.yaml /etc/meilibridge/config.yaml

# Set user
USER meilibridge

# Default API port
ENV MEILIBRIDGE_API_PORT=7701

# Expose API port (documentation only; binding handled by runtime)
EXPOSE ${MEILIBRIDGE_API_PORT}

# Set working directory
WORKDIR /var/lib/meilibridge

# Health check - using wget which is smaller than curl
# Or you could implement a health check command in meilibridge itself
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD /usr/local/bin/meilibridge health --port ${MEILIBRIDGE_API_PORT:-7701} || exit 1

# Use the entrypoint script
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD []
