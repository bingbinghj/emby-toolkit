# syntax=docker/dockerfile:1.7
ARG BUILDPLATFORM=linux/amd64
FROM --platform=$BUILDPLATFORM node:20-alpine AS frontend-build
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN --mount=type=cache,id=emby-toolkit-rust-npm,target=/root/.npm npm ci --prefer-offline --no-audit --no-fund --legacy-peer-deps
COPY frontend/ ./
RUN npm run build

FROM rust:1.99.1-bookworm AS backend-build
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=1
WORKDIR /app
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
COPY backend/src/ ./backend/src/
COPY backend/assets/ ./backend/assets/
COPY fonts/ ./fonts/
RUN --mount=type=cache,id=emby-toolkit-rust-cargo,target=/usr/local/cargo/registry \
    --mount=type=cache,id=emby-toolkit-rust-target,target=/app/backend/target \
    cargo build --locked --release --manifest-path backend/Cargo.toml && \
    cp /app/backend/target/release/emby-toolkit /app/emby-toolkit

FROM debian:bookworm-slim
ENV LANG=C.UTF-8 TZ=Asia/Shanghai CONFIG_DIR=/config APP_DATA_DIR=/config \
    ETK_STATIC_DIR=/app/static ETK_BIND=0.0.0.0:5257 PUID=0 PGID=0 UMASK=000
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tzdata curl bash gosu dumb-init ffmpeg procps bzip2 xz-utils && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd -r -g 918 embytoolkit && \
    useradd -r -g embytoolkit -d /embytoolkit -s /bin/bash -u 918 embytoolkit && \
    mkdir -p /embytoolkit /config && chown embytoolkit:embytoolkit /embytoolkit /config
COPY --from=backend-build /app/emby-toolkit /app/emby-toolkit
COPY --from=frontend-build /app/frontend/dist/ /app/static/
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=5 \
    CMD curl -f http://localhost:5257/api/health || exit 1
VOLUME ["/config"]
EXPOSE 5257
ENTRYPOINT ["/entrypoint.sh"]
