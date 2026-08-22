//! T1 §8 单元测试清单（14 项）。临时目录用 tempfile。

use std::path::PathBuf;

use serde_json::Value;

use super::*;

fn temp_db(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join(format!("{tag}.db"));
    (dir, db)
}

/// 构造与真实 config.json 同构的旧格式 config（7 路由）。
fn legacy_config_json() -> String {
    r#"{
  "listen_host": "127.0.0.1",
  "listen_port": 8899,
  "static_upstreams": [],
  "countries": [],
  "worker_url": "https://edge.ponyjob.top",
  "worker_secret": "test-secret",
  "routes": {
    "anthropic": "api.anthropic.com",
    "openai": "api.openai.com",
    "opencode": "opencode.ai",
    "google": "www.google.com",
    "github": "github.com",
    "x": "api.twitter.com",
    "facebook": "www.facebook.com"
  },
  "upstreams": {
    "vercel": { "url": "https://vedge.example/api/proxy", "secret": "s" }
  },
  "route_upstreams": { "openai": "vercel", "opencode": "vercel" },
  "pool_refresh_sec": 300
}"#
    .to_string()
}

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).expect("write file");
}

// ---- §8.1 open 两次同一路径 ----

#[test]
fn open_twice_second_not_fresh_data_kept() {
    let (_dir, db) = temp_db("open2");
    let (store, created) = Store::open(&db).unwrap();
    assert!(created, "首启应为全新库");
    store
        .insert_token("t1", "hash-open-twice", None)
        .unwrap();
    drop(store);

    let (_store2, created2) = Store::open(&db).unwrap();
    assert!(!created2, "二次 open 不应为全新库");
    assert_eq!(_store2.list_tokens().unwrap().len(), 1, "数据应保留");
}

// ---- §8.2 token CRUD 往返 ----

#[test]
fn token_crud_roundtrip() {
    let (_dir, db) = temp_db("tokcrud");
    let store = Store::open(&db).unwrap().0;
    let id = store.insert_token("dev", "hash-abc", Some(12345)).unwrap();

    let by_hash = store.get_token_by_hash("hash-abc").unwrap().unwrap();
    let by_id = store.get_token_by_id(id).unwrap().unwrap();
    assert_eq!(by_hash.id, id);
    assert_eq!(by_id.name, "dev");
    assert_eq!(by_id.token_hash, "hash-abc");
    assert_eq!(by_id.expires_at, Some(12345));
    assert!(by_id.revoked_at.is_none());
    assert_eq!(store.list_tokens().unwrap().len(), 1);

    store.revoke_token(id).unwrap();
    // 撤销判断在 token.rs（README §3.1），store 层 by_hash 仍返回行
    let after = store.get_token_by_hash("hash-abc").unwrap().unwrap();
    assert!(after.revoked_at.is_some());
}

// ---- §8.3 token_hash UNIQUE ----

#[test]
fn token_hash_unique_conflict() {
    let (_dir, db) = temp_db("hashuniq");
    let store = Store::open(&db).unwrap().0;
    store.insert_token("a", "dup-hash", None).unwrap();
    let err = store.insert_token("b", "dup-hash", None).unwrap_err();
    assert!(matches!(err, StoreError::Sqlite(_)), "应为 Sqlite 约束冲突: {err:?}");
}

// ---- §8.4 route CRUD 与三态 update ----

