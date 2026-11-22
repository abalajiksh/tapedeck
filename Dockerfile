# syntax=docker/dockerfile:1
# check=skip=SecretsUsedInArgOrEnv
#
# (Both lines above are parser directives and must stay first. The skipped
# check flags TAPEDECK_SECRET_KEY_PATH by its name; it is a path, not a secret.)
#
# Tapedeck as a container: one static musl binary on Alpine.
#
#   docker build -t tapedeck .
#   docker run -p 8080:8080 -v tapedeck-data:/data tapedeck
#
# The Postgres build is the same file with one argument (compose.postgres.yaml
# runs it beside a database):
#
#   docker build --build-arg CARGO_FEATURES=postgres -t tapedeck:postgres .
#
# Published to ghcr.io/abalajiksh/tapedeck by .github/workflows/docker.yml
# when a v* tag reaches the GitHub mirror. The hosts and the OBS packages are
# built elsewhere (glibc, Jenkins) and are not affected by anything here.
#
# Why musl: the dependency tree has no OpenSSL, bundles SQLite's C, and the
# rest is Rust, so a static binary costs nothing — and the runtime image then
# carries almost no packages, which is what keeps a vulnerability scan of it
# short. musl's slow allocator is answered by mimalloc (Cargo.toml, main.rs).
#
# What the runtime image deliberately lacks:
#   * ca-certificates — the roots are compiled in (webpki-roots).
#   * tzdata — time zones are compiled in (chrono-tz).
#   * ffmpeg — optional (report_video.rs); without it a mixdown plays in the
#     browser and offers no video export. To have the export, build on top:
#       FROM ghcr.io/abalajiksh/tapedeck
#       USER root
#       RUN apk add --no-cache ffmpeg
#       USER tapedeck

# Pinned to the CI agents' toolchain (td-pipelines: rust:1.98.1-bookworm,
# BUN_VERSION 1.4.2). The bun version is part of the UI fingerprint, so a
# different one builds a different bundle.
ARG RUST_VERSION=1.98.1
ARG BUN_VERSION=1.4.2
ARG ALPINE_VERSION=3.24

FROM oven/bun:${BUN_VERSION}-alpine AS bun

# ── build ────────────────────────────────────────────────────────────────────
FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS build

# build-base: a C compiler for ring, the bundled SQLite and mimalloc.
# libstdc++/libgcc: bun's musl build links against them.
RUN apk add --no-cache build-base libstdc++ libgcc
COPY --from=bun /usr/local/bin/bun /usr/local/bin/bun

# The bundle's version stamp (svelte.config.js); the workflow passes the commit.
ARG TAPEDECK_UI_VERSION_NAME=docker
ENV TAPEDECK_UI_VERSION_NAME=${TAPEDECK_UI_VERSION_NAME}

WORKDIR /src
COPY . .

# build.rs builds the UI with bun before rustc runs, so this is the whole build.
# The cache mounts only help a local rebuild: every release changes the source,
# so in CI this step always runs in full. target/ is a cache mount and vanishes
# with the step, hence the copy out inside the same RUN.
#
# CARGO_FEATURES picks the backend: empty is SQLite, `postgres` is Postgres. The
# feature switches one type alias and nothing downstream (design note 0005), so
# both builds share one target cache and the second costs one crate. They also
# share target/release/tapedeck, which is why that mount is locked: two builds
# at once would otherwise copy out whichever binary finished last.
ARG TARGETARCH
ARG CARGO_FEATURES=
RUN --mount=type=cache,id=tapedeck-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=tapedeck-target-${TARGETARCH},target=/src/target,sharing=locked \
    --mount=type=cache,id=tapedeck-node-modules-${TARGETARCH},target=/src/web/node_modules \
    cargo build --release --locked ${CARGO_FEATURES:+--features "$CARGO_FEATURES"} \
 && cp target/release/tapedeck /tapedeck \
 && strip /tapedeck

# ── runtime ──────────────────────────────────────────────────────────────────
FROM alpine:${ALPINE_VERSION}

LABEL org.opencontainers.image.title="Tapedeck" \
      org.opencontainers.image.description="Self-hosted scrobbler and music journal" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.url="https://codeberg.org/abksh/tapedeck" \
      org.opencontainers.image.documentation="https://codeberg.org/abksh/tapedeck#docker"

# The base tag is a snapshot; upgrading here gives every release the security
# fixes Alpine shipped since (a scan of the first build found libssl3,
# libcrypto3 and zlib fixes the 3.24 tag did not have yet).
RUN apk upgrade --no-cache \
 && addgroup -S -g 10001 tapedeck \
 && adduser -S -D -H -u 10001 -G tapedeck -h /data tapedeck \
 && mkdir -p /data \
 && chown tapedeck:tapedeck /data

COPY --from=build /tapedeck /usr/local/bin/tapedeck

# Every file Tapedeck writes lands on the volume, and the defaults live here
# rather than in a compose file people edit. The secret key above all: if it
# landed on the container's own layer, the next container would mint a new one
# and every stored credential (Plex tokens, Navidrome passwords, Last.fm
# sessions) would become undecryptable. The working directory is /data too, so
# anything relative — a .env, tapedeck.toml — is read from the volume.
#
# The Postgres build keeps these defaults and needs DATABASE_URL set to a
# postgres:// URL; handed the SQLite one below it refuses to start and says
# which schemes it takes. It ignores SQLITE_DB_PATH and EVERYNOISE_DB_PATH
# (those stores move into the one database), but the key, uploads and logs
# still land on /data, so it needs the volume all the same.
ENV HOST=0.0.0.0 \
    PORT=8080 \
    DATABASE_URL="sqlite:/data/scrobbles.db?mode=rwc" \
    SQLITE_DB_PATH=/data/tapedeck.db \
    EVERYNOISE_DB_PATH="sqlite:/data/everynoise.db?mode=rwc" \
    TAPEDECK_UPLOADS_DIR=/data/uploads \
    TAPEDECK_SECRET_KEY_PATH=/data/tapedeck.key \
    TAPEDECK_CONFIG=/data/tapedeck.toml \
    LOG_DIR=/data/logs \
    ENABLE_FILE_LOGGING=false

WORKDIR /data
VOLUME /data
USER tapedeck
EXPOSE 8080

# busybox wget is in the base image; nothing is added for this.
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s --retries=3 \
    CMD wget -q -O /dev/null "http://127.0.0.1:${PORT}/health" || exit 1

ENTRYPOINT ["/usr/local/bin/tapedeck"]
