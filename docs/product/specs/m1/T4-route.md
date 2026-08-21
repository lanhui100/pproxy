# T4 — 路由引擎（route.rs）

> 依赖: T1 | 并行波次 2（与 T2/T5 并行，仅新增 route.rs）| 上游: M1 spec §3.3 / §7 SSRF

## 1. 目标

路由表从 config.json 硬编码迁至 SQLite 并支持热生效；实现自动上游选择 + override；实现 SSRF 校验（target_host 白名单校验）。

## 2. 文件位置

- 新增 `crates/core/src/route.rs`；`lib.rs` 追加 `pub mod route;`（仅此一行）。
- 无新增 Cargo 依赖。

## 3. 公开接口（Rust 签名级）

```rust
// crates/core/src/route.rs
use crate::store::{Store, RouteRow, StoreError, now_unix};
use crate::edge::EdgeClient;
use std::sync::RwLock;
use std::collections::HashMap;

#[derive(Debug)]
pub enum RouteError {
    UnknownRoute,
    Disabled,
    InvalidName,        // 创建时：^[a-z][a-z0-9_-]{0,63}$，且禁止 pony_ 前缀（§6 第 0 条）
    InvalidHost,        // 创建时：SSRF 校验失败（§6）
    InvalidUpstream,    // override_upstream 非法（F8：非 worker|vercel 且非已配置上游名/空串）
    Duplicate,          // name 已存在
    Store(StoreError),
}
impl Display + Error + From<StoreError>;

// F8 修订：Named(String) 承载迁移导入的 route_upstreams 绑定（任意已配置上游名）。
// 序列化为名字字符串；"worker"|"vercel" 归一为对应变体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream { Worker, Vercel, Named(String) }

pub struct RouteTable { ... }   // 内部见 §4

impl RouteTable {
    /// 打开 Store 并全量加载路由表到内存；edges 为上游客户端表（test_route 用）
    pub fn new(store: Arc<Store>, edges: Arc<HashMap<String, EdgeClient>>)
        -> Result<Self, RouteError>;

    /// 解析：route 名 + 剩余 path?query → (target_url, upstream)
    /// UnknownRoute / Disabled 语义见 T3 §4.2
    pub fn resolve(&self, name: &str, path_query: &str)
        -> Result<(String, Upstream), RouteError>;

    /// 上游自动选择（C-P1-5 改 pub）：override 优先，否则按 host 规则。
    /// T6 /api/routes 列表的 effective_upstream 字段与 resolve 共用此决策。
    pub fn pick_upstream(&self, target_host: &str, override_upstream: Option<&str>) -> Upstream;

    /// 实时生效的上游（C-P1-5）：T6 GET /api/routes 的 effective_upstream 字段来源。
    /// 路由不存在 → None。
    pub fn effective_upstream(&self, name: &str) -> Option<Upstream>;

    /// 管理面 CRUD（写库 + 同步刷新内存，模式同 T2 单一写路径）
    pub fn create_route(&self, r: &NewRoute) -> Result<(), RouteError>;
    pub fn list_routes(&self) -> Result<Vec<RouteRow>, RouteError>;
    /// 三态参数（C-P0-2，serde double_option 配合）：None=不改；Some(None)=清除；Some(Some(v))=设置。
    /// F8：override 值域——"worker"|"vercel" 恒合法（worker 由 config.worker_url 提供，
    /// 不经 upstreams map）；空串拒绝（语义=未设置，应传 null 清除）；其余须为已配置上游名
    /// （edges 表键），否则 InvalidUpstream。
    pub fn update_route(&self, name: &str,
        override_upstream: Option<Option<String>>, enabled: Option<bool>) -> Result<bool, RouteError>;
    pub fn delete_route(&self, name: &str) -> Result<bool, RouteError>;

    /// 连通性实测：经对应上游请求 https://{target_host}/ 首页。
    /// 使用独立 reqwest Client（10s 总超时，S-P2-6），与 EdgeClient 内部 client 隔离，
    /// 避免管理面实测被数据面连接池/长超时拖累。
    /// 网络失败/超时不是 Err：返回 Ok(TestResult { ok: false, error: Some(msg), .. })。
    /// 上游未配置（edges 中无该 upstream 对应客户端）→ Ok(TestResult { ok: false,
    ///   error: Some("upstream not configured") })（C-P1-5，非 Err——路由存在性已由调用方校验）。
    pub fn test_route(&self, name: &str) -> Result<TestResult, RouteError>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TestResult {
    pub ok: bool,          // 上游返回 2xx/3xx/401/403/405（链路通）为 true
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewRoute {
    pub name: String,
    pub target_host: String,
    pub override_upstream: Option<String>,  // F8 值域：worker|vercel|已配置上游名（见 update_route 注）
}
```

