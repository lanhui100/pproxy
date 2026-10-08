//! 红相冻结集成测试（legacy item 4：VERCEL_HOSTS 死回退）。
//!
//! 背景（Lead 已机器实证，勿重复调研）：crates/core/src/route.rs 的
//! `VERCEL_HOSTS` 常量（api.openai.com / opencode.ai）默认回退已下线的
//! Vercel 出口，导致无 override 时这两类 host 落到已下线上游。
//!
//! 契约（修复目标）：
//!   - 无 override 时默认上游一律回退 Worker（`Upstream::Worker`）；
//!   - 显式 override（如 "vercel"）仍优先于 host 规则（`Upstream::Vercel`）。
//!
//! 验收方式：本文件在修复前必须 FAIL（红相），修复后必须 PASS（绿相）。
//! 判定规则对应 T4 §8.1：override 优先，否则按 host 规则——此处 host 规则
//! 默认值即为 Worker。

use std::collections::HashMap;
use std::sync::Arc;

use pproxy_core::route::Upstream;
use pproxy_core::{RouteTable, Store};

/// 构造空路由表（内存热表 + 空上游客户端表），仅用于纯决策函数 pick_upstream。
fn empty_table(tag: &str) -> (tempfile::TempDir, RouteTable) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join(format!("{tag}.db"));
    let store = Store::open(&db).expect("store open").0;
    let rt = RouteTable::new(Arc::new(store), Arc::new(HashMap::new())).expect("route table init");
    (dir, rt)
}

/// 红相：`api.openai.com` / `opencode.ai` 无 override 时不得再回退 Vercel。
#[test]
fn default_upstream_falls_back_to_worker() {
    let (_dir, rt) = empty_table("default_upstream");
    assert_eq!(rt.pick_upstream("api.openai.com", None), Upstream::Worker);
    assert_eq!(rt.pick_upstream("opencode.ai", None), Upstream::Worker);
}

/// 绿相（修复后仍须保持）：显式 override 优先于 host 规则。
#[test]
fn explicit_override_still_wins() {
    let (_dir, rt) = empty_table("override");
    assert_eq!(
        rt.pick_upstream("api.openai.com", Some("vercel")),
        Upstream::Vercel
    );
    // 大小写不敏感 + 其他 host 默认 Worker 一并锁定，防回归
    assert_eq!(rt.pick_upstream("API.OPENAI.COM", None), Upstream::Worker);
    assert_eq!(rt.pick_upstream("api.anthropic.com", None), Upstream::Worker);
}
