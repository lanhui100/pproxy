//! 本地代理引擎（M6）：白名单分流 + PAC 生成 + 隧道/直连 TCP 分流器。
//!
//! 模块布局：
//! - [`whitelist`]：域名后缀匹配（纯函数）
//! - [`pac`]：PAC 脚本生成（纯函数）
//! - [`engine`]：127.0.0.1:18900 监听 + CONNECT/absolute-form 分流循环
//! - [`engine_tunnel`]：WS gate 中继（方案 A / 同步口令导入的独立加速通道）
//! - [`engine_upstream`]：远端上游 HTTP 代理中继（方案 B chained 模式）
//!
//! 安全边界：仅绑回环；不解密任何 TLS；非白名单一律本机直连。

pub mod engine;
pub mod engine_tunnel;
pub mod engine_upstream;
pub mod pac;
pub mod sysproxy;
pub mod whitelist;
