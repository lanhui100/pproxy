# Agent Note: 桌面端 Toast 背景调整为浅灰色质感

Status: implemented

## Problem

桌面端 Toast 原先采用深灰色毛玻璃底色（`bg-neutral-900/90`），在常规桌面浅色主题下色彩偏深偏重，视觉体量过大，不够轻盈和谐。需要将其调整为浅灰色质感底色。

## Decision

1. **亮色模式采用浅灰色毛玻璃底色并移除边框**：
   - 将 `desktop/src/components/common/ToastHost.vue` 中的卡片背景调整为浅灰色毛玻璃 `bg-neutral-100/90`（深色模式下保留 `dark:bg-neutral-800/95`）；
   - 彻底移除边框（`border-0`），依靠阴影 `shadow-lg` 与毛玻璃层次实现浮层立体感；
   - 相应调整文字色阶为 `text-neutral-800`，副文本为 `text-neutral-600`，关闭与复制按钮在亮色下适配轻黑背景 `hover:bg-black/5` 与 `hover:text-neutral-900`。
2. **测试与规范同步更新**：
   - 更新 `desktop/src/lib/toastSpec.test.ts` 中针对 Toast 背景色的断言，允许浅灰中性色匹配并同步移除边框约束描述。

## Alternatives considered

- **改为半透明纯白色（`bg-white/80`）**：纯白色在浅色暖白页面底色上对比度发飘，缺乏边界感。采用 `bg-neutral-100/90`（带 10% 透光的浅灰色中性毛玻璃）既能保持轻盈素净，又具有明确的浮层层级感。

## Consequences

- 桌面端 Toast 在浅色模式下呈现精致克制的浅灰色毛玻璃质感，不再过深过重。
- 119 项前端单测全绿，生产构建无告警通过。
