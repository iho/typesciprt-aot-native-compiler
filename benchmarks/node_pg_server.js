'use strict';
// Node.js HTTP + PostgreSQL benchmark server — equivalent to bench_pg.ts.
// Uses built-in node:http + pg.Pool (from npm).
//
// Usage:
//   PORT=17002 node benchmarks/node_pg_server.js

const http = require('node:http');
const { Pool } = require('pg');

const PORT = parseInt(process.env.PORT || '17002', 10);

const pool = new Pool({
  host:     process.env.PGHOST     || 'localhost',
  port:     parseInt(process.env.PGPORT || '5432', 10),
  user:     process.env.PGUSER     || 'bench',
  password: process.env.PGPASSWORD || 'bench',
  database: process.env.PGDATABASE || 'bench',
  max: 10,
});

async function handler(req, res) {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  const pathname = url.pathname;
  const method = req.method;

  try {
    if (method === 'GET' && pathname === '/') {
      res.writeHead(200, { 'Content-Type': 'text/plain' });
      res.end('Hello from Node.js!');
      return;
    }

    if (method === 'GET' && pathname === '/health') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ status: 'ok', runtime: 'node' }));
      return;
    }

    if (method === 'GET' && pathname === '/db') {
      const result = await pool.query('SELECT NOW() AS now, version() AS version');
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(result.rows[0]));
      return;
    }

    if (method === 'GET' && pathname === '/users') {
      const result = await pool.query('SELECT id, name, email FROM users ORDER BY id LIMIT 20');
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(result.rows));
      return;
    }

    const userMatch = pathname.match(/^\/users\/(\d+)$/);
    if (method === 'GET' && userMatch) {
      const id = parseInt(userMatch[1], 10);
      const result = await pool.query('SELECT id, name, email FROM users WHERE id = $1', [id]);
      if (result.rows.length === 0) {
        res.writeHead(404, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: 'not found' }));
      } else {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(result.rows[0]));
      }
      return;
    }

    if (method === 'POST' && pathname === '/echo') {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const body = Buffer.concat(chunks).toString();
      res.writeHead(200, { 'Content-Type': 'text/plain' });
      res.end(body);
      return;
    }

    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('Not Found');
  } catch (err) {
    console.error(err);
    res.writeHead(500, { 'Content-Type': 'text/plain' });
    res.end('Internal Server Error');
  }
}

const server = http.createServer((req, res) => {
  handler(req, res).catch((err) => {
    console.error(err);
    res.writeHead(500).end('Error');
  });
});

server.listen(PORT, () => {
  console.log(`Node.js server listening on :${PORT}`);
});
