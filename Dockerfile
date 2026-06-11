# ── Stage 1: Build ────────────────────────────────────────────────
FROM rust:1.96-slim-bookworm AS builder

# Install SQLite dev headers (required by sqlx)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    libsqlite3-dev pkg-config && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy dependency manifests first (layer caching)
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src

# Build the binary (sqlx needs to find migrations at build time for offline mode)
RUN cargo build --release

# ── Stage 2: Runtime ──────────────────────────────────────────────
FROM debian:bookworm-slim

# No SQLite dev dependency at runtime — runtime libsqlite3-0 is installed below
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    libsqlite3-0 ca-certificates openssh-client && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the release binary
COPY --from=builder /app/target/release/nerdbot /usr/local/bin/nerdbot

ENTRYPOINT ["nerdbot"]
CMD ["--config", "/config/config.toml"]
