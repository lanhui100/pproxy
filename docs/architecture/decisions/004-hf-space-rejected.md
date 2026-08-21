# ADR-004: 放弃 HF Space 出口方案

- 状态: 已接受 | 日期: 2026-08-21

## 背景

HF Space（Docker SDK，免费 cpu-basic）原计划作为 OpenAI/zen 的备用出口（AWS us-east IP）。部署时 HF API 返回 402：

> Static Spaces are free for everyone, but hosting Gradio and Docker Spaces on free cpu-basic requires a PRO subscription.

## 决策

放弃 HF Space 免费方案（PRO $9/月不符合零成本约束），改用 Vercel Function（见 ADR-002）。

## 后果

- ✅ Vercel 方案落地且验证通过
- 📁 `deploy/hf-space/` 保留为参考（main.py 已重写为可用的流式转发实现，若未来购买 PRO 可直接启用）
- 部署脚本 `deploy/hf-space/deploy.py`（经 Worker 网关调 HF API，绕过 GFW 的 NDJSON commit 方案）保留备用