#[test]
fn route_crud_and_update_three_state() {
    let (_dir, db) = temp_db("routecrud");
    let store = Store::open(&db).unwrap().0;
    store
        .insert_route(&NewRoute {
            name: "openai".into(),
            target_host: "api.openai.com".into(),
            override_upstream: None,
        })
        .unwrap();

    let r = store.get_route("openai").unwrap().unwrap();
    assert_eq!(r.target_host, "api.openai.com");
    assert!(r.upstream.is_none());
    assert!(r.enabled);
    assert!(store.get_route("nope").unwrap().is_none());
    assert_eq!(store.list_routes().unwrap().len(), 1);

    // None=不改该列（C-P0-2）
    store.update_route("openai", None, Some(false)).unwrap();
    let r = store.get_route("openai").unwrap().unwrap();
    assert!(!r.enabled);
    assert!(r.override_upstream.is_none());

    // Some(Some(v))=设置
    store.update_route("openai", Some(Some("vercel".into())), None).unwrap();
    let r = store.get_route("openai").unwrap().unwrap();
    assert_eq!(r.override_upstream.as_deref(), Some("vercel"));
    assert!(!r.enabled, "None 参数不应覆盖 enabled");

    // Some(None)=清除（置 NULL）
    store.update_route("openai", Some(None), Some(true)).unwrap();
    let r = store.get_route("openai").unwrap().unwrap();
    assert!(r.override_upstream.is_none());
    assert!(r.enabled);

    assert!(store.delete_route("openai").unwrap());
    assert!(!store.delete_route("openai").unwrap(), "二次删除应返回 false");
}

// ---- §8.5 upsert_usage 累加 ----

#[test]
fn upsert_usage_accumulates() {
    let (_dir, db) = temp_db("usageacc");
    let store = Store::open(&db).unwrap().0;
    let row = UsageRow {
        ts_hour: 3600,
        route: "openai".into(),
        token_id: 1,
        requests: 3,
        bytes_in: 100,
        bytes_out: 200,
    };
    store.upsert_usage(&[row.clone()]).unwrap();
    store
        .upsert_usage(&[UsageRow { requests: 2, bytes_in: 50, bytes_out: 60, ..row }])
        .unwrap();
    let got = store.query_usage(0, None, None).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].requests, 5);
    assert_eq!(got[0].bytes_in, 150);
    assert_eq!(got[0].bytes_out, 260);
}

// ---- §8.6 query_usage 过滤与闭区间 ----

#[test]
fn query_usage_filters_and_closed_interval() {
    let (_dir, db) = temp_db("usageq");
    let store = Store::open(&db).unwrap().0;
    let mk = |h: u64, route: &str, token_id: i64, requests: u64| UsageRow {
        ts_hour: h,
        route: route.into(),
        token_id,
        requests,
        bytes_in: 0,
        bytes_out: 0,
    };
    store
        .upsert_usage(&[
            mk(3600, "openai", 1, 1),   // 恰为 since_hour（闭区间含）
            mk(7200, "openai", 1, 2),
            mk(7200, "google", 1, 4),
            mk(7200, "openai", 2, 8),
            mk(10800, "openai", 1, 16), // 区间外
        ])
        .unwrap();

    // 闭区间下界：3600/7200/10800 三个小时的 openai token 1 全命中（3 行）
    let all = store.query_usage(3600, Some("openai"), Some(1)).unwrap();
    assert_eq!(all.len(), 3, "应含 since_hour 当小时且排除 token 2 与 google");
    assert_eq!(all[0].ts_hour, 3600);

    let by_route = store.query_usage(0, Some("google"), None).unwrap();
    assert_eq!(by_route.len(), 1);
    assert_eq!(by_route[0].requests, 4);

    let by_token = store.query_usage(0, None, Some(2)).unwrap();
    assert_eq!(by_token.len(), 1);
    assert_eq!(by_token[0].requests, 8);

    let none = store.query_usage(10800 + 3600, None, None).unwrap();
    assert!(none.is_empty(), "上界之后无数据");
}

// ---- §8.7 迁移：7 路由导入 + config 重写 + .bak ----

