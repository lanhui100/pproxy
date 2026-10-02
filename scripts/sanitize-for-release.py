#!/usr/bin/env python3
"""
scripts/sanitize-for-release.py
PProxy 开源发布前敏感信息脱敏与检查工具。

用途：
  在公开发布/开源打包时，将真实的内部 Tailscale IP、真实 VPS 出口 IP 以及内部域名
  替换为符合 RFC 2606 / RFC 5737 / RFC 6598 的标准文档示例地址。
  本地开发与运维保持真实信息不变，作为项目记忆。

用法：
  # 1. 检查是否存在敏感信息（只读扫描，有敏感信息返回非零退出码）
  python3 scripts/sanitize-for-release.py --check

  # 2. 原地替换指定目录或文件（供打包/发布流水线使用）
  python3 scripts/sanitize-for-release.py --in-place --targets docs .agents/notes README.md README.en.md

  # 3. 输出脱敏后的临时目录（不污染当前工作区）
  python3 scripts/sanitize-for-release.py --export-dir /tmp/pproxy-sanitized
"""

import argparse
import os
import re
import shutil
import sys

# 标准替换规则表（保持语法与语义对应）
# 格式: (正则表达式, 替换后的文本, 描述)
REPLACEMENTS = [
    # 1. 内部 Tailscale 节点 IP -> RFC 6598 共享测试地址
    (r"\b100\.95\.193\.103\b", "100.64.0.1", "devserver 主力节点 IP"),
    (r"\b100\.105\.241\.39\b", "100.64.0.2", "tencent 备灾节点 IP"),
    (r"\b100\.97\.143\.121\b", "100.64.0.3", "preprod 备灾节点 IP"),
    (r"\b100\.120\.38\.106\b", "100.64.0.10", "desktop 客户端 IP"),

    # 2. 真实 VPS 公网出口 IP -> RFC 5737 文档专用公网 IP (TEST-NET-2)
    (r"\b192\.210\.231\.8\b", "198.51.100.8", "RackNerd VPS 出口真实 IP"),

    # 3. 内部域名与出口 -> RFC 2606 保留示例域名
    (r"\brn\.ponygo\.fun\b", "vps.example.com", "RackNerd VPS 节点域名"),
    (r"\bgate\.ponygo\.fun\b", "cf-gate.example.com", "Cloudflare Worker Gate 域名"),
    (r"\bvedge\.ponygo\.fun\b", "vercel-gate.example.com", "Vercel Gate 域名"),
    (r"\bvgate\.ponygo\.fun\b", "vercel-gate.example.com", "Vercel 隧道网关域名"),
    (r"\bedge\.ponygo\.fun\b", "edge.example.com", "Cloudflare Edge 域名"),
    (r"\baccess\.ponygo\.fun\b", "dist.example.com", "桌面客户端发布更新域名"),
    (r"\bdl\.ponygo\.fun\b", "download.example.com", "产物下载私有域名"),
]

DEFAULT_TARGETS = ["docs", ".agents/notes", "README.md", "README.en.md"]


def find_text_files(targets):
    files = []
    for t in targets:
        if os.path.isfile(t):
            files.append(t)
        elif os.path.isdir(t):
            for root, _, filenames in os.walk(t):
                # 排除 git / target 目录
                if "/.git" in root or "/target" in root:
                    continue
                for f in filenames:
                    if f.endswith((".md", ".json", ".yaml", ".yml", ".txt", ".sh", ".toml")):
                        files.append(os.path.join(root, f))
    return files