### 3.1 上游自动选择（精确）

```rust
pub fn pick_upstream(&self, target_host: &str, override_upstream: Option<&str>) -> Upstream {
    if let Some(o) = override_upstream {
        if !o.is_empty() {  // F8：空串视作未设置，回退 host 规则
            return parse_upstream(o).unwrap_or_else(|| {
                Upstream::Worker  // 非法值视作 Worker 并 warn（DB 手工篡改兜底）
            });
        }
    }
    match target_host.to_ascii_lowercase().as_str() {
        "api.openai.com" | "opencode.ai" => Upstream::Vercel,
        _ => Upstream::Worker,
    }
}
// F8：parse_upstream 接受任意非空值——"worker"|"vercel" 归一为对应变体，
// 其余归一为 Named(名字)。
```

- `resolve` 的 upstream 决策：直接调用 `pick_upstream(target_host, override_upstream.as_deref())`。
- `RouteRow.upstream` 列存**创建时**的自动选择结果（仅展示用途），实际转发永远实时决策——保证 pick_upstream 规则变更后无需迁移数据。

## 4. 内存结构与热生效

```rust
pub struct RouteTable {
    store: Arc<Store>,
    edges: Arc<HashMap<String, EdgeClient>>,     // test_route 用（C-P1-5）
    test_client: reqwest::Client,                // test_route 专用，10s 超时（S-P2-6）
    table: RwLock<HashMap<String, RouteRow>>,    // key = route name
}
```

- 与 T2 相同的一致性模式：**单一写路径**（create/update/delete 只在 RouteTable 方法内发生），写库成功后同步全量重载内存。热生效 = 方法返回即生效，无轮询、无文件监听。
- `resolve` / `effective_upstream` 为纯内存读（`RwLock` read），数据面热路径无锁竞争问题（个人网关 QPS 极低）。

## 5. resolve 的 URL 拼装（精确，含测试向量）

输入 `name="anthropic"`, `path_query="/v1/messages?beta=true"`：
1. 查内存表：存在且 `enabled` → `target_host = "api.anthropic.com"`。
2. 剥离 path_query 首个 `/` 后拼 `https://{target_host}/{rest}`；query 保留原样。
3. 结果 `https://api.anthropic.com/v1/messages?beta=true`。

边界：
- `path_query="/"` → `https://{host}/`。
- `path_query=""` → `https://{host}/`。
- path_query 已含 `?` → 原样保留（不重复编码）。
- **禁止**对 path_query 做 percent-decode 再编码（透传原字节，SDK 已编码）。

## 6. 校验规则（name + SSRF，M1 spec §7 安全必做）

### 6.0 name 校验（先于一切 host 校验）

1. 正则 `^[a-z][a-z0-9_-]{0,63}$` 不匹配 → InvalidName。
2. **保留前缀显式校验（S-P1-4/C-P1-2 裁决）**：`name.starts_with("pony_")` → InvalidName。理由：数据面以 `pony_` 前缀区分路径中的 token 段与 route 段（T3 §4.1），该歧义必须由路由创建时显式排除，不能依赖正则"天然排除"的错误断言。`__admin__`（含下划线开头）已被正则第 1 字符 `[a-z]` 拒绝。

### 6.1 SSRF 校验（target_host 白名单校验）

`create_route` 时（update 不允许改 host）对 `target_host` 执行 `validate_host()`，全部通过才入库：