#[test]
fn migrate_imports_and_rewrites_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let original = legacy_config_json();
    write_file(&cfg_path, &original);
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap().0;

    let out = store.migrate_config_if_needed(&cfg_path).unwrap();
    assert_eq!(out, MigrationOutcome::Imported(7));

    let routes = store.list_routes().unwrap();
    assert_eq!(routes.len(), 7);
    let by_name = |n: &str| routes.iter().find(|r| r.name == n).unwrap().clone();
    // F8：route_upstreams 绑定写入 override_upstream（resolve 实际读取列）；
    // upstream 列留 NULL（自动选择快照仅展示用途，迁移无快照）。
    assert_eq!(by_name("openai").upstream, None, "upstream 列为快照列，迁移无快照");
    assert_eq!(by_name("openai").override_upstream.as_deref(), Some("vercel"));
    assert_eq!(by_name("opencode").override_upstream.as_deref(), Some("vercel"));
    assert!(by_name("anthropic").override_upstream.is_none(), "无映射 → NULL 自动选择");
    assert!(by_name("openai").enabled);

    // config 重写为删除 routes 键后的原对象 + db_path（T1 §6.1.7）
    let rewritten: Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let obj = rewritten.as_object().unwrap();
    assert!(!obj.contains_key("routes"), "routes 键应删除");
    assert_eq!(obj.get("worker_url").and_then(Value::as_str), Some("https://edge.ponyjob.top"));
    assert_eq!(obj.get("worker_secret").and_then(Value::as_str), Some("test-secret"));
    assert!(obj.get("upstreams").is_some(), "upstreams 应保留");
    assert!(obj.get("route_upstreams").is_some(), "route_upstreams 应保留");
    assert_eq!(obj.get("listen_port").and_then(Value::as_i64), Some(8899));
    assert_eq!(
        obj.get("db_path").and_then(Value::as_str),
        Some(db.display().to_string().as_str()),
        "应新增 db_path 记录实际 DB 路径"
    );
    // 其余键全部保留（仅 routes 被删 + db_path 新增）
    let orig_obj = serde_json::from_str::<Value>(&original).unwrap();
    let mut orig_keys: Vec<String> =
        orig_obj.as_object().unwrap().keys().map(|k| k.to_string()).collect();
    orig_keys.retain(|k| k != "routes");
    orig_keys.push("db_path".into());
    orig_keys.sort();
    let mut new_keys: Vec<String> = obj.keys().map(|k| k.to_string()).collect();
    new_keys.sort();
    assert_eq!(orig_keys, new_keys, "键集合应仅差 routes/db_path 两处");

    // .bak 存在且内容为原始
    let bak = std::fs::read_to_string(dir.path().join("config.json.bak")).unwrap();
    assert_eq!(bak, original);
}

// F4：旧 config 含非法路由（name/host 校验不过、target 非字符串）→ 跳过不阻断，
// 合法项照常入库。
#[test]
fn migrate_skips_invalid_routes_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_file(
        &cfg_path,
        r#"{
  "worker_url": "https://edge.ponyjob.top",
  "worker_secret": "s",
  "routes": {
    "good_route": "api.example.com",
    "pony_bad": "api.example.com",
    "BadName": "api.example.com",
    "bad_host": "http://evil.com/path",
    "not_string": 12345
  }
}"#,
    );
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap().0;

    let out = store.migrate_config_if_needed(&cfg_path).unwrap();
    assert_eq!(out, MigrationOutcome::Imported(1), "仅 good_route 入库");

    let routes = store.list_routes().unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].name, "good_route");
    assert_eq!(routes[0].target_host, "api.example.com");
}

// F8：route_upstreams 绑定经迁移写入 override_upstream 列，resolve 据此选中
// 绑定上游（echo→localstub 场景，T8 步骤 5.5 的单测级回归）。
#[test]
fn migrate_binding_lands_in_override_upstream_and_resolve_picks_it() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_file(
        &cfg_path,
        r#"{
  "worker_url": "https://edge.ponyjob.top",
  "worker_secret": "s",
  "routes": { "echo": "echo.example.com" },
  "route_upstreams": { "echo": "localstub" }
}"#,
    );
    let store = Store::open(&dir.path().join("state.db")).unwrap().0;
    assert_eq!(
        store.migrate_config_if_needed(&cfg_path).unwrap(),
        MigrationOutcome::Imported(1)
    );

    let row = &store.list_routes().unwrap()[0];
    assert_eq!(row.override_upstream.as_deref(), Some("localstub"));
    assert_eq!(row.upstream, None, "upstream 列为快照列，迁移无快照");

    // resolve 层验证：Named 绑定优先于 host 规则（echo.example.com 非 Vercel host）
    let rt = crate::route::RouteTable::new(
        std::sync::Arc::new(store),
        std::sync::Arc::new(std::collections::HashMap::new()),
    )
    .unwrap();
    let (_, up) = rt.resolve("echo", "/ping").unwrap();
    assert_eq!(up.as_str(), "localstub");
}

