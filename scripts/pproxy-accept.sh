#!/usr/bin/env bash
set -euo pipefail
TK=$(grep -oP '(?<=^admin_token = ")[^"]+' "$HOME/.pony/config.toml")
for h in api.openai.com api.x.ai api.anthropic.com api.b.ai; do
  code=$(curl -s -o /dev/null -m 20 --proxy http://127.0.0.1:8899 -w '%{http_code}' "https://$h/v1/models")
  [ "$code" = "401" ] || { echo "FAIL $h -> $code"; exit 1; }
done
pproxy status >/dev/null 2>&1 || { echo "FAIL pproxy status"; exit 1; }
if curl -s -m 10 --noproxy '*' -H "Authorization: Bearer $TK" http://100.95.193.103:8900/api/routes | grep -q vercel; then
  echo "FAIL vercel residue"; exit 1
fi
echo "ACCEPTANCE PASS"
