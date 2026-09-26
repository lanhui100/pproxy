# Agent Note: 桌面端初始化接入页面极简化与全屏展示改造

Status: implemented

## Decision

1. **未配置时隐藏侧边栏并全屏居中展示**：
   - 抽出 `useAppConfig.ts` 集中管理 `isConfigured` 状态；
   - `App.vue` 侧边栏通过 `v-if="isConfigured"` 条件渲染，未配置时主区域自适应为全屏居中展示。
2. **Dashboard 初始化接入页面极简化**：
   - 去除“快速接入”徽标、去除“输入接入令牌，立即开启专属智能加速”副标题；
   - 去除输入框右上方的“支持 pony-gate:// 口令或授权码”辅助文案；
   - 输入框左上方由“接入令牌”改为“粘贴令牌”；
   - 输入框 placeholder 由“粘贴接入令牌 / 授权码”改为“usr_live_***”以更直观地引导用户；
   - 接入提交按钮文案由“立即开启加速”统一精简为“立即接入”。

## Alternatives considered

- **保留侧边栏置灰/禁用不可点击**：仍会占据左侧屏幕空间并分散用户注意力，不如完全全屏沉浸式完成首步配置，接入完成后自动展示侧边栏进入主仪表盘。
- **沿用长提示文本（如 支持 pony-gate...）**：桌面端面对普通租户用户，长技术名词提示会增加认知负荷，直接使用“usr_live_***”作为 placeholder 既保持了对常见令牌前缀的指引，又实现了界面极简化。

## Consequences

- 桌面端首次启动/未配置令牌时的向导页面呈现极简风格，直观易用。
- 配套 Vitest 单测（`desktop/src/lib/uiSpec.test.ts`）增加相关规范校验并持续守卫。
- 机械验证命令：`cd desktop && npm test && npm run build`（退出码 0）。
