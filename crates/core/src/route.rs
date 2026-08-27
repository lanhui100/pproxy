//! 路由引擎（T4）：SQLite 路由表 + 内存热生效 + SSRF 校验。
//!
//! 一致性模式同 T2：单一写路径（create/update/delete 仅发生在本模块方法内），
//! 写库成功后同步全量重载内存表；`resolve` / `effective_upstream` 纯内存读
//! （RwLock read，数据面热路径个人网关 QPS 极低，无锁竞争问题）。
//! 热生效 = 写方法返回即生效，无轮询、无文件监听。

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use crate::edge::{EdgeClient, ForwardRequest};
use crate::store::{NewRoute, RouteRow, Store, StoreError};

/// test_route 总超时（S-P2-6）：管理面实测不得被数据面 120s 长超时拖累。
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 上游自动选择规则（§3.1）：命中则 Vercel，其余 Worker。
const VERCEL_HOSTS: &[&str] = &["api.openai.com", "opencode.ai"];

#[derive(Debug)]
pub enum RouteError {
    UnknownRoute,
    Disabled,
    InvalidName,     // 创建时：^[a-z][a-z0-9_-]{0,63}$，且禁止 pony_ 前缀（§6.0）
    InvalidHost,     // 创建时：SSRF 校验失败（§6.1）
    InvalidUpstream, // override_upstream 非 "worker"|"vercel"（S-P2-7）
    Duplicate,       // name 已存在
    Store(StoreError),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRoute => write!(f, "unknown route"),
            Self::Disabled => write!(f, "route disabled"),
            Self::InvalidName => write!(f, "invalid route name"),
            Self::InvalidHost => write!(f, "invalid target host"),
            Self::InvalidUpstream => write!(f, "invalid upstream (must be worker|vercel)"),
            Self::Duplicate => write!(f, "route name already exists"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for RouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<StoreError> for RouteError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// 上游标识（F8 扩展）：Worker/Vercel 为自动选择结果；Named 承载迁移导入的
/// route_upstreams 绑定（任意已配置上游名，如 localstub）。序列化为名字字符串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    Worker,
    Vercel,
    Named(String),
}

impl Upstream {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Worker => "worker",
            Self::Vercel => "vercel",
            Self::Named(s) => s,
        }
    }

    /// 已知名归一为枚举值（保持既有相等语义），其余进 Named。
    fn from_name(s: &str) -> Self {
        match s {
            "worker" => Self::Worker,
            "vercel" => Self::Vercel,
            other => Self::Named(other.to_string()),
        }
    }
}

impl serde::Serialize for Upstream {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// 精确小写匹配（S-P2-7：override 仅接受 "worker"|"vercel"；F8 扩展：
/// 迁移导入的 route_upstreams 绑定名同样合法——与 T1 §6.1.6 迁移语义一致）。
fn parse_upstream(s: &str) -> Option<Upstream> {
    Some(Upstream::from_name(s))
}

/// F8：create/update 的 override 值域校验——"worker"|"vercel" 恒合法
/// （worker 由 config.worker_url 提供，不经 upstreams map），其余须为
/// 已配置上游名（edges 表键）。空串拒绝（语义=未设置，应传 null 清除）。
fn valid_override(edges: &HashMap<String, EdgeClient>, s: &str) -> bool {
    match s {
        "worker" | "vercel" => true,
        "" => false,
        other => edges.contains_key(other),
    }
}

/// §6.0 name 校验（先于一切 host 校验）。pub 供迁移导入复用（F4：旧
/// config.json routes 键入库前必须过同一套校验，非法项跳过不阻断）。
pub fn validate_name(name: &str) -> Result<(), RouteError> {
    if name.is_empty() || name.len() > 64 {
        return Err(RouteError::InvalidName);
    }
    // 保留字与前缀拦截
    const RESERVED_NAMES: &[&str] = &[
        "pony", "api", "admin", "dsk", "health", "metrics", "sys", "static", "proxy", "token", "tokens",
    ];
    if name.starts_with("pony_") || RESERVED_NAMES.contains(&name) {
        return Err(RouteError::InvalidName);
    }
    let bytes = name.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return Err(RouteError::InvalidName);
    }
    if !bytes
        .iter()
        .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
    {
        return Err(RouteError::InvalidName);
    }
    Ok(())
}

