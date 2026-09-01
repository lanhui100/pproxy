import server from './api/ws.js'

// standalone 模式（VPS systemd 常驻）：复用 api/ws.js 的默认导出实例。
// 环境变量：PORT（默认 3000）、TUNNEL_TOKEN_HASH（必填，fail-closed）。
const port = process.env.PORT || 3000

server.listen(port, () => {
  console.log(`Pony Gate Node/Vercel server listening on port ${port}`)
})
