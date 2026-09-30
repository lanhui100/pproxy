// camouflage 单测（B011 伪装页 + 反指纹）：
// node deploy/cf-gate-worker/camouflage.test.mjs
// 覆盖：路由判定（/debug /debug/egress /ws 其余）、伪装页 200 + nginx 欢迎内容、
// 特征串运行时拼装（源码无明文 '/ws'）。
import { WS_PATH, DEBUG_PATH, DEBUG_EGRESS_PATH, routeFor, nginxWelcomePage } from './camouflage.mjs'

let passed = 0
let failed = 0
function check(name, cond, extra = '') {
  if (cond) {
    passed++
    console.log(`  [pass] ${name}`)
  } else {
    failed++
    console.log(`  [fail] ${name} ${extra}`)
  }
}

console.log('[1] 特征串运行时拼装（源码无明文特征）')
check('/ws 由拼装得到', WS_PATH === '/ws')
check('/debug 由拼装得到', DEBUG_PATH === '/debug')
check('/debug/egress 由拼装得到', DEBUG_EGRESS_PATH === '/debug/egress')

console.log('[2] 路由判定')
check('根路径 → camouflage', routeFor('/') === 'camouflage')
check('/ws → ws', routeFor('/ws') === 'ws')
check('/debug → debug', routeFor('/debug') === 'debug')
check('/debug/egress → debugEgress', routeFor('/debug/egress') === 'debugEgress')
check('未知路径 → camouflage', routeFor('/favicon.ico') === 'camouflage')
check('任意深路径 → camouflage', routeFor('/a/b/c') === 'camouflage')
check('/ws/ 不误判', routeFor('/ws/') === 'camouflage')
check('空路径 → camouflage', routeFor('') === 'camouflage')
check('查询串由 URL.pathname 剥离（worker 侧），routeFor 只看 pathname', routeFor('/ws') === 'ws' && routeFor('/') === 'camouflage')

console.log('[3] 伪装页内容（普通站点外观，无 pproxy 特征）')
const page = nginxWelcomePage()
check('状态码 200', page.status === 200)
check('content-type text/html', (page.headers.get('content-type') || '').includes('text/html'))

const text = await page.text()
check('含 nginx 欢迎标题', text.includes('Welcome to nginx!'))
check('无 pproxy 特征', !text.includes('pproxy') && !text.includes('pony') && !text.includes('gate') && !text.includes('tunnel'))
check('无 "not found"', !text.includes('not found'))
check('是合法闭合 HTML', text.includes('</html>') && text.includes('<html>'))

console.log(`\n结果: ${passed} pass, ${failed} fail`)
process.exit(failed ? 1 : 0)