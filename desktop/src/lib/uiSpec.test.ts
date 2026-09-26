import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

describe('UI/UX Specification Checks', () => {
  const dashboardPath = resolve(__dirname, '../views/DashboardView.vue')
  const settingsPath = resolve(__dirname, '../views/SettingsView.vue')

  it('DashboardView defaults usage dimension to 24h', () => {
    const content = readFileSync(dashboardPath, 'utf-8')
    expect(content).toContain("const usageDimension = ref<'7d' | '24h'>('24h')")
  })

  it('DashboardView standardizes egress names to 出口R, 出口C and 出口V', () => {
    const content = readFileSync(dashboardPath, 'utf-8')
    expect(content).toContain("name: '出口R'")
    expect(content).toContain("name: '出口C'")
    expect(content).toContain("name: '出口V'")
    expect(content).not.toContain("name: 'C出口'")
    expect(content).not.toContain("name: 'V出口'")
  })

  it('SettingsView aligns title, section headers and field naming', () => {
    const content = readFileSync(settingsPath, 'utf-8')
    // 标题必须是“设置”
    expect(content).toContain('text-xl font-bold tracking-tight text-foreground">设置</h1>')
    expect(content).not.toContain('设置中心')

    // 加速模式 & 去除方案A/B
    expect(content).toContain('加速模式')
    expect(content).not.toContain('加速出网方案')
    expect(content).not.toContain('方案 A')
    expect(content).not.toContain('方案 B')

    // 字段精简化（设置页极净化改造：隧道由系统自动获取，不再需要单独展示与修改隧道端点）
    expect(content).toContain('接入令牌')
    expect(content).not.toContain('连接口令 / 加速授权码')
    expect(content).not.toContain('修改隧道端点')
    expect(content).toContain('同步口令')
    expect(content).not.toContain('口令一键导入 (多端同步)')
    expect(content).toContain('服务器配置')
    expect(content).not.toContain('手动配置服务器参数')
    expect(content).toContain('加速名单')
    expect(content).not.toContain('域名名单')
    expect(content).not.toContain('智能分流加速名单')
    expect(content).toContain('反代令牌')
    expect(content).not.toContain('反代访问令牌')

    // 加速名单徽标直接罗列，去长方形卡片背景
    expect(content).toMatch(/<h2 class="text-sm font-bold text-foreground">加速名单<\/h2>[\s\S]*?class="flex flex-wrap gap-2/)

    // 去卡片化
    expect(content).not.toContain('<Card')
    expect(content).not.toContain('CardTitle')
    expect(content).not.toContain('CardHeader')

    // 文本可选（无全局 select-none）
    expect(content).not.toContain('class="space-y-6 select-none"')

    // 安全性：空密码不发送
    expect(content).toContain('if (editRemotePass.value.trim())')
    expect(content).toContain('chainedConfig.password = editRemotePass.value.trim()')
    expect(content).toContain('if (remotePass.value.trim())')
  })

  it('DashboardView implements debounce lock on proxy toggle', () => {
    const content = readFileSync(dashboardPath, 'utf-8')
    expect(content).toContain('const isToggling = ref(false)')
    expect(content).toContain(':disabled="isToggling"')
  })

  it('DashboardView guards site probing until proxy engine is running', () => {
    const content = readFileSync(dashboardPath, 'utf-8')
    // 站点拨测必须检查 isRunning 状态，未开启代理时不盲测
    expect(content).toContain('if (!isRunning.value)')
    // 代理就绪后联动触发测速
    expect(content).toMatch(/!wasRunning && event\.payload\.on[\s\S]*?runAllTests/)
  })
})

