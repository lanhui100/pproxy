#!/usr/bin/env python3
"""desktop_static.py — 桌面端 `build_clash_meta_yaml` 源码级契约断言（C3/C6 哨兵）。

为什么是源码级：桌面端 Tauri GUI crate 无法在本机无显示环境构建/运行，运行态验证
由 W3 自证；本脚本只做"契约漂移哨兵"——断言生成器源码内的 YAML 字面量满足 C3/C6，
避免 8443/Basic 凭据/MATCH 方向被悄悄改回去。

作用域纪律：只在 `fn build_clash_meta_yaml` 函数体内匹配，避免把无关 Bearer
（如管理面 HTTP 头、注释、测试断言）误判为死配置。

子命令：
  fields --file LIB_RS   断言 C3：port: 8443 / username / password / 无 Bearer 死配置 /
                         生成器返回 Result（fail-fast）
  rules  --file LIB_RS   断言 C6：规则顺序（MATCH,PROXY 尾部、GEOIP,CN,DIRECT 紧邻其前、
                         探活域与内网 CIDR 在首条 PROXY 规则之前）
"""

import argparse
import re
import sys

FN_NAME = "build_clash_meta_yaml"
PROBE_DOMAINS = [
    "connectivitycheck.gstatic.com",
    "clients3.google.com",
    "msftconnecttest.com",
    "captive.apple.com",
    "cp.cloudflare.com",
]
INTRANET_CIDRS = ["127.0.0.0/8", "192.168.0.0/16"]
RULE_RE = re.compile(r"^\s*-\s+([A-Za-z0-9][A-Za-z0-9_,.\-/]*)\s*$")


def extract_fn(src: str):
    lines = src.splitlines()
    start = next((i for i, line in enumerate(lines) if f"fn {FN_NAME}" in line), None)
    if start is None:
        return None
    end = next((i for i in range(start + 1, len(lines)) if lines[i].rstrip() == "}"), None)
    if end is None:
        return None
    return lines[start : end + 1]


