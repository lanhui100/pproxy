// 决定性诊断：全局计数器区分"连接建立"与"消息投递"断在哪一环
const stats = { conns: 0, msgs: 0, sends: 0, lastMsg: '', lastErr: '', started: new Date().toISOString() }

export default {
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname === '/stats') {
      return new Response(JSON.stringify({ ...stats, now: new Date().toISOString() }, null, 1), {
        headers: { 'content-type': 'application/json', 'cache-control': 'no-store' },
      })
    }
    if (url.pathname !== '/ws') return new Response('ok', { status: 200 })
    // TEMP 实验：不升级，直接返回计数器——验证升级形状的请求是否到达 handler
    if (!request.headers.get('Upgrade')) {
      stats.conns++
      return new Response(JSON.stringify({ reached_handler: true, conns_total: stats.conns }), {
        headers: { 'content-type': 'application/json' },
      })
    }
    stats.conns++
    const pair = new WebSocketPair()
    const server = pair[1]
    try {
      server.accept()
    } catch (e) {
      stats.lastErr = 'accept: ' + String(e)
      return new Response('accept failed', { status: 500 })
    }
    try {
      server.send('probe:conns-at-upgrade=' + stats.conns)
      stats.sends++
    } catch (e) {
      stats.lastErr = 'send-after-accept: ' + String(e)
    }
    server.addEventListener('message', (e) => {
      stats.msgs++
      stats.lastMsg = String(e.data).slice(0, 60)
      try { server.send('echo:' + String(e.data)) ; stats.sends++ } catch (err) { stats.lastErr = 'echo send: ' + String(err) }
    })
    server.addEventListener('error', (e) => { stats.lastErr = 'socket error event' })
    server.addEventListener('close', () => {})
    return new Response(null, { status: 101, webSocket: server })
  },
}
