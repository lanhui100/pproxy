#!/usr/bin/env python3
"""yaml_contract.py — Clash Meta YAML 口径断言器（契约 C3 字段 / C6 分流）。

定位：把 .dev-team/contract.md 的 C3/C6 条款编译成可执行断言；不做任何业务实现。
调用方（bash 用例）以退出码 0/1 记账，stdout 单行 detail 直接作为 [PASS]/[FAIL] 的实测值。

子命令：
  fields --file F --token T   断言 C3 隧道节点口径：
                              proxies 含 type=http、port=8443、server 非空、
                              username/password 均等于请求令牌、
                              且不含 headers.Authorization: Bearer（死配置）
  rules  --file F             断言 C6 分流口径：
                              末尾规则 `- MATCH,PROXY`、`GEOIP,CN,DIRECT` 在其之前、
                              探活域与内网 CIDR 全部 DIRECT 且排在所有 PROXY 规则之前

豁免：桌面端生成器（desktop/src-tauri/src/lib.rs::build_clash_meta_yaml）不在
本断言器的运行时覆盖内（Tauri GUI crate 无法在无显示环境构建），改由
cases_desktop_static.sh 做源码级契约断言，理由见 tests/acceptance/README.md 豁免表。
"""

import argparse
import re
import sys

import yaml

PROBE_DOMAINS = [
    "connectivitycheck.gstatic.com",
    "clients3.google.com",
    "msftconnecttest.com",
    "captive.apple.com",
    "cp.cloudflare.com",
]
INTRANET_CIDRS = ["127.0.0.0/8", "192.168.0.0/16"]


def _load(path):
    with open(path, "r", encoding="utf-8") as fh:
        text = fh.read()
    return text, yaml.safe_load(text)


def _rule_parts(rule):
    return [p.strip() for p in str(rule).split(",")]


def _rule_action(rule):
    parts = _rule_parts(rule)
    # DOMAIN,x,DIRECT / IP-CIDR,x,DIRECT,no-resolve / GEOIP,CN,DIRECT
    if len(parts) >= 3:
        return parts[2].upper()
    return ""


def _first_proxy_index(rules):
    for idx, rule in enumerate(rules):
        if _rule_action(rule) == "PROXY":
            return idx
    return None


def cmd_fields(args):
    problems = []
    try:
        text, doc = _load(args.file)
    except FileNotFoundError:
        print(f"FAIL: file not found: {args.file}")
        return 1
    except yaml.YAMLError as exc:
        print(f"FAIL: yaml parse error: {str(exc).splitlines()[0]}")
        return 1
    except OSError as exc:
        print(f"FAIL: read error: {exc}")
        return 1

    if not isinstance(doc, dict):
        print(f"FAIL: root is {type(doc).__name__}, expected mapping")
        return 1

    proxies = doc.get("proxies")
    if not isinstance(proxies, list) or not proxies:
        print(f"FAIL: proxies missing/empty (type={type(proxies).__name__})")
        return 1

    http_proxies = [
        p for p in proxies if isinstance(p, dict) and str(p.get("type", "")).lower() == "http"
    ]
    if not http_proxies:
        print(f"FAIL: no proxy with type=http; observed types={[p.get('type') if isinstance(p, dict) else p for p in proxies]}")
        return 1

    tunnel = http_proxies[0]
    port = tunnel.get("port")
    if port != 8443:
        problems.append(f"port={port!r} (expect 8443)")
    server = tunnel.get("server")
    if not isinstance(server, str) or not server.strip():
        problems.append(f"server={server!r} (expect non-empty host)")
    if tunnel.get("tls") is not True:
        problems.append(f"tls={tunnel.get('tls')!r} (expect true)")
    if tunnel.get("username") != args.token:
        problems.append(f"username={tunnel.get('username')!r} (expect token)")
    if tunnel.get("password") != args.token:
        problems.append(f"password={tunnel.get('password')!r} (expect token)")

    headers = tunnel.get("headers")
    if isinstance(headers, dict):
        for key, val in headers.items():
            if str(key).lower() == "authorization" and "bearer" in str(val).lower():
                problems.append(f"headers.Authorization={val!r} (Bearer 死配置，CONNECT 不接受)")
    if re.search(r"Authorization\s*:\s*\"?\s*Bearer", text, re.IGNORECASE):
        problems.append("raw text still contains 'Authorization: \"Bearer'")

    summary = (
        f"proxy name={tunnel.get('name')!r} type=http server={server!r} port={port!r} "
        f"tls={tunnel.get('tls')!r} username={'<token>' if tunnel.get('username') == args.token else tunnel.get('username')!r} "
        f"password={'<token>' if tunnel.get('password') == args.token else tunnel.get('password')!r} bearer=absent"
    )
    if problems:
        print("FAIL: " + "; ".join(problems) + f" | observed: {summary}")
        return 1
    print("OK: " + summary)
    return 0


