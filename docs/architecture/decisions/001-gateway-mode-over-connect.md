# ADR-001: API 网关模式而非 CONNECT 隧道

- 状态: 已接受 | 日期: 2026-08-21

## 背景

客户端经本地代理访问 HTTPS 站点的标准方式是 CONNECT 隧道（端到端加密）。但免费海外出口（CF Worker、Vercel Function）只接受 HTTP 语义请求，无法建立原始 TCP 隧道——Worker/Vercel 的转发模式是 `?url=<target>` 明文 HTTP 转发。

## 决策

pproxy 采用 **API 网关模式**：客户端不设系统代理，而是把 SDK 的 base_url 指向网关（`http://127.0.0.1:8899/{service}/...`），网关将请求转写为上游 `?url=` 格式。

## 后果

- ✅ 零客户端配置成本（改一个环境变量）
- ✅ 无需 MITM/自签 CA
- ❌ 不支持系统级全局代理（CONNECT 无法经上游转发）
- ❌ 仅覆盖 HTTP(S) API 场景，不覆盖任意 TCP
- CONNECT 方法保留直连兜底（国内可达站点）
