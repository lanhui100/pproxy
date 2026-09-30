# Agent Note: gate-camouflage-page

Status: implemented

## Problem

gate worker 非 `/ws` 路径返回裸 `404 not found`——这是最典型的"可疑 Worker"指纹
（edgetunnel 社区实证：CF 重点风控 Worker 代理，裸 404 会被扫号/指纹工具快速识别）。
需求：低成本消除该指纹，让 gate 域名在浏览器/curl 下看起来是普通站点，同时不改变
`/ws` 隧道协议、不动运维探活端点（/debug、/debug/egress）。

## Decision

给 gate worker 增加**伪装页 + 反指纹**（B011，edgetunnel 借鉴，负向清单除外）：

1. **伪装页**：非 `/ws`（且非 /debug、非 /debug/egress）路径统一返回 **200 仿 nginx
   欢迎页**（`text/html`，标准 nginx 欢迎内容），不再裸 404。浏览器直开 gate 域名
   看到的是普通站点首面，curl 非 /ws 得到欢迎页而非 "not found"。
2. **特征串运行时拼装**：`/ws` 等关键路径/错误文案不落明文（`['/','w','s'].join('')`
   等价拼装），降低静态指纹扫描（对 worker 源码做正则"查杀"的自动化）的命中率；
   token 鉴权维持 Bearer header（不进 URL 查询串，现状即如此，文档明示）。
3. **明确不抄**（负向清单）：不做 edgetunnel 的"多语言无后门声明"注释垫片——pproxy
   自托管开源自用，垫片引入信任争议且无必要（issue #1135 教训）。
4. **行为边界**：/debug、/debug/egress（运维诊断）与 `/ws`（隧道）行为完全不变；
   伪装页实现抽到 `camouflage.mjs` 纯函数（node 直测），worker.js 只接线。

## Alternatives considered

- **A. 非 /ws 返回 404 仿页（nginx 404 风格）**：落选。裸 404 文本被指纹工具高权重
  匹配"Worker 缺省"；200 欢迎页更像普通站点，且与 edgetunnel 主流做法一致。
- **B. 仿 CF Error 1101 页**：落选。1101 页面在社区已被大量复制，反而不具伪装性，
  且与"gate 是 CF 托管"的事实重合，色系/文案易被针对性识别。
- **C. 特征串硬编码 + 依赖混淆（垫片/注释墙）**：落选（负向清单）。运行时拼装已
  覆盖"躲自动化正则"的现实威胁；垫片是审计污染源（#1135），不做。
- **D. 伪装页内联进 worker.js**：落选。HTML 常量会显著增大主文件且难单测；独立
  `camouflage.mjs` 模块让验收可机械执行（node 单测断言返回 200 + nginx 欢迎内容）。

## Consequences

- gate 域名在非隧道路径下的指纹从"可疑 Worker"变为"普通站点"，防扫号成本极低；
- /ws 协议、/debug、/debug/egress 无任何行为变化；客户端/探活不受影响；
- 单测可机械验收（`node deploy/cf-gate-worker/camouflage.test.mjs`，非零退出）；
- 部署走既有优雅不下线 SOP（wrangler versions 灰度，DEPLOY.md §更新 CF Worker）。