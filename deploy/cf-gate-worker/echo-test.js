export default {
  async fetch(request, env, ctx) {
    if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') {
      return new Response('need ws', { status: 400 })
    }
    const pair = new WebSocketPair()
    // 注意：本 workerd 版本禁止 accept() 后再返回 Response——返回即自动接手
    const server = pair[1]
    ctx.acceptWebSocket(server)
    server.addEventListener('message', (e) => {
      console.log('[echo] got:', typeof e.data)
      server.send('echo:' + String(e.data).slice(0, 50))
    })
    return new Response(null, { status: 101, webSocket: server })
  },
}
