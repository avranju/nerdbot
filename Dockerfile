# ── Stage 1: Build ────────────────────────────────────────────────
FROM rust:1.96-slim-trixie AS builder

# Install SQLite dev headers (required by sqlx) and libheif dev (for HEIC conversion)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    libsqlite3-dev libheif-dev pkg-config && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy dependency manifests first (layer caching)
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src

# Build the binary (sqlx needs to find migrations at build time for offline mode)
RUN cargo build --release

# ── Stage 2: Runtime ──────────────────────────────────────────────
FROM debian:trixie-slim

# No SQLite dev dependency at runtime — runtime libsqlite3-0 is installed below
# libheif1 provides HEIC/HEIF decoding support at runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    libsqlite3-0 libheif1 ca-certificates openssh-client && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the release binary
COPY --from=builder /app/target/release/nerdbot /usr/local/bin/nerdbot

ENTRYPOINT ["nerdbot"]
CMD ["--config", "/config/config.toml"]
