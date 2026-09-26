# Agent Note: 桌面端 Toast 样式重构（深灰质感背景与最小三行高度排版）

Status: implemented

## Decision

1. **改白色半透明为深灰质感背景，提升桌面背景对比度**：
   - 之前白色透明（`bg-white/80`）在浅色应用背景上对比度发飘且可读性受干扰；
   - 将 `desktop/src/components/common/ToastHost.vue` 改为深灰质感毛玻璃背景（`bg-neutral-900/90 dark:bg-neutral-800/95`，配以微妙白边 `border border-white/10` 与 `shadow-lg`）；
   - 文字使用清晰的 `text-neutral-100` / `text-neutral-300`，使提示内容与浅色或深色主界面均具有极高的视觉辨识度。
2. **重塑 Toast 尺寸比例，保证高度至少 3 行，避免细长条**：
   - 增加最小高度门禁（`min-h-[5.25rem]`），内部文本容器设置 `min-h-[3.25rem]` 与 `leading-relaxed`；
   - 单行短文案自动居中，补充副级提示或空态高度支撑，确保无论短文案还是长堆栈报错均保持沉稳、协调的卡片体态，彻底消除“扁细长条”感；
   - 适度放宽宽度为 `w-[min(calc(100vw-3rem),21rem)]`，内边距增至 `p-3.5`，圆角调整为更为精致的 `rounded-xl`。

## Alternatives considered

- **纯黑不透明实底卡片**：缺乏细腻质感，容易显得过于生硬。采用深灰高不透明度（90%~95%）并叠加 `backdrop-blur-xl`，既保留层次美感又彻底解决了对比度问题。
- **固定写死高度（例如固定 height: 90px）**：会破坏长文本自动换行扩展的能力；使用 `min-h-[5.25rem]` 既能在短文本时保底 3 行高度，又能在长报错时自然向下伸展。

## Consequences

- 桌面端所有操作提示（连接成功、复制口令、报错提醒等）视觉对比度明显提升，清晰易读。
- 配套修改 `desktop/src/lib/toastSpec.test.ts` 测试断言，全部 119 项单元测试通过。
- 机械验证通过：`npm test && npm run build`（退出码 0）。
