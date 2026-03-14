import { Hono } from './hono/src/hono'

const app = new Hono()

app.get('/', (c) => c.text('Hello World'))
app.get('/hello/:name', (c) => {
  const name = c.req.param('name')
  return c.text(`Hello, ${name}!`)
})

serve(8888, async (req) => {
  return app.fetch(req)
})
