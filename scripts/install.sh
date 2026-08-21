#!/bin/bash
set -e

echo "=== Building pproxy (release) ==="
cd /home/USER/pproxy
cargo build --release 2>&1 | tail -20

echo "=== Installing systemd service ==="
sudo cp systemd/pproxy.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable pproxy
sudo systemctl start pproxy

echo "=== Waiting for service ==="
sleep 3
sudo systemctl status pproxy --no-pager

echo "=== Testing proxy ==="
curl -x http://127.0.0.1:8899 -s --max-time 15 https://ipinfo.io/json | head -c 300
echo
echo "=== Done ==="
echo "Proxy: http://127.0.0.1:8899"
echo "Stats: http://127.0.0.1:8900/stats"
