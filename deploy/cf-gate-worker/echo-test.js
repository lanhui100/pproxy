export default {
  async fetch(request, env, ctx) {
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') {
      return new Response('need ws', { status: 400 })
    }
    const pair = new WebSocketPair()
    // 注意：本 workerd 版本禁止 accept() 后再返回 Response——返回即自动接手
    const server = pair[1]
    const keys = []
    let o = ctx
    while (o && o !== Object.prototype) { keys.push(...Object.getOwnPropertyNames(o)); o = Object.getPrototypeOf(o) }
    server.accept()
    server.send('ctx-keys:' + JSON.stringify([...new Set(keys)]))
    server.addEventListener('message', (e) => server.send('echo:' + String(e.data).slice(0, 40)))
    server.addEventListener('message', (e) => {
      console.log('[echo] got:', typeof e.data)
      server.send('echo:' + String(e.data).slice(0, 50))
    })
    return new Response(null, { status: 101, webSocket: server })
  },
}