/// §6.1 SSRF 校验（target_host 白名单校验，全部通过才入库）。
///
/// 规则 5（解析期防护）威胁分析——记录为审查依据：M1 不做 DNS 解析校验
/// （YAGNI）。上游是 CF Worker / Vercel Function 代为 fetch，本机不直接连接
/// target_host；SSRF 风险面在"Worker 替我们请求内网"——CF Worker 出口为公网
/// 数据中心，无法达本机/内网，Vercel 同理。故域名格式白名单已足够。
pub fn validate_host(host: &str) -> Result<(), RouteError> {
    // 规则 1：格式——非空、无 scheme、无路径/查询、无端口、长度 ≤253
    if host.is_empty() {
        return Err(RouteError::InvalidHost);
    }
    let lower = host.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Err(RouteError::InvalidHost);
    }
    if lower.contains('/') || lower.contains('?') {
        return Err(RouteError::InvalidHost);
    }
    // 含 ':' 拒绝：M1 路由仅支持 443 默认端口
    if lower.contains(':') {
        return Err(RouteError::InvalidHost);
    }
    if host.len() > 253 {
        return Err(RouteError::InvalidHost);
    }
    // 规则 2：字符白名单 [a-z0-9.-]（小写化后）。
    if !lower
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return Err(RouteError::InvalidHost);
    }
    // 域名结构
    let labels: Vec<&str> = lower.split('.').collect();
    if labels.len() < 2 {
        return Err(RouteError::InvalidHost);
    }
    let tld = labels[labels.len() - 1];
    if tld.len() < 2 || !tld.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(RouteError::InvalidHost);
    }
    for label in &labels[..labels.len() - 1] {
        let lb = label.as_bytes();
        if lb.is_empty() {
            return Err(RouteError::InvalidHost); // 连续点/前导点
        }
        if !lb[0].is_ascii_alphanumeric() || !lb[lb.len() - 1].is_ascii_alphanumeric() {
            return Err(RouteError::InvalidHost); // 段以 '-' 开头/结尾
        }
        if !lb
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            return Err(RouteError::InvalidHost);
        }
    }
    // 规则 3：黑名单后缀（内网域名与泛解析穿透域名）
    const BLOCKED_SUFFIXES: &[&str] = &[
        ".localhost",
        ".local",
        ".internal",
        ".localdomain",
        ".nip.io",
        ".sslip.io",
    ];
    if lower == "localhost" || BLOCKED_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return Err(RouteError::InvalidHost);
    }
    // 规则 4：metadata 端点与硬编码保留 IP
    if lower == "169.254.169.254" || lower == "127.0.0.1" {
        return Err(RouteError::InvalidHost);
    }
    Ok(())
}

/// §5 URL 拼装：剥离 path_query 首个 `/` 后拼 https://{host}/{rest}。
/// query 原样保留，禁止 percent-decode 再编码（透传原字节，SDK 已编码）。
fn build_target_url(target_host: &str, path_query: &str) -> String {
    let rest = path_query.strip_prefix('/').unwrap_or(path_query);
    if rest.is_empty() {
        format!("https://{target_host}/")
    } else {
        format!("https://{target_host}/{rest}")
    }
}

