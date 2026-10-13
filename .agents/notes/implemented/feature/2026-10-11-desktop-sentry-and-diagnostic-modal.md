# Agent Note: 接入桌面端 Sentry 可观测性与连接状态就地排障

Status: implemented

## Problem

桌面端用户在公网环境下偶发连接状态全标红失败，但前端 UI 将具体底层错误（如 TLS 握手超时、DNS 阻断、Token 鉴权 401、端口被拒绝等）折叠为纯“失败”，导致现场排查极度困难；同时缺乏结构化的客户端错误上报机制。

## Decision

1. **引入 `@sentry/vue` 可观测性 SDK**：
   - 在 `desktop/src/lib/sentry.ts` 实现初始化、敏感数据脱敏（自动剥除 Authorization/token）、Breadcrumb 追踪与消息上报能力；
   - 支持通过 `VITE_SENTRY_DSN` 或全局 `window.__PONY_SENTRY_DSN__` 动态注入 DSN，未配置时自动本地降级不报错。
2. **连接状态失败诊断透出与一键排障日志**：
   - 在 `DashboardView.vue` 探针拨测（`probeSite`）失败时自动捕获并记入 Sentry Breadcrumb 与 Event；
   - 连接状态标红时，用户可直接点击查看弹窗详情，并支持一键复制标准排障日志。

## Alternatives considered

- **仅在后端打印日志**：公网环境客户端机器无法直接被服务端读取日志，客户端埋点必不可少；
- **全量无条件上报**：对“代理未启用”等正常业务待命状态做前置拦截过滤，防 Sentry 配额污染与噪声风暴。

## Consequences

- 接入完成且通过 `vue-tsc` 与全部 116 项 Vitest 单元测试；
- 彻底解决公网连接失败黑盒问题。