def read(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def cmd_fields(args):
    try:
        src = read(args.file)
    except OSError as exc:
        print(f"FAIL: read error: {exc}")
        return 1
    body_lines = extract_fn(src)
    if not body_lines:
        print(f"FAIL: 未定位到 fn {FN_NAME}（生成器被改名/删除 → C3 无法锚定）")
        return 1
    body = "\n".join(body_lines)
    problems = []

    if not re.search(r"port:\s*8443", body):
        problems.append("缺 port: 8443")
    if not re.search(r'username:\s*(\\")?', body):
        problems.append("缺 username 字段")
    if not re.search(r'password:\s*(\\")?', body):
        problems.append("缺 password 字段")
    if re.search(r'Authorization:\s*\\?"?\s*Bearer', body):
        problems.append("仍有 headers.Authorization: Bearer 死配置")
    if not re.search(r"Result<\s*String\s*,\s*String\s*>", body):
        problems.append("生成器签名非 Result<String, String>（fail-fast 缺失）")

    summary = f"fn {FN_NAME}: lines={len(body_lines)} port:8443={('port: 8443' in body)}"
    if problems:
        print("FAIL: " + "; ".join(problems) + f" | {summary}")
        return 1
    print("OK: C3 源码级齐备 (" + summary + ")")
    return 0


def _rules_of(body_lines):
    """从源码字面量里抽规则行（`- RULE`），丢弃注释/转义噪声。"""
    in_rules = False
    rules = []
    for line in body_lines:
        stripped = line.strip()
        if re.match(r"^rules:\s*$", stripped):
            in_rules = True
            continue
        if not in_rules:
            continue
        if stripped.startswith('"#') or stripped.startswith("))") or stripped == "}":
            break
        match = RULE_RE.match(line)
        if match:
            rules.append(match.group(1))
    return rules


def _action(rule):
    parts = [p.strip() for p in rule.split(",")]
    return parts[2].upper() if len(parts) >= 3 else ""


def cmd_rules(args):
    try:
        src = read(args.file)
    except OSError as exc:
        print(f"FAIL: read error: {exc}")
        return 1
    body_lines = extract_fn(src)
    if not body_lines:
        print(f"FAIL: 未定位到 fn {FN_NAME}（生成器被改名/删除 → C6 无法锚定）")
        return 1
    rules = _rules_of(body_lines)
    if not rules:
        print("FAIL: 函数体内未抽到 rules 列表（字面量结构可能已变）")
        return 1

    problems = []
    if rules[-1].upper().replace(" ", "") != "MATCH,PROXY":
        problems.append(f"末条规则={rules[-1]!r}（应 MATCH,PROXY）")
    geo_idx = next((i for i, r in enumerate(rules) if r.upper().replace(" ", "") == "GEOIP,CN,DIRECT"), None)
    if geo_idx is None:
        problems.append("缺 GEOIP,CN,DIRECT")
    elif geo_idx != len(rules) - 2:
        problems.append(f"GEOIP,CN,DIRECT 位于 {geo_idx}，应紧邻末条 MATCH（{len(rules) - 2}）")

    first_proxy = next((i for i, r in enumerate(rules) if _action(r) == "PROXY"), None)
    if first_proxy is None:
        problems.append("无 PROXY 规则")
    else:
        for domain in PROBE_DOMAINS:
            idx = next((i for i, r in enumerate(rules) if domain in r and _action(r) == "DIRECT"), None)
            if idx is None:
                problems.append(f"探活域 {domain} 非 DIRECT")
            elif idx > first_proxy:
                problems.append(f"探活域 {domain}@{idx} 在首条 PROXY@{first_proxy} 之后")
        for cidr in INTRANET_CIDRS:
            idx = next((i for i, r in enumerate(rules) if cidr in r and _action(r) == "DIRECT"), None)
            if idx is None:
                problems.append(f"内网 CIDR {cidr} 非 DIRECT")
            elif idx > first_proxy:
                problems.append(f"内网 CIDR {cidr}@{idx} 在首条 PROXY@{first_proxy} 之后")

    summary = f"rules={len(rules)} last={rules[-1]!r} geoip_idx={geo_idx} first_proxy_idx={first_proxy}"
    if problems:
        print("FAIL: " + "; ".join(problems) + f" | {summary}")
        return 1
    print("OK: C6 源码级齐备 (" + summary + ")")
    return 0


def cmd_health(args):
    """桌面端组自检 URL 口径（作用域同 fields：仅 build_clash_meta_yaml 函数体）。"""
    try:
        src = read(args.file)
    except OSError as exc:
        print(f"FAIL: read error: {exc}")
        return 1
    body_lines = extract_fn(src)
    if not body_lines:
        print(f"FAIL: 未定位到 fn {FN_NAME}")
        return 1
    text = "\n".join(body_lines)
    expect = "https://www.gstatic.com/generate_204"
    problems = []
    if expect not in text:
        problems.append(f"缺健康检查 URL {expect}")
    if "http://cp.cloudflare.com" in text:
        problems.append("仍出现 http://cp.cloudflare.com（明文 80 健康检查）")
    if problems:
        print("FAIL: " + "; ".join(problems) + f" | fn {FN_NAME}: lines={len(body_lines)}")
        return 1
    print(f"OK: 桌面端 health url={expect} plaintext_cp_cloudflare=absent (fn {FN_NAME}: lines={len(body_lines)})")
    return 0


def main():
    parser = argparse.ArgumentParser(description="桌面端 Clash 生成器源码级契约断言")
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("fields")
    p.add_argument("--file", required=True)
    p.set_defaults(fn=cmd_fields)
    p = sub.add_parser("rules")
    p.add_argument("--file", required=True)
    p.set_defaults(fn=cmd_rules)
    p = sub.add_parser("health")
    p.add_argument("--file", required=True)
    p.set_defaults(fn=cmd_health)
    args = parser.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
