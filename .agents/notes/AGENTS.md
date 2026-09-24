# AGENTS.md —— 决策记录区工作指引（自动加载）

> 进 `.agents/notes/` 子树即加载本文。规则正文在 `README.md`，这里只放每次必做的动作。

## 每次必做

1. **写新 ADR 前先检索**：按主题词/模块路径/关键符号 `grep -r` 扫活动树
   （proposed/implemented/rejected；archived 只读史不判）——命中则读全文、链入、
   判 supersession（程序见 write-adr 写前检索）。
2. **Supersession 不 deferred**：被取代的旧条同提交归档或链入；多阶段提案部分落地就拆条。
3. **读触发**：评审变更（change-review）、找简化/归档、治理审查、接手陌生模块时，
   先读相关 implemented 再动手（读触发规则见 README.md）。

## 禁区

- `archived/` 冻结：永不编辑、不作现行权威（细则见 `archived/AGENTS.md`）；
- 状态轴失真零容忍：未实施的不进 implemented，标题时态与目录一致（判据 1.8）。