1. **格式**：非空、无 scheme（`http://`/`https://` 前缀 → InvalidHost）、无路径/查询（含 `/` 或 `?` → InvalidHost）、无端口（含 `:` → InvalidHost，M1 路由仅支持 443 默认端口）、长度 ≤253。
2. **字符白名单**：仅 `[a-z0-9.-]`（小写化后），正则 `^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$`——即合法域名（**C-P2-1/S-P2-9 裁决：删除 `(\*\.)?` 通配前缀**，M1 路由均为精确域名，通配语义未定义且扩大 SSRF 面）；拒绝裸 IP 形式（纯数字+点，如 `192.168.1.1`、`127.0.0.1`）→ InvalidHost。理由：路由目标是公网 API 域名，裸 IP 无业务需求且是 SSRF 主载体。
3. **黑名单后缀**：`localhost`、`*.localhost`、`*.local`、`*.internal`、`.internal` 结尾任意层级、`*.localdomain` → InvalidHost。
4. **metadata 黑名单**（云元数据端点域名化变体）：`metadata.google.internal` 已被第 3 条覆盖；额外精确拒绝 `169.254.169.254`（IP 形式第 2 条已拒）。
5. **解析期防护**：M1 不做 DNS 解析校验（YAGNI：上游是 CF Worker/Vercel Function 代为 fetch，本机不直接连接 target_host；SSRF 风险面在"Worker 替我们请求内网"——CF Worker 出口为公网数据中心，无法达本机/内网，Vercel 同理。故域名格式白名单已足够，记录此分析为审查依据）。
6. `resolve` 时**不再**重复校验（表内数据创建时已校验；DB 手工篡改属运维越权，不在威胁模型内，注释说明）。

## 7. 依赖任务

T1（Store/RouteRow）。`test_route` 需要 EdgeClient（core 已有，经 `new(store, edges)` 注入）。

## 8. 单元测试清单

1. `pick_upstream`：`api.openai.com`→Vercel、`opencode.ai`→Vercel、`api.anthropic.com`→Worker、`www.google.com`→Worker、大小写不敏感（`API.OPENAI.COM`→Vercel）；override="vercel" 的 anthropic → Vercel；override 非法值 → Worker。
2. override 优先：override="worker" 的 openai 路由 → Worker。
3. `effective_upstream`（C-P1-5）：存在路由 → 与 resolve 决策一致；不存在 → None。
4. resolve 拼装：§5 全部向量（含 `""`、`"/"`、带 query）。
5. 未知 route → UnknownRoute；`enabled=false` → Disabled。
6. 热生效：create_route 后 resolve 立即可见；delete 后立即 UnknownRoute；update enabled=false 后立即 Disabled。
7. `validate_host` 通过：`api.anthropic.com`、`www.google.com`、`opencode.ai`。
8. `validate_host` 拒绝：`http://api.anthropic.com`（scheme）、`api.anthropic.com/path`（路径）、`api.anthropic.com:8443`（端口）、`192.168.1.1`（IP）、`127.0.0.1`、`localhost`、`foo.localhost`、`svc.internal`、`metadata.google.internal`、空串、`*.example.com`（通配，C-P2-1）、含下划线/空格非法字符（大写输入先小写化再校验——`Api.Example.com` 合法）。
9. Duplicate：重复 name create → RouteError::Duplicate。
10. name 校验：`pony_x`（保留前缀，S-P1-4）→ InvalidName；`pony`（无下划线，不匹配正则首段约束）→ InvalidName；`OpenAI`（大写）→ InvalidName；`a`（合法单字符）→ 通过；64+ 字符 → InvalidName。
11. update_route 三态（C-P0-2）：`None` 不改列；`Some(None)` 清除 override；`Some(Some("vercel"))` 设置；F8 值域：`Some(Some("worker"))` 合法、`Some(Some(""))` 拒绝、`Some(Some("<已配置上游名>"))` 合法、`Some(Some("bogus"))`（非配置名）→ InvalidUpstream。
12. `test_route`：对 `api.openai.com` 实测返回 `status=Some(401)` 且 `ok=true`（网络用例标记 `#[ignore]`，集成时 `cargo test -- --ignored` 跑；CI/门禁跑非网络用例）。
13. `test_route` 上游未配置（C-P1-5）：edges 空表构造 → `Ok(ok=false, error=Some("upstream not configured"))`，非 Err。

## 9. 验收标准

- `cargo test -p pproxy-core route::` 全绿（网络用例除外，标 `#[ignore]`）。
- 迁移后 7 路由 resolve 行为与旧 `resolve_route()` 一致（T8 回归断言）。
- lib.rs 改动仅 `pub mod route;` 一行。
- 安全审查通过项：validate_host 覆盖 §6 全部规则（含 pony_ 前缀显式拒绝、无通配前缀），代码注释含第 5 条威胁分析。
