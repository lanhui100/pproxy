// gate 伪装页 + 反指纹（B011，edgetunnel 借鉴，负向清单除外）。
// 纯函数模块：worker.js 只接线；node 可直测（camouflage.test.mjs）。
//
// 动机：非 /ws 裸 404 是"可疑 Worker"最高权重指纹；伪装成普通站点降低扫号命中。
// 做法：
//   1. 关键路径/文案**运行时拼装**（不落明文），降低对 worker 源码正则查杀的命中；
//   2. 非隧道路径返回 200 仿 nginx 欢迎页（普通站点外观）；
//   3. 不抄 edgetunnel 的"多语言无后门声明"注释垫片（信任争议，负向清单）。

/// 运行时拼装关键路径与文案：源码中不出现 '/ws'、'not found'、'websocket required'
/// 等明文串（静态指纹扫描按正则抓这些特征）。
export const WS_PATH = ['/', 'w', 's'].join('')
export const DEBUG_PATH = ['/', 'd', 'e', 'b', 'u', 'g'].join('')
export const DEBUG_EGRESS_PATH = ['/', 'd', 'e', 'b', 'u', 'g', '/', 'e', 'g', 'r', 'e', 's', 's'].join('')

/// 路径是否命中某个按片段拼装的路径（等价于 pathname === piecewisePath）。
function pathEquals(pathname, piecewisePath) {
  return typeof pathname === 'string' && pathname === piecewisePath
}

/// gate 路由判定：
/// @returns 'debug' | 'debugEgress' | 'ws' | 'camouflage'
export function routeFor(pathname) {
  if (pathEquals(pathname, DEBUG_PATH)) return 'debug'
  if (pathEquals(pathname, DEBUG_EGRESS_PATH)) return 'debugEgress'
  if (pathEquals(pathname, WS_PATH)) return 'ws'
  return 'camouflage'
}

/// 仿 nginx 欢迎页（200 text/html）。内容是标准 nginx 安装欢迎页外观，
/// 浏览器直开域名看到普通站点首面；不携带任何 pproxy/gate/隧道特征。
export function nginxWelcomePage() {
  const html = [
    '<!DOCTYPE html>',
    '<html>',
    '<head>',
    '<title>Welcome to nginx!</title>',
    '<style>',
    'html { color-scheme: light dark; }',
    'body { width: 35em; margin: 0 auto;',
    'font-family: Tahoma, Verdana, Arial, sans-serif; }',
    '</style>',
    '</head>',
    '<body>',
    '<h1>Welcome to nginx!</h1>',
    '<p>If you see this page, the nginx web server is successfully installed and',
    'working. Further configuration is required.</p>',
    '<p>For online documentation and support please refer to',
    '<a href="http://nginx.org/">nginx.org</a>.<br/>',
    'Commercial support is available at',
    '<a href="http://nginx.com/">nginx.com</a>.</p>',
    '<p><em>Thank you for using nginx.</em></p>',
    '</body>',
    '</html>',
  ].join('\n')
  return new Response(html, {
    status: 200,
    headers: { 'content-type': 'text/html; charset=utf-8' },
  })
}
