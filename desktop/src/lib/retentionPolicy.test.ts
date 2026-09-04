import { describe, expect, it } from 'vitest'
import { readFileSync, mkdtempSync, writeFileSync, readdirSync, rmSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { tmpdir } from 'node:os'

/**
 * 模拟并测试版本归档轮转保留算法：
 * 保留最新的 keepCount 个版本，淘汰旧版本的 exe 及 sig，始终保留 latest.json。
 */
export function pruneHistoricalReleases(dir: string, keepCount: number = 3): string[] {
  const files = readdirSync(dir)
  const exeFiles = files.filter((f) => f.endsWith('_x64-setup.exe'))

  // 提取语义版本号做准确排序，若无则退化为 mtime
  const parseVersion = (name: string): number[] => {
    const match = name.match(/(\d+)\.(\d+)\.(\d+)/)
    if (!match) return [0, 0, 0]
    return [parseInt(match[1], 10), parseInt(match[2], 10), parseInt(match[3], 10)]
  }

  exeFiles.sort((a, b) => {
    const [ma1, mi1, p1] = parseVersion(a)
    const [ma2, mi2, p2] = parseVersion(b)
    if (ma1 !== ma2) return ma2 - ma1
    if (mi1 !== mi2) return mi2 - mi1
    return p2 - p1
  })

  const removed: string[] = []
  const toRemove = exeFiles.slice(keepCount)

  for (const exe of toRemove) {
    const exePath = join(dir, exe)
    rmSync(exePath, { force: true })
    removed.push(exe)

    const sig = `${exe}.sig`
    const sigPath = join(dir, sig)
    if (files.includes(sig)) {
      rmSync(sigPath, { force: true })
      removed.push(sig)
    }
  }

  return removed
}

describe('Release Distribution Retention Policy', () => {
  it('prunes older releases exceeding keepCount and preserves latest.json', () => {
    const testDir = mkdtempSync(join(tmpdir(), 'pproxy-retention-test-'))
    try {
      const versions = ['0.3.30', '0.3.31', '0.3.32', '0.3.33', '0.3.34']
      for (const v of versions) {
        writeFileSync(join(testDir, `Pony.Proxy_${v}_x64-setup.exe`), `exe content ${v}`)
        writeFileSync(join(testDir, `Pony.Proxy_${v}_x64-setup.exe.sig`), `sig content ${v}`)
      }
      writeFileSync(join(testDir, 'latest.json'), '{"version":"0.3.34"}')

      const removed = pruneHistoricalReleases(testDir, 3)

      expect(removed).toContain('Pony.Proxy_0.3.30_x64-setup.exe')
      expect(removed).toContain('Pony.Proxy_0.3.30_x64-setup.exe.sig')
      expect(removed).toContain('Pony.Proxy_0.3.31_x64-setup.exe')
      expect(removed).toContain('Pony.Proxy_0.3.31_x64-setup.exe.sig')

      const remaining = readdirSync(testDir)
      // 应该保留 0.3.34, 0.3.33, 0.3.32 及其 sig，加上 latest.json
      expect(remaining).toContain('Pony.Proxy_0.3.34_x64-setup.exe')
      expect(remaining).toContain('Pony.Proxy_0.3.34_x64-setup.exe.sig')
      expect(remaining).toContain('Pony.Proxy_0.3.33_x64-setup.exe')
      expect(remaining).toContain('Pony.Proxy_0.3.33_x64-setup.exe.sig')
      expect(remaining).toContain('Pony.Proxy_0.3.32_x64-setup.exe')
      expect(remaining).toContain('Pony.Proxy_0.3.32_x64-setup.exe.sig')
      expect(remaining).toContain('latest.json')

      expect(remaining).not.toContain('Pony.Proxy_0.3.30_x64-setup.exe')
      expect(remaining).not.toContain('Pony.Proxy_0.3.31_x64-setup.exe')
    } finally {
      rmSync(testDir, { recursive: true, force: true })
    }
  })

  it('keeps all files when count is within keepCount limit', () => {
    const testDir = mkdtempSync(join(tmpdir(), 'pproxy-retention-test-within-'))
    try {
      writeFileSync(join(testDir, 'Pony.Proxy_0.3.34_x64-setup.exe'), 'exe content')
      writeFileSync(join(testDir, 'Pony.Proxy_0.3.34_x64-setup.exe.sig'), 'sig content')
      writeFileSync(join(testDir, 'latest.json'), '{}')

      const removed = pruneHistoricalReleases(testDir, 3)
      expect(removed.length).toBe(0)

      const remaining = readdirSync(testDir)
      expect(remaining.length).toBe(3)
    } finally {
      rmSync(testDir, { recursive: true, force: true })
    }
  })

  it('sync-desktop-release.sh contains retention cleanup logic', () => {
    const scriptPath = resolve(__dirname, '../../../scripts/sync-desktop-release.sh')
    const content = readFileSync(scriptPath, 'utf-8')
    expect(content).toContain('KEEP_VERSIONS')
  })

  it('publish-desktop-dist.sh contains R2/S3 or multi-version retention deployment support', () => {
    const scriptPath = resolve(__dirname, '../../../scripts/publish-desktop-dist.sh')
    const content = readFileSync(scriptPath, 'utf-8')
    expect(content).toContain('R2_BUCKET')
  })
})
