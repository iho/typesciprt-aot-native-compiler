import { Pool } from 'pg';

const PORT = parseInt(process.env.PORT || '17002', 10);

const pool = new Pool({
  host:     process.env.PGHOST     || 'localhost',
  port:     parseInt(process.env.PGPORT || '5432', 10),
  user:     process.env.PGUSER     || 'bench',
  password: process.env.PGPASSWORD || 'bench',
  database: process.env.PGDATABASE || 'bench',
  max: 10,
});

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

Bun.serve({
  port: PORT,
  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const { pathname } = url;
    const method = req.method;

    if (method === 'GET' && pathname === '/') {
      return new Response('Hello from Bun!', { headers: { 'Content-Type': 'text/plain' } });
    }

    if (method === 'GET' && pathname === '/health') {
      return json({ status: 'ok', runtime: 'bun' });
    }

    if (method === 'GET' && pathname === '/db') {
      const result = await pool.query('SELECT NOW() AS now, version() AS version');
      return json(result.rows[0]);
    }

    if (method === 'GET' && pathname === '/users') {
      const result = await pool.query('SELECT id, name, email FROM users ORDER BY id LIMIT 20');
      return json(result.rows);
    }

    const userMatch = pathname.match(/^\/users\/(\d+)$/);
    if (method === 'GET' && userMatch) {
      const id = parseInt(userMatch[1], 10);
      const result = await pool.query('SELECT id, name, email FROM users WHERE id = $1', [id]);
      if (result.rows.length === 0) return json({ error: 'not found' }, 404);
      return json(result.rows[0]);
    }

    if (method === 'POST' && pathname === '/echo') {
      const body = await req.text();
      return new Response(body, { headers: { 'Content-Type': 'text/plain' } });
    }

    return new Response('Not Found', { status: 404 });
  },
  error(err: Error): Response {
    console.error(err);
    return new Response('Internal Server Error', { status: 500 });
  },
});

console.log(`Bun+pg server listening on :${PORT}`);