def check_files(files):
    total_findings = 0
    findings_by_file = {}

    for file_path in files:
        # 跳过本脱敏脚本自身及已落地的本 ADR（避免自引用误报）
        if "sanitize-for-release.py" in file_path or "2026-10-01-open-source-sanitization-strategy.md" in file_path:
            continue
        try:
            with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
                lines = f.readlines()
        except Exception as e:
            print(f"Warning: Failed to read {file_path}: {e}", file=sys.stderr)
            continue

        file_findings = []
        for line_no, line in enumerate(lines, 1):
            for pattern, _, desc in REPLACEMENTS:
                matches = re.findall(pattern, line)
                if matches:
                    file_findings.append((line_no, desc, matches[0], line.strip()))
                    total_findings += len(matches)

        if file_findings:
            findings_by_file[file_path] = file_findings

    return total_findings, findings_by_file


def sanitize_text(content):
    modified = content
    for pattern, replacement, _ in REPLACEMENTS:
        modified = re.sub(pattern, replacement, modified)
    return modified


def apply_in_place(files):
    changed_count = 0
    for file_path in files:
        if "sanitize-for-release.py" in file_path or "2026-10-01-open-source-sanitization-strategy.md" in file_path:
            continue
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()
            sanitized = sanitize_text(content)
            if sanitized != content:
                with open(file_path, "w", encoding="utf-8") as f:
                    f.write(sanitized)
                print(f"  [✓] Sanitized: {file_path}")
                changed_count += 1
        except Exception as e:
            print(f"  [✗] Failed to process {file_path}: {e}", file=sys.stderr)
    return changed_count


def export_sanitized(targets, export_dir):
    if os.path.exists(export_dir):
        shutil.rmtree(export_dir)
    os.makedirs(export_dir, exist_ok=True)

    files = find_text_files(targets)
    print(f"Exporting sanitized copy to: {export_dir}")
    for file_path in files:
        if "sanitize-for-release.py" in file_path:
            continue
        dest_path = os.path.join(export_dir, file_path)
        os.makedirs(os.path.dirname(dest_path), exist_ok=True)
        try:
            with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
                content = f.read()
            sanitized = sanitize_text(content)
            with open(dest_path, "w", encoding="utf-8") as f:
                f.write(sanitized)
        except Exception as e:
            print(f"Failed to copy {file_path}: {e}", file=sys.stderr)
    print(f"Export complete. Total files processed: {len(files)}")


def main():
    parser = argparse.ArgumentParser(description="PProxy 开源发布前敏感信息脱敏与检查工具")
    parser.add_argument("--check", action="store_true", help="只读检查是否存在敏感特征（有则退出码 1）")
    parser.add_argument("--in-place", action="store_true", help="在当前工作区原地替换脱敏")
    parser.add_argument("--export-dir", type=str, help="导出脱敏后的文件副本到指定目录")
    parser.add_argument("--targets", nargs="*", default=DEFAULT_TARGETS, help="目标路径列表（默认: docs .agents/notes README.md README.en.md）")

    args = parser.parse_args()

    files = find_text_files(args.targets)

    if args.export_dir:
        export_sanitized(args.targets, args.export_dir)
        return 0

    if args.in_place:
        print(f"Applying sanitization in-place to {len(files)} files...")
        changed = apply_in_place(files)
        print(f"\nDone. {changed} files were sanitized.")
        return 0

    # 默认或显式 --check 模式
    total, findings = check_files(files)
    if total > 0:
        print(f"\n[FAIL] 发现 {total} 处未脱敏的内部生产特征（敏感 IP / 私有域名）：\n")
        for f_path, item_list in findings.items():
            print(f"• {f_path} ({len(item_list)} 处):")
            for line_no, desc, matched, line_sample in item_list[:5]:
                print(f"   L{line_no}: [{desc}] -> {matched}")
            if len(item_list) > 5:
                print(f"   ... 还有 {len(item_list) - 5} 处")
        print("\n提示：在发布前可运行以下命令进行脱敏处理：")
        print("  python3 scripts/sanitize-for-release.py --in-place")
        print("  或使用 --export-dir 导出纯净副本发布。")
        return 1
    else:
        print(f"[PASS] 扫描了 {len(files)} 个文件，未发现未脱敏的内部 IP 与私有域名。")
        return 0


if __name__ == "__main__":
    sys.exit(main())
