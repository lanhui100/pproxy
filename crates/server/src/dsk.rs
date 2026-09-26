//! 桌面端更新分发（M5 拓展，用户裁决 2026-08-22）：
//! GET /dsk/:filename —— 从 PPROXY_DESKTOP_DIST_DIR（默认 /opt/pony-desktop-releases）
//! 提供最新 release 的 latest.json / 安装包 / .sig 签名三件套，
//! 供 Tauri updater 在 tailnet 内自更新（避免直连 GitHub 的国内网络问题）。
//!
//! 安全口径（ADR-007 边界内）：
//! - 该路由**豁免 admin Bearer**（updater 插件不携带凭据，嵌入 admin token 到
//!   分发二进制 = 凭据泄漏）。边界=tailnet 设备集；分发的文件本身即面向这些
//!   设备的非机密产物（安装包/版本清单/签名），latest.json 含 minisign 公钥
//!   口径的签名串，防篡改由客户端验签保证。
//! - 文件名白名单 ^[A-Za-z0-9._-]{1,100}$：拒绝路径穿越/任意读。

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

const DEFAULT_DIST_DIR: &str = "/opt/pony-desktop-releases";
const MAX_NAME_LEN: usize = 100;

fn dist_dir() -> PathBuf {
    std::env::var("PPROXY_DESKTOP_DIST_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DIST_DIR))
}

/// 文件名白名单：单段、无分隔符、无 ".."、长度受限——结构性杜绝目录穿越。
pub(crate) fn sanitize_filename(raw: &str) -> Option<String> {
    let n = raw.len();
    if n == 0 || n > MAX_NAME_LEN || !raw.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        return None;
    }
    // 双点防御：白名单虽允许单字符 '.'，但连续 '..' 一律拒绝
    if raw.contains("..") || raw.starts_with('.') {
        return None;
    }
    Some(raw.to_string())
}

fn content_type_of(name: &str) -> &'static str {
    if name.ends_with(".json") {
        "application/json"
    } else if name.ends_with(".sig") {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

/// GET /dsk/:filename
pub(crate) async fn dsk_file_handler(
    State(st): State<crate::api::AdminState>,
    Path(name): Path<String>,
) -> Response {
    let _ = &st; // 复用 AdminState（鉴权中间件在此路由上被豁免，见 api.rs）
    let Some(name) = sanitize_filename(&name) else {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response();
    };
    let path = dist_dir().join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                content_type_of(&name),
            )],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// 数据面公开分发（HTTPS 经 CF Tunnel；文件非机密+客户端验签，spec m6 §12）。
pub(crate) async fn dsk_file_public(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let Some(name) = sanitize_filename(&name) else {
        return (StatusCode::BAD_REQUEST, "invalid filename").into_response()
    };
    let dir = dist_dir();
    let file_path = dir.join(&name);
    // 回退尝试：如果指定目录找不到，且在 $HOME/pony-desktop-releases，尝试读取
    let read_res = match tokio::fs::read(&file_path).await {
        Ok(bytes) => Ok(bytes),
        Err(e) => {
            if let Ok(home) = std::env::var("HOME") {
                let home_fallback = std::path::PathBuf::from(home).join("pony-desktop-releases").join(&name);
                if home_fallback != file_path {
                    tokio::fs::read(&home_fallback).await
                } else {
                    Err(e)
                }
            } else {
                Err(e)
            }
        }
    };
    match read_res {
        Ok(bytes) => {
            let ct = if name.ends_with(".json") { "application/json" }
                     else if name.ends_with(".sig") { "text/plain" }
                     else { "application/octet-stream" };
            (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, ct)], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_filename;

    #[test]
    fn whitelist_accepts_release_artifacts() {
        for name in [
            "latest.json",
            "pony-desktop_0.2.0_x64-setup.exe",
            "pony-desktop_0.2.0_x64-setup.exe.sig",
        ] {
            assert_eq!(sanitize_filename(name), Some(name.to_string()), "{name}");
        }
    }

    #[test]
    fn traversal_and_hostile_names_rejected() {
        for name in [
            "../state.db",
            "..",
            ".pproxy.env",
            ".",
            "a/b",
            "a\\b",
            "",
            "has space.exe",
            "中文.json",
        ] {
            assert!(sanitize_filename(name).is_none(), "{name} 应被拒绝");
        }
    }
}