/// 在同步上下文执行 future：优先复用当前线程的 tokio runtime handle
/// （T6 handler 经 spawn_blocking 调用，可安全 block_on），否则构建临时
/// 单线程 runtime（单元测试等非 tokio 环境）。
fn block_on<F: Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(fut),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build temp runtime")
            .block_on(fut),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestResult {
    pub ok: bool, // 上游返回 2xx/3xx/401/403/405（链路通）为 true
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

pub struct RouteTable {
    store: Arc<Store>,
    /// 上游客户端表，test_route 用（C-P1-5）
    edges: Arc<HashMap<String, EdgeClient>>,
    table: RwLock<HashMap<String, RouteRow>>, // key = route name
}

impl RouteTable {
    /// 打开 Store 并全量加载路由表到内存；edges 为上游客户端表（test_route 用）。
    pub fn new(
        store: Arc<Store>,
        edges: Arc<HashMap<String, EdgeClient>>,
    ) -> Result<Self, RouteError> {
        let rt = Self {
            store,
            edges,
            table: RwLock::new(HashMap::new()),
        };
        rt.reload()?;
        Ok(rt)
    }

    fn lock_read(&self) -> RwLockReadGuard<'_, HashMap<String, RouteRow>> {
        // WHY unwrap_or_else：锁中毒（持锁线程 panic）时取回数据继续服务，
        // 同 Store::lock_conn 的降级策略
        self.table.read().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_write(&self) -> RwLockWriteGuard<'_, HashMap<String, RouteRow>> {
        self.table.write().unwrap_or_else(|p| p.into_inner())
    }

    /// 单一写路径的收尾步骤：写库成功后全量重载内存表。
    fn reload(&self) -> Result<(), RouteError> {
        let rows = self.store.list_routes()?;
        let map: HashMap<String, RouteRow> = rows.into_iter().map(|r| (r.name.clone(), r)).collect();
        *self.lock_write() = map;
        Ok(())
    }

    /// 解析：route 名 + 剩余 path?query → (target_url, upstream)。
    /// UnknownRoute / Disabled 语义见 T3 §4.2（两者均映射 404 同体，防枚举）。
    ///
    /// 规则 6（§6.1）：此处不重复 SSRF 校验——表内数据创建时已校验；
    /// DB 手工篡改属运维越权，不在威胁模型内。
    pub fn resolve(&self, name: &str, path_query: &str) -> Result<(String, Upstream), RouteError> {
        let row = self.lock_read().get(name).cloned();
        let Some(row) = row else {
            return Err(RouteError::UnknownRoute);
        };
        if !row.enabled {
            return Err(RouteError::Disabled);
        }
        let upstream = self.pick_upstream(&row.target_host, row.override_upstream.as_deref());
        Ok((build_target_url(&row.target_host, path_query), upstream))
    }

    /// 上游自动选择（C-P1-5 改 pub）：override 优先，否则按 host 规则。
    /// T6 /api/routes 列表的 effective_upstream 字段与 resolve 共用此决策。
    ///
    /// F8：override 值域扩展——除 "worker"|"vercel" 外，迁移导入的
    /// route_upstreams 绑定名（任意已配置上游名）同样生效；空串视作未设置
    /// （回退 host 规则）。
    pub fn pick_upstream(&self, target_host: &str, override_upstream: Option<&str>) -> Upstream {
        if let Some(o) = override_upstream {
            if !o.is_empty() {
                return parse_upstream(o).unwrap_or_else(|| {
                    // 非法值视作 Worker 并 warn（DB 手工篡改的兜底；API 层已在
                    // create/update 前置校验拒绝）
                    tracing::warn!(host = %target_host, override = %o, "invalid override_upstream, fallback to worker");
                    Upstream::Worker
                });
            }
        }
        if VERCEL_HOSTS.contains(&target_host.to_ascii_lowercase().as_str()) {
            Upstream::Vercel
        } else {
            Upstream::Worker
        }
    }

    /// 实时生效的上游（C-P1-5）：T6 GET /api/routes 的 effective_upstream 字段来源。
    /// 路由不存在 → None。
    pub fn effective_upstream(&self, name: &str) -> Option<Upstream> {
        self.lock_read()
            .get(name)
            .map(|r| self.pick_upstream(&r.target_host, r.override_upstream.as_deref()))
    }

    /// 管理面 CRUD（写库 + 同步刷新内存，模式同 T2 单一写路径）。
    ///
    /// F8：override_upstream 值域扩展——"worker"|"vercel"（S-P2-7）之外，
    /// 已配置上游名（edges 表键）同样合法；未知名仍 InvalidUpstream。
    pub fn create_route(&self, r: &NewRoute) -> Result<(), RouteError> {
        validate_name(&r.name)?;
        validate_host(&r.target_host)?;
        if let Some(o) = &r.override_upstream {
            if !valid_override(&self.edges, o) {
                return Err(RouteError::InvalidUpstream);
            }
        }
        if self.lock_read().contains_key(&r.name) {
            return Err(RouteError::Duplicate);
        }
        self.store.insert_route(r)?;
        self.reload()
    }

    pub fn list_routes(&self) -> Result<Vec<RouteRow>, RouteError> {
        let mut rows: Vec<RouteRow> = self.lock_read().values().cloned().collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name)); // 与 store ORDER BY name 一致
        Ok(rows)
    }

    /// 三态参数（C-P0-2，serde double_option 配合）：None=不改；Some(None)=清除；
    /// Some(Some(v))=设置。override_upstream 提供值时必须为 "worker"|"vercel"
    /// 或已配置上游名（F8，S-P2-7 扩展），否则 InvalidUpstream。
    /// 返回是否命中（false=路由不存在）。
    pub fn update_route(
        &self,
        name: &str,
        override_upstream: Option<Option<String>>,
        enabled: Option<bool>,
    ) -> Result<bool, RouteError> {
        if let Some(Some(o)) = &override_upstream {
            if !valid_override(&self.edges, o) {
                return Err(RouteError::InvalidUpstream);
            }
        }
        let updated = self.store.update_route(name, override_upstream, enabled)?;
        if updated {
            self.reload()?;
        }
        Ok(updated)
    }

    pub fn delete_route(&self, name: &str) -> Result<bool, RouteError> {
        let deleted = self.store.delete_route(name)?;
        if deleted {
            self.reload()?;
        }
        Ok(deleted)
    }

    /// 连通性实测：经对应上游请求 https://{target_host}/ 首页（C-P1-5）。
    ///
    /// 成功判定（2026-08 修订）：两个 edge（cf-worker/vercel）在**透传源站响应**时
    /// 都会附加 `x-proxy-edge` 标记头；edge 自身错误（鉴权 403/参数 400/上游
    /// 502/未配置 500）不带该头。因此：
    /// - 收到带标记头的响应 → 源站可达 → ok=true（**任意状态码**——
    ///   api.anthropic.com 首页 404、api.x.ai 返回 421 都不代表链路不通）；
    /// - 收到无标记头的响应 → edge 自身错误 → ok=false（error 附状态码）；
    /// - 传输失败/超时 → ok=false（原语义不变）。
    ///
    /// S-P2-6：独立 10s 总超时（tokio::time::timeout 外层等效），管理面实测
    /// 最坏 10s 返回。网络失败/超时不是 Err：返回 Ok(TestResult { ok: false, .. })。
    /// 上游未配置（edges 中无该 upstream 对应客户端）→ Ok(ok=false,
    /// error=Some("upstream not configured"))（C-P1-5，非 Err）。
    pub fn test_route(&self, name: &str) -> Result<TestResult, RouteError> {
        let row = self.lock_read().get(name).cloned();
        let Some(row) = row else {
            return Err(RouteError::UnknownRoute);
        };
        let upstream = self.pick_upstream(&row.target_host, row.override_upstream.as_deref());
        let Some(edge) = self.edges.get(upstream.as_str()) else {
            return Ok(TestResult {
                ok: false,
                status: None,
                latency_ms: None,
                error: Some("upstream not configured".into()),
            });
        };
        let req = ForwardRequest {
            method: reqwest::Method::GET,
            target_url: format!("https://{}/", row.target_host),
            headers: reqwest::header::HeaderMap::new(),
            body: None,
        };
        let start = Instant::now();
        let resp = block_on(tokio::time::timeout(TEST_TIMEOUT, edge.execute(req)));
        let latency_ms = start.elapsed().as_millis() as u64;
        match resp {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                // 标记头=edge 透传的源站响应（链路通）；缺失=edge 自身错误（链路断）
                if resp.headers().contains_key("x-proxy-edge") {
                    Ok(TestResult {
                        ok: true,
                        status: Some(status),
                        latency_ms: Some(latency_ms),
                        error: None,
                    })
                } else {
                    Ok(TestResult {
                        ok: false,
                        status: Some(status),
                        latency_ms: Some(latency_ms),
                        error: Some(format!("edge 自身错误（status {status}），未到达源站")),
                    })
                }
            }
            Ok(Err(e)) => Ok(TestResult {
                ok: false,
                status: None,
                latency_ms: None,
                error: Some(format!("request failed: {e}")),
            }),
            Err(_) => Ok(TestResult {
                ok: false,
                status: None,
                latency_ms: None,
                error: Some("timeout after 10s".into()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join(format!("{tag}.db"));
        let store = Store::open(&db).unwrap().0;
        (dir, Arc::new(store))
    }

    fn empty_edges() -> Arc<HashMap<String, EdgeClient>> {
        Arc::new(HashMap::new())
    }

    fn new_route(name: &str, host: &str, override_upstream: Option<&str>) -> NewRoute {
        NewRoute {
            name: name.into(),
            target_host: host.into(),
            override_upstream: override_upstream.map(|s| s.into()),
        }
    }

    fn table(tag: &str) -> (tempfile::TempDir, RouteTable) {
        let (dir, store) = temp_store(tag);
        (dir, RouteTable::new(store, empty_edges()).unwrap())
    }

    /// F8：带 localstub 上游的表（Named 绑定用例）。
    fn table_with_stub(tag: &str) -> (tempfile::TempDir, RouteTable) {
        let (dir, store) = temp_store(tag);
        let mut edges = HashMap::new();
        edges.insert(
            "localstub".to_string(),
            EdgeClient::new("http://127.0.0.1:1", "s").unwrap(),
        );
        (dir, RouteTable::new(store, Arc::new(edges)).unwrap())
    }

    // ---- §8.1 pick_upstream 规则 ----

    #[test]
    fn pick_upstream_rules() {
        let (_dir, rt) = table("pick");
        assert_eq!(rt.pick_upstream("api.openai.com", None), Upstream::Vercel);
        assert_eq!(rt.pick_upstream("opencode.ai", None), Upstream::Vercel);
        assert_eq!(rt.pick_upstream("api.anthropic.com", None), Upstream::Worker);
        assert_eq!(rt.pick_upstream("www.google.com", None), Upstream::Worker);
        // 大小写不敏感
        assert_eq!(rt.pick_upstream("API.OPENAI.COM", None), Upstream::Vercel);
        // override 优先于 host 规则；已知名归一为枚举值
        assert_eq!(
            rt.pick_upstream("api.anthropic.com", Some("vercel")),
            Upstream::Vercel
        );
        assert_eq!(
            rt.pick_upstream("api.anthropic.com", Some("worker")),
            Upstream::Worker
        );
        // F8：Named 绑定（迁移导入的 route_upstreams 名）原样生效；
        // 空串视作未设置，回退 host 规则。
        assert_eq!(
            rt.pick_upstream("echo.example.com", Some("localstub")),
            Upstream::Named("localstub".into())
        );
        assert_eq!(rt.pick_upstream("echo.example.com", Some("")), Upstream::Worker);
    }

    // ---- §8.2 override 优先 ----

    #[test]
    fn override_takes_precedence_in_resolve() {
        let (_dir, rt) = table("override");
        rt.create_route(&new_route("openai", "api.openai.com", Some("worker")))
            .unwrap();
        let (_, up) = rt.resolve("openai", "/").unwrap();
        assert_eq!(up, Upstream::Worker);

        // F8：Named 绑定在 resolve 中同样优先于 host 规则（须已配置上游名）
        let (_dir2, rt2) = table_with_stub("override_named");
        rt2.create_route(&new_route("echo", "echo.example.com", Some("localstub")))
            .unwrap();
        let (_, up) = rt2.resolve("echo", "/ping").unwrap();
        assert_eq!(up, Upstream::Named("localstub".into()));
    }

    // ---- §8.3 effective_upstream 与 resolve 决策一致 ----

    #[test]
    fn effective_upstream_matches_resolve() {
        let (_dir, rt) = table("effective");
        rt.create_route(&new_route("anthropic", "api.anthropic.com", None))
            .unwrap();
        let (_, up) = rt.resolve("anthropic", "/").unwrap();
        assert_eq!(rt.effective_upstream("anthropic"), Some(up));
        assert_eq!(rt.effective_upstream("nope"), None);
    }

    // ---- §8.4 resolve URL 拼装向量（§5）----

    #[test]
    fn resolve_url_vectors() {
        let (_dir, rt) = table("url");
        rt.create_route(&new_route("anthropic", "api.anthropic.com", None))
            .unwrap();
        let (url, _) = rt
            .resolve("anthropic", "/v1/messages?beta=true")
            .unwrap();
        assert_eq!(url, "https://api.anthropic.com/v1/messages?beta=true");
        let (url, _) = rt.resolve("anthropic", "/").unwrap();
        assert_eq!(url, "https://api.anthropic.com/");
        let (url, _) = rt.resolve("anthropic", "").unwrap();
        assert_eq!(url, "https://api.anthropic.com/");
    }

    // ---- §8.5 未知 / 禁用 ----

    #[test]
    fn unknown_and_disabled() {
        let (_dir, rt) = table("unk");
        assert!(matches!(
            rt.resolve("nope", "/"),
            Err(RouteError::UnknownRoute)
        ));
        rt.create_route(&new_route("anthropic", "api.anthropic.com", None))
            .unwrap();
        rt.update_route("anthropic", None, Some(false)).unwrap();
        assert!(matches!(
            rt.resolve("anthropic", "/"),
            Err(RouteError::Disabled)
        ));
    }

    // ---- §8.6 热生效 ----

    #[test]
    fn hot_reload_on_write() {
        let (_dir, rt) = table("hot");
        let nr = new_route("anthropic", "api.anthropic.com", None);
        rt.create_route(&nr).unwrap();
        assert!(rt.resolve("anthropic", "/").is_ok(), "create 后立即可见");

        assert!(rt.delete_route("anthropic").unwrap());
        assert!(matches!(
            rt.resolve("anthropic", "/"),
            Err(RouteError::UnknownRoute)
        ));

        rt.create_route(&nr).unwrap();
        assert!(rt.update_route("anthropic", None, Some(false)).unwrap());
        assert!(matches!(
            rt.resolve("anthropic", "/"),
            Err(RouteError::Disabled)
        ));
    }

    // ---- §8.7 validate_host 通过 ----

    #[test]
    fn validate_host_accepts() {
        for host in ["api.anthropic.com", "www.google.com", "opencode.ai"] {
            assert!(validate_host(host).is_ok(), "{host} 应通过");
        }
    }

    // ---- §8.8 validate_host 拒绝 ----

    #[test]
    fn validate_host_rejects() {
        let rejected = [
            "http://api.anthropic.com", // scheme
            "api.anthropic.com/path",   // 路径
            "api.anthropic.com:8443",   // 端口
            "192.168.1.1",              // 裸 IP
            "127.0.0.1",                // 裸 IP
            "localhost",                // 黑名单
            "foo.localhost",            // 黑名单后缀
            "svc.internal",             // 黑名单后缀
            "metadata.google.internal", // metadata 端点（.internal 覆盖）
            "",                         // 空串
            "*.example.com",            // 通配符（C-P2-1）
            "api_example.com",          // 非法字符（下划线）
            "api example.com",          // 非法字符（空格）
        ];
        for host in rejected {
            assert!(
                matches!(validate_host(host), Err(RouteError::InvalidHost)),
                "{host} 应 InvalidHost"
            );
        }
        // 大写输入先小写化再校验 → 合法
        assert!(validate_host("Api.Example.com").is_ok());
    }

    // ---- §8.9 Duplicate ----

    #[test]
    fn duplicate_name_rejected() {
        let (_dir, rt) = table("dup");
        rt.create_route(&new_route("anthropic", "api.anthropic.com", None))
            .unwrap();
        assert!(matches!(
            rt.create_route(&new_route("anthropic", "www.google.com", None)),
            Err(RouteError::Duplicate)
        ));
    }

    // ---- §8.10 name 校验 ----

    #[test]
    fn name_validation() {
        let (_dir, rt) = table("name");
        let invalid = [
            "pony_x", // 保留前缀（S-P1-4）
            "pony",   // 与 token 前缀段混淆（§8.10 测试向量，从严拒绝）
            "OpenAI", // 大写
            "",       // 空串
            &"a".repeat(65), // 超长（正则上限总长 64）
        ];
        for name in invalid {
            assert!(
                matches!(
                    rt.create_route(&new_route(name, "api.anthropic.com", None)),
                    Err(RouteError::InvalidName)
                ),
                "{name} 应 InvalidName"
            );
        }
        rt.create_route(&new_route("a", "api.anthropic.com", None))
            .unwrap();
        assert!(rt.resolve("a", "/").is_ok(), "单字符合法名应通过");
    }

    // ---- §8.11 update 三态（C-P0-2）----

    #[test]
    fn update_route_three_state() {
        let (_dir, rt) = table("threestate");
        rt.create_route(&new_route("anthropic", "api.anthropic.com", Some("vercel")))
            .unwrap();
        assert_eq!(rt.effective_upstream("anthropic"), Some(Upstream::Vercel));

        // None：不改该列
        rt.update_route("anthropic", None, None).unwrap();
        assert_eq!(rt.effective_upstream("anthropic"), Some(Upstream::Vercel));

        // Some(None)：清除 override → 回落自动选择（anthropic → Worker）
        assert!(rt
            .update_route("anthropic", Some(None), None)
            .unwrap());
        assert_eq!(rt.effective_upstream("anthropic"), Some(Upstream::Worker));

        // Some(Some("vercel"))：设置
        assert!(rt
            .update_route("anthropic", Some(Some("vercel".into())), None)
            .unwrap());
        assert_eq!(rt.effective_upstream("anthropic"), Some(Upstream::Vercel));

        // Some(Some("bogus"))：InvalidUpstream（S-P2-7；F8 后未配置名仍拒绝）
        assert!(matches!(
            rt.update_route("anthropic", Some(Some("bogus".into())), None),
            Err(RouteError::InvalidUpstream)
        ));

        // 不存在的路由 → false（非 Err）
        assert!(!rt.update_route("nope", None, Some(true)).unwrap());
    }

    // ---- §8.12 test_route 实测（网络用例，CI 门禁跳过）----

    #[test]
    #[ignore]
    fn test_route_live_openai() {
        let worker_url =
            std::env::var("PPROXY_TEST_WORKER_URL").expect("需设置 PPROXY_TEST_WORKER_URL");
        let worker_secret =
            std::env::var("PPROXY_TEST_WORKER_SECRET").expect("需设置 PPROXY_TEST_WORKER_SECRET");
        let mut edges = HashMap::new();
        edges.insert(
            "worker".to_string(),
            EdgeClient::new(&worker_url, &worker_secret).unwrap(),
        );
        let (_dir, store) = temp_store("live");
        let rt = RouteTable::new(store, Arc::new(edges)).unwrap();
        rt.create_route(&new_route("openai", "api.openai.com", Some("worker")))
            .unwrap();
        let res = rt.test_route("openai").unwrap();
        // 2026-08 修订：带 x-proxy-edge 头即判通（任意源站状态码，OpenAI 无凭据
        // 首页可能 401/403），不再断言具体状态
        assert!(res.ok, "源站可达即应 ok，实际: {:?}", res.error);
        assert!(res.status.is_some());
        assert!(res.error.is_none());
    }

    // ---- §8.13 test_route 上游未配置（C-P1-5）----

    #[test]
    fn test_route_upstream_not_configured() {
        let (_dir, rt) = table("noedge");
        assert!(matches!(
            rt.test_route("nope"),
            Err(RouteError::UnknownRoute)
        ));
        rt.create_route(&new_route("anthropic", "api.anthropic.com", None))
            .unwrap();
        let res = rt.test_route("anthropic").unwrap();
        assert!(!res.ok);
        assert_eq!(res.error.as_deref(), Some("upstream not configured"));
        assert_eq!(res.status, None);
    }
}
