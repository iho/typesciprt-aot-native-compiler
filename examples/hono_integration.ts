import { Hono } from '../hono/src/hono'

const app = new Hono()

app.get('/', (c) => c.text('Hello from Hono'))

app.get('/hello/:name', (c) => {
  const name = c.req.param('name')
  return c.text(`Hello, ${name}!`)
})

app.get('/json', (c) => c.json({ ok: true, value: 42 }))

serve(17999, async (req) => app.fetch(req))
