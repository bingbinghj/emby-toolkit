# syntax=docker/dockerfile:1.7
ARG BUILDPLATFORM=linux/amd64
FROM --platform=$BUILDPLATFORM node:20-alpine AS frontend-build
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN --mount=type=cache,id=emby-toolkit-rust-npm,target=/root/.npm npm ci --prefer-offline --no-audit --no-fund --legacy-peer-deps
COPY frontend/ ./
RUN npm run build

FROM rust:1.99.1-bookworm AS backend-deps
ARG CARGO_BUILD_JOBS=2
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
WORKDIR /app
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
# 普通镜像层可导出到 Actions 缓存；cache mount 的 target 不会随 GHA 层缓存保存。
# 当前包只有默认 lib/main 两个目标，依赖仅随清单或工具链变化重新编译。
RUN mkdir -p backend/src && \
    printf 'fn main() {}\n' > backend/src/main.rs && \
    touch backend/src/lib.rs && \
    cargo build --locked --release --manifest-path backend/Cargo.toml && \
    cargo clean --release --package emby-toolkit --manifest-path backend/Cargo.toml

FROM backend-deps AS backend-build
COPY backend/src/ ./backend/src/
COPY backend/assets/ ./backend/assets/
COPY fonts/ ./fonts/
RUN cargo build --locked --release --manifest-path backend/Cargo.toml && \
    cp /app/backend/target/release/emby-toolkit /app/emby-toolkit

FROM debian:bookworm-slim AS runtime-deps
ENV LANG=C.UTF-8 TZ=Asia/Shanghai CONFIG_DIR=/config APP_DATA_DIR=/config \
    ETK_STATIC_DIR=/app/static ETK_BIND=0.0.0.0:5257 PUID=0 PGID=0 UMASK=000
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tzdata curl bash gosu dumb-init ffmpeg procps bzip2 xz-utils python3 python3-venv libchromaprint-tools nodejs && \
    ffmpeg -hide_banner -muxers 2>/dev/null | grep -q 'chromaprint' && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd -r -g 918 embytoolkit && \
    useradd -r -g embytoolkit -d /embytoolkit -s /bin/bash -u 918 embytoolkit && \
    mkdir -p /embytoolkit /config && chown embytoolkit:embytoolkit /embytoolkit /config
COPY backend/music_engine/requirements.txt /app/music_engine/requirements.txt
RUN mkdir -p /app/.build-tmp && \
    python3 -m venv /app/music-venv && \
    TMPDIR=/app/.build-tmp /app/music-venv/bin/pip install --no-cache-dir -r /app/music_engine/requirements.txt && \
    rm -rf /app/.build-tmp
ENV ETK_MUSIC_ENGINE=/app/music_engine/worker.py ETK_MUSIC_PYTHON=/app/music-venv/bin/python

FROM runtime-deps AS runtime
COPY backend/music_engine/etk_music/ /app/music_engine/etk_music/
COPY backend/music_engine/worker.py backend/music_engine/LICENSE.MoviePilot /app/music_engine/
COPY --from=backend-build /app/emby-toolkit /app/emby-toolkit
COPY --from=frontend-build /app/frontend/dist/ /app/static/
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=5 \
    CMD curl -f http://localhost:5257/api/health || exit 1
VOLUME ["/config"]
EXPOSE 5257
ENTRYPOINT ["/entrypoint.sh"]
