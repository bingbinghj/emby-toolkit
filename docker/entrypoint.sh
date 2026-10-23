#!/bin/bash
set -e
CONFIG_DIR="${CONFIG_DIR:-/config}"
PUID="${PUID:-0}"
PGID="${PGID:-0}"
UMASK="${UMASK:-000}"
mkdir -p "$CONFIG_DIR" /embytoolkit
groupmod -o -g "$PGID" embytoolkit
usermod -o -u "$PUID" embytoolkit
chown embytoolkit:embytoolkit /embytoolkit "$CONFIG_DIR"
find "$CONFIG_DIR" -maxdepth 1 -mindepth 1 -exec chown embytoolkit:embytoolkit {} +
umask "$UMASK"
if [ -S /var/run/docker.sock ]; then
    chmod 666 /var/run/docker.sock
fi
if [ "$PUID" -eq 0 ]; then
    exec dumb-init /app/emby-toolkit "$@"
else
    exec dumb-init gosu embytoolkit:embytoolkit /app/emby-toolkit "$@"
fi