// ---- §8.8 迁移幂等 ----

#[test]
fn migrate_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    write_file(&cfg_path, &legacy_config_json());
    let store = Store::open(&dir.path().join("state.db")).unwrap().0;

    assert_eq!(store.migrate_config_if_needed(&cfg_path).unwrap(), MigrationOutcome::Imported(7));
    let after_first = std::fs::read_to_string(&cfg_path).unwrap();
    assert_eq!(
        store.migrate_config_if_needed(&cfg_path).unwrap(),
        MigrationOutcome::AlreadyMigrated
    );
    assert_eq!(store.list_routes().unwrap().len(), 7);
    assert_eq!(
        std::fs::read_to_string(&cfg_path).unwrap(),
        after_first,
        "AlreadyMigrated 态不得重写 config"
    );
}

// ---- §8.9 无 routes 键 ----

#[test]
fn migrate_no_routes_key_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let cfg = r#"{"listen_host":"127.0.0.1","listen_port":8899,"worker_url":"https://e"}"#;
    write_file(&cfg_path, cfg);
    let store = Store::open(&dir.path().join("state.db")).unwrap().0;

    assert_eq!(
        store.migrate_config_if_needed(&cfg_path).unwrap(),
        MigrationOutcome::NoLegacyRoutes
    );
    assert_eq!(std::fs::read_to_string(&cfg_path).unwrap(), cfg, "config 不动");
    assert!(store.list_routes().unwrap().is_empty());
    assert!(!dir.path().join("config.json.bak").exists(), "不应产生备份");
}

// ---- §8.10 迁移中断态 ----

#[test]
fn migrate_interrupted_state_config_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    let original = legacy_config_json();
    write_file(&cfg_path, &original);
    let store = Store::open(&dir.path().join("state.db")).unwrap().0;

    // 模拟中断态：routes 表已导入但 config 未重写
    store
        .insert_route(&NewRoute {
            name: "anthropic".into(),
            target_host: "api.anthropic.com".into(),
            override_upstream: None,
        })
        .unwrap();

    assert_eq!(
        store.migrate_config_if_needed(&cfg_path).unwrap(),
        MigrationOutcome::AlreadyMigrated
    );
    assert_eq!(
        std::fs::read_to_string(&cfg_path).unwrap(),
        original,
        "中断态下 config 原样不动（不删键、不备份）"
    );
    assert!(!dir.path().join("config.json.bak").exists(), "不备份");
}

// ---- §8.11 hour_floor 边界 ----

#[test]
fn hour_floor_boundaries() {
    assert_eq!(hour_floor(3600), 3600);
    assert_eq!(hour_floor(3599), 0);
    assert_eq!(hour_floor(0), 0);
    assert_eq!(hour_floor(7199), 3600);
}

// ---- §8.12 name 部分唯一索引 ----

#[test]
fn token_name_unique_partial_index() {
    let (_dir, db) = temp_db("nameuniq");
    let store = Store::open(&db).unwrap().0;
    let id1 = store.insert_token("dev", "hash-1", None).unwrap();

    // 同 name 二次插入（均未撤销）→ 唯一冲突
    let err = store.insert_token("dev", "hash-2", None).unwrap_err();
    assert!(matches!(err, StoreError::Sqlite(_)));

    // 撤销后同名可复用
    store.revoke_token(id1).unwrap();
    let id2 = store.insert_token("dev", "hash-3", None).unwrap();
    assert_ne!(id1, id2);

    // 此时再插入同 name（未撤销行已存在）→ 冲突
    let err = store.insert_token("dev", "hash-4", None).unwrap_err();
    assert!(matches!(err, StoreError::Sqlite(_)));
}

