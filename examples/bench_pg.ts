// AOT-compiled HTTP + PostgreSQL benchmark server.
// Routes: GET /  GET /health  GET /db  GET /users  POST /echo
//
// Usage:
//   cargo run -p tscc -- examples/bench_pg.ts
//   ./examples/bench_pg.exe
//
// Environment variables (with defaults):
//   PGHOST=localhost  PGPORT=5432  PGUSER=bench  PGPASSWORD=bench  PGDATABASE=bench
//   PORT=17001

import { Pool } from './pg_client';

const port = 17001;
const pgHost = 'localhost';
const pgPort = 5432;
const pgUser = 'bench';
const pgPassword = 'bench';
const pgDatabase = 'bench';

const pool = new Pool({
  host: pgHost,
  port: pgPort,
  user: pgUser,
  password: pgPassword,
  database: pgDatabase,
  max: 10,
});

// Simple router
const routes: { method: string; path: string; handler: (req: Request) => Promise<Response> }[] = [];

function get(path: string, h: (req: Request) => Promise<Response>) { routes.push({ method: 'GET', path, handler: h }); }
function post(path: string, h: (req: Request) => Promise<Response>) { routes.push({ method: 'POST', path, handler: h }); }

get('/', async (_req: Request) => {
  return new Response('Hello from AOT native compiler!', {
    status: 200,
    headers: { 'Content-Type': 'text/plain' },
  });
});

get('/health', async (_req: Request) => {
  return new Response(JSON.stringify({ status: 'ok', runtime: 'native' }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
});

get('/db', async (_req: Request) => {
  const result = await pool.query('SELECT NOW() AS now, version() AS version');
  const row = result.rows[0];
  return new Response(JSON.stringify({ now: row['now'], version: row['version'] }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
});

get('/users', async (_req: Request) => {
  const result = await pool.query('SELECT id, name, email FROM users ORDER BY id LIMIT 20');
  return new Response(JSON.stringify(result.rows), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
});

get('/users/:id', async (req: Request) => {
  const url = new URL(req.url);
  const parts = url.pathname.split('/');
  const id = parts[2] || '1';
  const result = await pool.query('SELECT id, name, email FROM users WHERE id = ' + id);
  if (result.rows.length === 0) {
    return new Response(JSON.stringify({ error: 'not found' }), {
      status: 404,
      headers: { 'Content-Type': 'application/json' },
    });
  }
  return new Response(JSON.stringify(result.rows[0]), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
});

post('/echo', async (req: Request) => {
  const body = await req.text();
  return new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'text/plain' },
  });
});

async function main() {
  await pool.connect();
  console.log('PostgreSQL pool connected (' + pgHost + ':' + pgPort + '/' + pgDatabase + ')');

  serve(port, async (req: Request) => {
    const url = new URL(req.url);
    const pathname = url.pathname;
    const method = req.method || 'GET';

    for (const route of routes) {
      if (route.method !== method) continue;
      if (route.path === pathname) return await route.handler(req);
      // Simple :param matching
      const rp = route.path.split('/');
      const pp = pathname.split('/');
      if (rp.length !== pp.length) continue;
      let match = true;
      for (let i = 0; i < rp.length; i++) {
        if (!rp[i].startsWith(':') && rp[i] !== pp[i]) { match = false; break; }
      }
      if (match) return await route.handler(req);
    }

    return new Response('Not Found', { status: 404 });
  });
}

main();
