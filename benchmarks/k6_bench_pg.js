// k6 load test — AOT native compiler vs Node.js benchmark
//
// Usage:
//   k6 run --env TARGET=http://localhost:17001 benchmarks/k6_bench_pg.js   # AOT
//   k6 run --env TARGET=http://localhost:17002 benchmarks/k6_bench_pg.js   # Node.js
//
// Or via bench_pg.sh which runs both automatically.

import http from 'k6/http';
import { check, sleep } from 'k6';
import { Trend } from 'k6/metrics';

const BASE = __ENV.TARGET || 'http://localhost:17001';

const dbLatency    = new Trend('db_query_latency',    true);
const usersLatency = new Trend('users_query_latency', true);
const helloLatency = new Trend('hello_latency',       true);
const userLatency  = new Trend('user_by_id_latency',  true);

export const options = {
  scenarios: {
    load: {
      executor: 'ramping-vus',
      startVUs: 0,
      stages: [
        { duration: '10s', target: 20 },   // ramp up
        { duration: '30s', target: 50 },   // sustained load
        { duration: '10s', target: 100 },  // spike
        { duration: '10s', target: 50 },   // recover
        { duration: '10s', target: 0 },    // ramp down
      ],
      gracefulRampDown: '5s',
    },
  },
  thresholds: {
    http_req_duration:   ['p(99)<1000'],
    db_query_latency:    ['p(99)<800'],
    users_query_latency: ['p(99)<800'],
  },
};

export default function () {
  const userId = Math.floor(Math.random() * 1000) + 1;

  // GET / — plain text, no DB
  {
    const r = http.get(`${BASE}/`);
    check(r, { 'hello 200': (res) => res.status === 200 });
    helloLatency.add(r.timings.duration);
  }

  // GET /db — SELECT NOW() + version()
  {
    const r = http.get(`${BASE}/db`);
    check(r, {
      'db 200':     (res) => res.status === 200,
      'db has now': (res) => res.json('now') !== null,
    });
    dbLatency.add(r.timings.duration);
  }

  // GET /users — SELECT 20 rows
  {
    const r = http.get(`${BASE}/users`);
    check(r, {
      'users 200':      (res) => res.status === 200,
      'users is array': (res) => Array.isArray(res.json()),
    });
    usersLatency.add(r.timings.duration);
  }

  // GET /users/:id — point lookup
  {
    const r = http.get(`${BASE}/users/${userId}`);
    check(r, { 'user 200': (res) => res.status === 200 });
    userLatency.add(r.timings.duration);
  }

  sleep(0.01);
}
