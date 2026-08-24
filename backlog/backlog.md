# 项目 Backlog

> 轻量模式（未迁移 taskctl）。事项层记录；执行级细节见对应 spec。

## 进行中

（无）

## 待办

### B001 — CF 故障恢复后重测 S1/S2 spike ⏸ blocked-by-external
- **触发**：M6 P0 spike 期间遭遇 CF 全球 PoP 部分中断（Minor Service Outage），WS 数据帧黑洞，测试数据不可信
- **动作**：监控 https://www.cloudflarestatus.com 恢复全绿后，重跑
  `node scripts/spike-tunnel.mjs wss://gate.ponyjob.top/ws <token> /files/100Mb.dat --loop 600`（S1）
  与 `--concurrency 4`（S2），结论回填 spec m6 §12 与 ADR-008
- **附带裁决**：恢复窗口内验证生产边缘 accept() 旧式与 ctx.acceptWebSocket 新式哪个可用（当前证据互相矛盾：本地 alpha 禁旧式、生产 ctx 缺失）
- **关联**：docs/product/specs/m6/README.md §0.1/§12

## 已完成

（B 编号从 B002 起递增；M6 主实现不占 backlog——执行真源为 spec m6）
