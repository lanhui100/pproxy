import { createGateServer } from './api/index.js'

const port = process.env.PORT || 3000
const server = createGateServer()

server.listen(port, () => {
  console.log(`Pony Gate Node/Vercel server listening on port ${port}`)
})
