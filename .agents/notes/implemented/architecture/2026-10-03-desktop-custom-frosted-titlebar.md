# Agent Note: desktop-custom-frosted-titlebar

Status: implemented

## Problem

桌面客户端（Tauri v2）原生系统标题栏在不同操作系统（尤其是 Windows 10/11）下样式割裂，缺乏质感，无法契合应用整体现代化的「中性浅灰质感 + 毛玻璃」设计语言。用户希望将桌面端原生窗口的顶部菜单/标题栏改为毛玻璃效果。

在 Tauri 架构下，原生系统标题栏由操作系统窗口管理器绘制，无法在原生标题栏内部直接注入 Web 层 CSS backdrop-filter 毛玻璃特效。必须开启无原生边框窗口（`decorations: false`），并在前端顶部通过自定义标题栏组件（Custom Titlebar）承载窗口拖拽、标题展示、最小化、最大化/还原以及关闭等原生窗口控制行为，同时应用半透明毛玻璃质感（`backdrop-blur`）。

## Decision

1. **窗口配置调整**：
   - 在 `desktop/src-tauri/tauri.conf.json` 中配置 `decorations: false`，移除系统原生标题栏。
   - 相应放宽 `desktop/scripts/check-invariants.sh` 中的 `tauri.conf.json` 语义比对规则，将 `decorations` 纳入允许属性。
2. **能力与权限开放**：
   - 窗口控制（最小化、最大化切换、关闭）由前端通过 `@tauri-apps/api/window` 的 `getCurrentWindow()` 调用原生命令。
   - 在 `desktop/src-tauri/capabilities/default.json` 中补齐 `core:window:allow-minimize`、`core:window:allow-toggle-maximize`、`core:window:allow-close` 权限。
   - 对应更新 `desktop/scripts/check-invariants.sh` 保证权限门禁规则自洽通过。
3. **前端自定义毛玻璃标题栏组件（`TitleBar.vue`）**：
   - 采用固定高度（38px / `h-9.5`），设置 `data-tauri-drag-region` 支持窗口拖拽。
   - 视觉采用半透明质感中性底色与高阶毛玻璃滤镜（`bg-background/80 backdrop-blur-xl border-b border-border/40`），双模下保持自适应。
   - 左侧展示应用 Logo 与名称，右侧提供最小化、最大化/还原、关闭按钮，操作区设置非拖拽保护。
   - 在 `desktop/src/App.vue` 顶层常驻渲染，整体布局适配无边框安全边距与高度。

## Alternatives considered

1. **保留原生标题栏并使用 OS 原生 Acrylic/Vibrancy 效果**：
   - 评估：Tauri v2 的 windowEffects 在 Windows 下对 decorated window 效果有限且不同 Windows 版本（Win10 vs Win11 22H2+）存在性能回退和样式兼容性问题，且标题栏按钮颜色无法精细定制。因此否决。
2. **仅在内容区顶部模拟内嵌头部栏**：
   - 评估：系统原生白条/深条标题栏依然保留在上层，整体割裂感强，未真正实现桌面端窗口头部的毛玻璃效果。因此否决。

## Consequences

- 桌面端主窗口拥有沉浸式现代无边框毛玻璃标题栏，视觉与质感大幅提升；
- 窗口最小化、最大化、关闭操作由前端平滑调用 Tauri window API，用户操作习惯保持一致；
- `check-invariants.sh` 持续守卫配置与权限变更。
