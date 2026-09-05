#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname """")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "=== pproxy clean build cache ==="
echo "[1/2] Cleaning root target..."
(cd "$PROJECT_ROOT" && cargo clean)

echo "[2/2] Cleaning desktop/src-tauri target..."
(cd "$PROJECT_ROOT/desktop/src-tauri" && cargo clean)

echo "=== Done ==="