// ---- §8.13 revoke 三态 ----

#[test]
fn revoke_three_outcomes() {
    let (_dir, db) = temp_db("revoke3");
    let store = Store::open(&db).unwrap().0;
    let id = store.insert_token("t", "hash-r3", None).unwrap();
    assert_eq!(store.revoke_token(id).unwrap(), RevokeOutcome::Revoked);
    assert_eq!(store.revoke_token(id).unwrap(), RevokeOutcome::AlreadyRevoked);
    assert_eq!(store.revoke_token(99999).unwrap(), RevokeOutcome::NotFound);
}

// ---- §8.14 ping ----

#[test]
fn ping_ok() {
    let (_dir, db) = temp_db("ping");
    let store = Store::open(&db).unwrap().0;
    store.ping().unwrap();
}

// ---- §9 验收：DB 目录 0700 / 文件 600（S-P2-2）----

#[cfg(unix)]
#[test]
fn db_permissions_hardened() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("pony-state");
    let db = db_dir.join("state.db");
    let _store = Store::open(&db).unwrap().0;
    let dir_mode = std::fs::metadata(&db_dir).unwrap().permissions().mode() & 0o777;
    let file_mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "DB 目录应为 0700");
    assert_eq!(file_mode, 0o600, "DB 文件应为 0600");
}

// ---- 附加：config 不存在 → NoLegacyRoutes（T1 §6.1.1）----

#[test]
fn migrate_missing_config_no_legacy() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap().0;
    assert_eq!(
        store.migrate_config_if_needed(&dir.path().join("absent.json")).unwrap(),
        MigrationOutcome::NoLegacyRoutes
    );
}

// ==== M3 监控存储（spec §9.3：upsert/latest/mark_read/prune，tempfile 内存库）====

use monitor::{MarkReadOutcome, QuotaSnapshotRow};

fn quota_row(ts: u64, upstream: &str, metric: &str, used: i64, pct: f64) -> QuotaSnapshotRow {
    QuotaSnapshotRow {
        ts,
        upstream: upstream.into(),
        metric: metric.into(),
        used,
        quota: if pct < 0.0 { -1 } else { 100_000 },
        pct,
    }
}

// ---- M3.1 upsert：INSERT OR REPLACE 同主键覆盖不累加 ----

#[test]
fn upsert_quota_snapshot_replaces_same_pk() {
    let (_dir, db) = temp_db("qup");
    let store = Store::open(&db).unwrap().0;
    store
        .upsert_quota_snapshot(&quota_row(3600, "cf", "requests_daily", 10_000, 10.0))
        .unwrap();
    // 同 PK 二次写入：覆盖而非报错/累加
    store
        .upsert_quota_snapshot(&quota_row(3600, "cf", "requests_daily", 85_000, 85.0))
        .unwrap();
    let rows = store.latest_quota_snapshots().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].used, rows[0].pct), (85_000, 85.0));
}

// ---- M3.2 latest：每 (upstream,metric) 最新一条 ----

#[test]
fn latest_quota_snapshots_per_key() {
    let (_dir, db) = temp_db("qlatest");
    let store = Store::open(&db).unwrap().0;
    store.upsert_quota_snapshot(&quota_row(3600, "cf", "requests_daily", 10, 0.01)).unwrap();
    store.upsert_quota_snapshot(&quota_row(7200, "cf", "requests_daily", 20, 0.02)).unwrap();
    store.upsert_quota_snapshot(&quota_row(3600, "vercel", "bandwidth", 30, -1.0)).unwrap();
    store.upsert_quota_snapshot(&quota_row(7200, "vercel", "function_invocations", 40, -1.0)).unwrap();

    let rows = store.latest_quota_snapshots().unwrap();
    assert_eq!(rows.len(), 3, "cf 1 条 + vercel 2 键各 1 条");
    assert_eq!(rows[0].upstream, "cf", "按 upstream 排序输出稳定");
    assert_eq!(rows[0].ts, 7200, "取每键最新 ts");
    let vercel_bw = rows.iter().find(|r| r.metric == "bandwidth").unwrap();
    assert_eq!((vercel_bw.ts, vercel_bw.used), (3600, 30));
}

