# Agent Note: 桌面端 Toast 去除边框并增强毛玻璃透明效果

Status: implemented

## Problem

桌面端 Toast 原先带有微边框（`border border-neutral-300/50 dark:border-neutral-700/50`），且透明度偏低，在界面上略显生硬，缺乏通透的毛玻璃浮层质感。用户明确要求去除边框并增加毛玻璃透明效果。

## Decision

1. **移除边框并微调透明度**：
   - 在 `desktop/src/components/common/ToastHost.vue` 中将边框调整为 `border-0`；
   - 背景色透明度微调为更通透的 `bg-neutral-100/70` 与 `dark:bg-neutral-900/70`，结合原有的 `backdrop-blur-2xl` 与 `shadow-lg` 强化毛玻璃浮沉层级感。
2. **测试与规范同步更新**：
   - 在 `desktop/src/lib/toastSpec.test.ts` 中添加对 `border-0` 无边框特性的断言验证。

## Alternatives considered

- **保留微弱极浅边框（如 `border-white/10`）**：用户明确要求“去除边框”，保留任何边框都不符合直观需求，利用高斯模糊（`backdrop-blur-2xl`）与适度阴影（`shadow-lg`）足以在浅色/深色模式下清晰呈现浮层边界。

## Consequences

- 桌面端 Toast 组件在浅色与深色模式下均呈现无边框且通透通灵的毛玻璃浮层效果。
- 前端测试全部通过，生产环境打包正常。
