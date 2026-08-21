# ADR-005: 废弃免费代理池，采用静态上游

- 状态: 已接受 | 日期: 2026-08-21

## 背景

项目初期方案为动态免费代理池（jsDelivr CDN 拉取 ProxyScrape/TheSpeedX 列表 → 批量验证 → 择优转发）。实测数据：

- 命中率 0-2.7%（US 4/253）
- 代理寿命分钟级（验证通过 15 分钟后全部死亡）
- OpenAI 对全部公共代理 IP 拉黑
- 动态验证耗时与代理寿命倒挂，池长期为 0

## 决策

- 停用动态池（`config.json` 中 `countries: []` 时 pool.refresh 直接返回）
- 上游改为静态双出口（CF Worker + Vercel，见 ADR-002）
- 池代码保留（`crates/core/src/{source,check,pool}.rs`），未来可接入付费代理源

## 后果

- ✅ 稳定性从"不可用"提升到生产级
- ✅ 省去持续验证的资源消耗
- ❌ 出口依赖 CF/Vercel 两家平台政策（风险由监控告警兜底，见 ROADMAP M3）