def cmd_rules(args):
    problems = []
    try:
        _text, doc = _load(args.file)
    except FileNotFoundError:
        print(f"FAIL: file not found: {args.file}")
        return 1
    except yaml.YAMLError as exc:
        print(f"FAIL: yaml parse error: {str(exc).splitlines()[0]}")
        return 1
    except OSError as exc:
        print(f"FAIL: read error: {exc}")
        return 1

    if not isinstance(doc, dict):
        print(f"FAIL: root is {type(doc).__name__}, expected mapping")
        return 1

    rules = doc.get("rules")
    if not isinstance(rules, list) or not rules:
        print(f"FAIL: rules missing/empty (type={type(rules).__name__})")
        return 1
    rules = [str(r).strip() for r in rules]

    tail = rules[-1].upper().replace(" ", "")
    if tail != "MATCH,PROXY":
        problems.append(f"last rule={rules[-1]!r} (expect 'MATCH,PROXY')")

    geo_idx = None
    for idx, rule in enumerate(rules):
        if rule.upper().replace(" ", "") == "GEOIP,CN,DIRECT":
            geo_idx = idx
            break
    if geo_idx is None:
        problems.append("missing 'GEOIP,CN,DIRECT'")
    elif geo_idx != len(rules) - 2:
        problems.append(f"'GEOIP,CN,DIRECT' at index {geo_idx}, expect immediately before final MATCH (index {len(rules) - 2})")

    first_proxy = _first_proxy_index(rules)
    if first_proxy is None:
        problems.append("no PROXY rule at all")
    else:
        for domain in PROBE_DOMAINS:
            idx = next(
                (
                    i
                    for i, rule in enumerate(rules)
                    if domain in rule and _rule_action(rule) == "DIRECT"
                ),
                None,
            )
            if idx is None:
                problems.append(f"probe domain {domain} not DIRECT")
            elif idx > first_proxy:
                problems.append(f"probe domain {domain} at {idx} after first PROXY rule at {first_proxy}")
        for cidr in INTRANET_CIDRS:
            idx = next(
                (
                    i
                    for i, rule in enumerate(rules)
                    if cidr in rule and _rule_action(rule) == "DIRECT"
                ),
                None,
            )
            if idx is None:
                problems.append(f"intranet CIDR {cidr} not DIRECT")
            elif idx > first_proxy:
                problems.append(f"intranet CIDR {cidr} at {idx} after first PROXY rule at {first_proxy}")

    summary = f"rules={len(rules)} last={rules[-1]!r} geoip_cn_idx={geo_idx} first_proxy_idx={first_proxy}"
    if problems:
        print("FAIL: " + "; ".join(problems) + f" | observed: {summary}")
        return 1
    print("OK: " + summary)
    return 0


def main():
    parser = argparse.ArgumentParser(description="Clash Meta YAML 契约断言器")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("fields")
    p.add_argument("--file", required=True)
    p.add_argument("--token", required=True)
    p.set_defaults(fn=cmd_fields)

    p = sub.add_parser("rules")
    p.add_argument("--file", required=True)
    p.set_defaults(fn=cmd_rules)

    args = parser.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