// ---- M3.3 insert/list：倒序、unread 过滤、limit ----

#[test]
fn alerts_insert_list_unread_and_limit() {
    let (_dir, db) = temp_db("alerts");
    let store = Store::open(&db).unwrap().0;
    for i in 0..5 {
        store.insert_alert(1000 + i as u64, "warning", &format!("alert-{i}")).unwrap();
    }
    let all = store.list_alerts(false, 500).unwrap();
    assert_eq!(all.len(), 5);
    assert_eq!(all[0].message, "alert-4", "倒序（最新在前）");

    let limited = store.list_alerts(false, 2).unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].message, "alert-4");

    // 标记两条已读后 unread 过滤
    store.mark_alert_read(all[0].id).unwrap();
    store.mark_alert_read(all[1].id).unwrap();
    let unread = store.list_alerts(true, 500).unwrap();
    assert_eq!(unread.len(), 3);
    assert!(unread.iter().all(|a| a.read_at.is_none()));
    assert_eq!(unread[0].message, "alert-2");
    // 已读行 read_at 非空
    let reread = store.list_alerts(false, 500).unwrap();
    let marked: Vec<_> = reread.iter().filter(|a| a.read_at.is_some()).collect();
    assert_eq!(marked.len(), 2);
}

// ---- M3.4 mark_alert_read 三态幂等（R4）----

#[test]
fn mark_alert_read_three_outcomes() {
    let (_dir, db) = temp_db("markread");
    let store = Store::open(&db).unwrap().0;
    let id = store.insert_alert(42, "critical", "boom").unwrap();
    assert_eq!(store.mark_alert_read(id).unwrap(), MarkReadOutcome::Marked);
    assert_eq!(store.mark_alert_read(id).unwrap(), MarkReadOutcome::AlreadyRead);
    assert_eq!(store.mark_alert_read(999_999).unwrap(), MarkReadOutcome::NotFound);
    // AlreadyRead 路径不得覆盖首次 read_at
    let row = &store.list_alerts(false, 500).unwrap()[0];
    assert!(row.read_at.is_some());
}

// ---- M3.5 prune：usage 30 天 / quota 90 天保留策略 ----

#[test]
fn prune_usage_and_quota_by_cutoff() {
    let (_dir, db) = temp_db("prune");
    let store = Store::open(&db).unwrap().0;

    // usage_hourly：ts_hour=0 与 3600 各一行（借 UsageRow 结构）
    store
        .upsert_usage(&[
            UsageRow { ts_hour: 0, route: "r".into(), token_id: 1, requests: 1, bytes_in: 0, bytes_out: 0 },
            UsageRow { ts_hour: 3600, route: "r".into(), token_id: 1, requests: 2, bytes_in: 0, bytes_out: 0 },
        ])
        .unwrap();
    assert_eq!(store.prune_usage_before(3600).unwrap(), 1, "仅删除 < cutoff 的行");
    let left_usage = store.query_usage(0, None, None).unwrap();
    assert_eq!(left_usage.len(), 1);
    assert_eq!(left_usage[0].ts_hour, 3600);
    assert_eq!(store.prune_usage_before(u64::MAX / 2).unwrap(), 1, "全删返回计数");

    // quota_snapshots：同口径
    store.upsert_quota_snapshot(&quota_row(1000, "cf", "requests_daily", 1, 0.001)).unwrap();
    store.upsert_quota_snapshot(&quota_row(2000, "cf", "requests_daily", 2, 0.002)).unwrap();
    assert_eq!(store.prune_quota_before(2000).unwrap(), 1);
    let left = store.latest_quota_snapshots().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].ts, 2000);
}
