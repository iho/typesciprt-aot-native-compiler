'use strict';

const express = require('express');
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

const app = express();
app.use(express.text());

app.get('/', (_req, res) => {
  res.type('text/plain').send('Hello from Node.js + Express!');
});

app.get('/health', (_req, res) => {
  res.json({ status: 'ok', runtime: 'node+express' });
});

app.get('/db', async (_req, res) => {
  const result = await pool.query('SELECT NOW() AS now, version() AS version');
  res.json(result.rows[0]);
});

app.get('/users', async (_req, res) => {
  const result = await pool.query('SELECT id, name, email FROM users ORDER BY id LIMIT 20');
  res.json(result.rows);
});

app.get('/users/:id', async (req, res) => {
  const id = parseInt(req.params.id, 10);
  const result = await pool.query('SELECT id, name, email FROM users WHERE id = $1', [id]);
  if (result.rows.length === 0) {
    res.status(404).json({ error: 'not found' });
  } else {
    res.json(result.rows[0]);
  }
});

app.post('/echo', (req, res) => {
  res.type('text/plain').send(req.body);
});

// Global error handler so async errors don't crash the process
app.use((err, _req, res, _next) => {
  console.error(err);
  res.status(500).send('Internal Server Error');
});

app.listen(PORT, () => {
  console.log(`Express+pg server listening on :${PORT}`);
});
