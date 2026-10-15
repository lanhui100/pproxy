#!/usr/bin/env bash
# Watchdog for maintaining SSH tunnel to muse egress proxy on 127.0.0.1:10880
set -u

TUNNEL_PORT=10880
REMOTE_TARGET="198.19.0.1:3128"
HOST="muse"

while true; do
    if ! nc -z 127.0.0.1 "$TUNNEL_PORT" 2>/dev/null; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] Tunnel on :$TUNNEL_PORT is down. Reconnecting to $HOST..."
        # Kill stale ssh processes forwarding this port if any
        pkill -f "127.0.0.1:$TUNNEL_PORT:$REMOTE_TARGET" 2>/dev/null || true
        ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes -N -f -L "127.0.0.1:$TUNNEL_PORT:$REMOTE_TARGET" "$HOST"
    fi
    sleep 5
done
