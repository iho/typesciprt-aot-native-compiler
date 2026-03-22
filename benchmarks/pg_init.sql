-- Benchmark schema and seed data
CREATE TABLE IF NOT EXISTS users (
  id    SERIAL PRIMARY KEY,
  name  TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE
);

INSERT INTO users (name, email)
SELECT
  'User ' || i,
  'user' || i || '@example.com'
FROM generate_series(1, 1000) AS s(i)
ON CONFLICT DO NOTHING;
