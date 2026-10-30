# Agent Note: absolute-form 请求显式拒绝，不再伪装成 401 凭据错误

Status: implemented

## Problem

数据面 `auth_middleware`（`crates/server/src/gateway.rs`）对所有进入 hyper 的请求按
origin-form 解析路径：`/v1/models` → 首段 `v1` 被当作 route 名 → `tokens.verify`
找不到 → 401 `{"error":"unauthorized"}`。

当客户端把本监听器当作通用 HTTP 正向代理（absolute-form 请求行
`GET http://host:port/path HTTP/1.1`）时，这个 401 与真实原因完全无关：

- 真实原因是**能力缺失**——数据面只实现 CONNECT 隧道（`connect.rs`，且限 443 +
  allowlist），没有任何明文 HTTP 转发实现；
- 报出的是**凭据错误**，排障者会去查 token、查密钥、查网关鉴权，而正确的方向是
  查代理配置。

2026-10-19 的现场即为此：dsh 把 `baseURL` 从公网域名切到 ClusterIP
`10.43.141.157:8080` 后全量失败。`no_proxy` 里有 CIDR `10.0.0.0/8`，但 harness 的
`bypassesProxy()`（`packages/util/http-proxy/src/policy.ts`）**明确不解析 CIDR**
——源码注释原文："CIDR notation is not matched — an operating system's bypass list
often carries `10.0.0.0/8`, which must be rewritten as suffixes."。于是请求被发往
`127.0.0.1:8899`，pproxy 回 401 `{"error":"unauthorized"}`，而 ponyllm 网关日志里
**一条记录都没有**——请求从未到达网关。这条误导性的 401 让排查绕了整条链路。

附带发现：根路径 `/` 因 `auth_middleware` 的提前 `next.run()` 放行，反而返回 200
info 页；`/foo` 却 401。同一类请求两种结果，进一步掩盖了问题本质。

## Decision

在 `auth_middleware` 的最前面、任何路径解析之前，用
`req.uri().scheme().is_some() || req.uri().authority().is_some()` 识别 absolute-form
（RFC 9112 §3.2.2），一律返回 **501 `forward_proxy_unsupported`**，并在响应体里
点名正确用法（CONNECT 隧道）与修复方向（把目标写进 `no_proxy`），同时
`tracing::warn!` 记录 `target`。

选择 501 而非 401/403：501 表达"本服务不实现该能力"，语义准确且不会诱导调用方
去重试凭据；501 也不会被通用 HTTP 客户端当成"临时故障"做无意义退避重试。

判定放在鉴权之前：这是能力边界，与调用方持有何种 token 无关。带合法 token 的
absolute-form 请求同样 501（有测试覆盖），否则又会造出"有 token 就 200"的假象。

## Alternatives considered

- **实现明文 HTTP 正向转发**：README 架构图确实把 "HTTP Proxy" 与 CONNECT 并列。
  但补一个正确的转发器要处理 hop-by-hop 头、`Proxy-Authorization`、TLS 剥离边界、
  SSRF（禁止私网目标）与 allowlist 一致性——远超本次排障范围，且会扩大数据面的
  安全面。真正的修复在调用方（`no_proxy` 写对），服务端只需**说清真相**。
- **保持 401 但在 body 里加提示**：改动最小，但 401 是"凭据错误"的强语义，任何
  按状态码分流的客户端/监控都会先入为主；且 401 常被重试逻辑特殊对待。
- **直接断开连接不回响应**：能避免误导，但排障者只看到 `connection reset`，
  信息量为零，比 401 更差。
- **只改文档说明"不支持 HTTP 代理"**：不解决现场——调用方拿到的仍是 401，
  文档不会在错误发生的那一刻出现。

## Consequences

- absolute-form 请求现在 501 + 可操作提示，不再伪装成凭据错误；origin-form
  的鉴权与路由行为完全不变（有对照组测试 `origin_form_still_reaches_route_resolution`）。
- README（中/英）、PRD、TECH_DESIGN 与 m6 / connect-tunnel spec 的正向代理声明已
  同步为 CONNECT-only（目标端口 443），并写明 absolute-form 返回 501。
- 客户端侧配套修复：`~/.bashrc` 的 `no_proxy` 追加 `10.43.141.157`（CIDR 对
  Node/undici 与 dsh 的 `bypassesProxy` 均无效，必须逐个列裸 IP）。
- 机械验收：`cargo test -p pproxy-server --lib gateway` 全绿，含新增
  `absolute_form_request_rejected_as_unsupported_not_unauthorized` 与
  `origin_form_still_reaches_route_resolution`。
- `saturation_contract::over_limit_fast_fail_and_recovery_contract` 的既有失败已单独
  修复（测试日志捕获未关闭 ANSI，解析器把 `\x1b[0m` 读成 0）；见
  `2026-10-19-fix-saturation-contract-ansi-parsing.md`。
