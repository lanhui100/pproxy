# Agent Note: 桌面端与 CLI 新增一键生成 Clash Meta 客户端配置与二维码

Status: implemented

## Problem

用户在移动端及第三方客户端（如手机端 Clash Meta、Flclash、小火箭等）需要快捷接入本地代理节点。之前：
1. 桌面端设置页中仅有 API 反代与常规代理切换，缺少针对 Clash Meta 等客户端的一键导出配置与二维码展示功能，用户在桌面端无法直观扫码导入；
2. CLI 中虽有 `pproxy clash` 与 `pproxy config export clash` 命令，但在客户端维度缺少 `pproxy client clash` / `pproxy client export` 等与“客户端”语义直接对应的命令集合，降低了多端集成配置的发现度与易用性。

## Decision

1. **桌面端后端集成二维码生成与 Clash 配置导出**（`desktop/src-tauri/`）：
   - 在 `desktop/src-tauri/Cargo.toml` 中引入 `qrcode`（与 CLI 保持一致的 `qrcode 0.14`）；
   - 在 `lib.rs` 中新增 `proxy_clash_config_get` 与 `proxy_clash_qr_svg` / `proxy_clash_export` 命令，自动读取当前绑定的局域网 IP / 端口及有效 Token，合成 Clash Meta / Mihomo 标准 YAML 配置，并渲染出 SVG 格式二维码供前端渲染。
2. **桌面端设置页新增 Clash Meta 客户端集成模块**（`desktop/src/views/SettingsView.vue`）：
   - 在设置页中新增「客户端与移动端配置」卡片；
   - 提供「查看/下载 Clash Meta 配置」与「手机扫码导入（二维码弹窗）」；
   - 支持一键复制订阅链接及配置文件内容。
3. **CLI 二进制客户端命令扩展**（`crates/cli/src/main.rs` 及子模块）：
   - 新增 `pproxy client` 命令组（别名支持 `pproxy client clash`、`pproxy client export`、`pproxy client qr`），将第三方客户端配置导出与二维码扫码纳为一级或专属客户端命令。
   - 保持既有 `pproxy clash` 与 `pproxy config export` 的向后兼容性。

## Alternatives considered

- **仅在前端 JS 中引入第三方 qrcode 库动态渲染二维码**：增加前端 bundle 体积且各端渲染标准不一；Rust 侧已在 CLI 中成熟使用 `qrcode` crate，直接复用并在后端生成矢量 SVG 二维码更加轻量、一致且安全。
- **让用户手动修改 YAML 文件填写 IP 和 Token**：普通用户操作门槛极高，容易因格式缩进错误导致客户端解析失败。

## Consequences

- 桌面端与 CLI 二进制双端均支持 Clash Meta 标准 YAML 与二维码一键导入；
- 用户体验平滑连贯，跨设备配置门槛大幅降低。
