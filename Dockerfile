# One image for the whole app: the `playground` binary (serve | seed | export)
# plus the built frontend, which `serve` hosts from STATIC_DIR.
#
#   docker build -t arrow-playground:local .
#
# Base images follow the pinned toolchain in docs/DESIGN.md. The builder and
# the runtime are both Debian trixie, so the binary links against the same
# glibc it was built with.

# ---------------------------------------------------------------------------
# Frontend: Vite build -> /web/dist
# ---------------------------------------------------------------------------
FROM node:26.12.1-trixie-slim AS web

WORKDIR /web

# Manifests first: the `npm ci` layer is reused until dependencies change.
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY web/ ./
RUN npm run build

# ---------------------------------------------------------------------------
# Server: release build of the `playground` binary
# ---------------------------------------------------------------------------
FROM rust:1.99.1-trixie AS server

WORKDIR /build

# Dependency layer. Compiling Arrow, Parquet, tokio and friends takes most of
# the build, so build them once against stub targets; Docker reuses this
# layer until Cargo.toml or Cargo.lock change. (A plain stub instead of
# cargo-chef: the package has one lib and one bin and no build script, so the
# stub is two files and needs no extra tool.) The package's own fingerprints
# are removed so the next build compiles the real sources even if their
# mtimes are older than the stub's.
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/.fingerprint/playground-*

COPY server/ ./
RUN cargo build --release --locked --bin playground

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
# No extra packages: the binary statically links its compression libraries and
# jemalloc, talks plain TCP or a unix socket to PostgreSQL, and TLS for
# clients is terminated in front of it (Cloud Run, or nothing on localhost).
FROM debian:trixie-slim

RUN groupadd --system --gid 10001 playground \
    && useradd --system --uid 10001 --gid playground --no-create-home \
        --home-dir /nonexistent --shell /usr/sbin/nologin playground

WORKDIR /app
COPY --from=server /build/target/release/playground /app/playground
COPY --from=web /web/dist /app/static

ENV STATIC_DIR=/app/static \
    PORT=8080 \
    RUST_LOG=info

# Numeric so platforms that enforce runAsNonRoot can verify it.
USER 10001:10001
EXPOSE 8080

ENTRYPOINT ["/app/playground"]
CMD ["serve"]
