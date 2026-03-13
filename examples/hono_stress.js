/**
 * k6 stress test for hono_simple.ts (localhost:8888)
 *
 * Goal: detect memory leaks by:
 *   1. Ramping up load (warm-up → sustained → spike → cool-down)
 *   2. Checking that p99 latency stays flat over time (a rising trend
 *      under constant load is the primary indicator of a leak)
 *   3. Verifying all responses are correct (wrong body = ARC bug)
 *
 * Run:
 *   # Start the server first:
 *   cargo run -p tscc -- examples/hono_simple.ts -o /tmp/hono_simple && /tmp/hono_simple &
 *
 *   # Basic stress run (outputs summary + trend data):
 *   k6 run examples/hono_stress.js
 *
 *   # With real-time Grafana/InfluxDB output:
 *   k6 run --out influxdb=http://localhost:8086/k6 examples/hono_stress.js
 *
 *   # Quick smoke test (1 VU, 10s):
 *   k6 run --vus 1 --duration 10s examples/hono_stress.js
 */

import http from "k6/http";
import { check, group, sleep } from "k6";
import { Counter, Rate, Trend } from "k6/metrics";

// ── Custom metrics ────────────────────────────────────────────────────────────

/** Tracks per-route latency so we can spot divergence between routes. */
const routeLatency = {
  root:    new Trend("latency_root",    true),
  hello:   new Trend("latency_hello",   true),
  urlTest: new Trend("latency_url_test",true),
  echo:    new Trend("latency_echo",    true),
  headers: new Trend("latency_headers", true),
  notFound:new Trend("latency_404",     true),
};

/** Count responses by type to catch silent error escalation. */
const errorRate   = new Rate("error_rate");
const wrongBody   = new Counter("wrong_body_count");

// ── Load profile ──────────────────────────────────────────────────────────────
//
// Memory leaks show up as latency drift under sustained load.
// The profile below holds 20 VUs for 2 minutes after warm-up —
// long enough to see a trend without needing hours.
//
// Stages:
//   0→10 VUs over 15s   ramp up
//   10   VUs for  45s   warm-up baseline (p99 should stabilise here)
//   10→30 VUs over 30s  spike
//   30   VUs for 120s   sustained peak  ← memory leak window
//   30→5  VUs over 15s  ramp down
//   5    VUs for  30s   cool-down (GC pressure test)
//   5→0  VUs over  5s   done

export const options = {
  stages: [
    { duration: "15s", target: 10  },
    { duration: "45s", target: 10  },
    { duration: "30s", target: 30  },
    { duration: "120s", target: 30 },
    { duration: "15s", target: 5   },
    { duration: "30s", target: 5   },
    { duration: "5s",  target: 0   },
  ],
  thresholds: {
    // Overall correctness
    http_req_failed:    ["rate<0.01"],   // <1% network/5xx errors
    error_rate:         ["rate<0.01"],
    wrong_body_count:   ["count<1"],     // zero wrong responses allowed

    // Latency must stay reasonable under peak load.
    // A memory leak typically pushes p99 above 500ms before OOM.
    http_req_duration:  ["p(99)<500", "p(95)<200"],
    latency_root:       ["p(99)<500"],
    latency_hello:      ["p(99)<500"],
    latency_url_test:   ["p(99)<500"],
    latency_echo:       ["p(99)<500"],
    latency_headers:    ["p(99)<500"],
    latency_404:        ["p(99)<500"],
  },
};

const BASE = "http://localhost:8888";

// ── Helper ────────────────────────────────────────────────────────────────────

function assertBody(res, expected, label) {
  const ok = res.body === expected;
  if (!ok) {
    wrongBody.add(1);
    console.error(`[${label}] expected: ${JSON.stringify(expected)}, got: ${JSON.stringify(res.body)}`);
  }
  return ok;
}

function assertBodyContains(res, substr, label) {
  const ok = res.body && res.body.includes(substr);
  if (!ok) {
    wrongBody.add(1);
    console.error(`[${label}] expected body to contain ${JSON.stringify(substr)}, got: ${JSON.stringify(res.body)}`);
  }
  return ok;
}

// ── Main scenario ─────────────────────────────────────────────────────────────

export default function () {
  // ── GET / ──────────────────────────────────────────────────────────────────
  group("GET /", () => {
    const res = http.get(`${BASE}/`);
    routeLatency.root.add(res.timings.duration);
    const ok = check(res, {
      "status 200": (r) => r.status === 200,
      "body correct": (r) => r.body === "Hello from native Hono!",
    });
    errorRate.add(!ok);
    assertBody(res, "Hello from native Hono!", "GET /");
  });

  // ── GET /hello/:name ───────────────────────────────────────────────────────
  group("GET /hello/:name", () => {
    const name = `user${Math.floor(Math.random() * 1000)}`;
    const res = http.get(`${BASE}/hello/${name}`);
    routeLatency.hello.add(res.timings.duration);
    const ok = check(res, {
      "status 200": (r) => r.status === 200,
      "body contains name": (r) => r.body && r.body.includes(name),
      "body contains pathname": (r) => r.body && r.body.includes(`/hello/${name}`),
    });
    errorRate.add(!ok);
    assertBodyContains(res, `Hello, ${name}!`, "GET /hello/:name");
  });

  // ── GET /url-test ──────────────────────────────────────────────────────────
  group("GET /url-test", () => {
    const fooVal = `val${Math.floor(Math.random() * 500)}`;
    const res = http.get(`${BASE}/url-test?foo=${fooVal}`);
    routeLatency.urlTest.add(res.timings.duration);
    const ok = check(res, {
      "status 200": (r) => r.status === 200,
      "is JSON": (r) => { try { JSON.parse(r.body); return true; } catch { return false; } },
      "pathname correct": (r) => { try { return JSON.parse(r.body).pathname === "/url-test"; } catch { return false; } },
      "query_foo correct": (r) => { try { return JSON.parse(r.body).query_foo === fooVal; } catch { return false; } },
    });
    errorRate.add(!ok);
    if (res.status === 200) {
      try {
        const parsed = JSON.parse(res.body);
        if (parsed.query_foo !== fooVal) {
          wrongBody.add(1);
          console.error(`[GET /url-test] query_foo mismatch: expected ${fooVal}, got ${parsed.query_foo}`);
        }
      } catch (e) {
        wrongBody.add(1);
      }
    }
  });

  // ── POST /echo ─────────────────────────────────────────────────────────────
  group("POST /echo", () => {
    // Use varying body sizes to stress the body-reading path (ARC for strings).
    const bodyLen = 10 + Math.floor(Math.random() * 990); // 10–1000 bytes
    const payload = "x".repeat(bodyLen);
    const res = http.post(`${BASE}/echo`, payload, {
      headers: { "Content-Type": "text/plain" },
    });
    routeLatency.echo.add(res.timings.duration);
    const expected = `Echo: ${payload}`;
    const ok = check(res, {
      "status 200": (r) => r.status === 200,
      "echo body correct": (r) => r.body === expected,
    });
    errorRate.add(!ok);
    assertBody(res, expected, "POST /echo");
  });

  // ── GET /headers ───────────────────────────────────────────────────────────
  group("GET /headers", () => {
    const accept = "text/html,application/json";
    const res = http.get(`${BASE}/headers`, {
      headers: { Accept: accept },
    });
    routeLatency.headers.add(res.timings.duration);
    const expected = `Accept: ${accept}`;
    const ok = check(res, {
      "status 200": (r) => r.status === 200,
      "accept header echoed": (r) => r.body === expected,
    });
    errorRate.add(!ok);
    assertBody(res, expected, "GET /headers");
  });

  // ── GET /nonexistent (404) ─────────────────────────────────────────────────
  group("GET /404", () => {
    // Mark 404 as expected so it doesn't count against http_req_failed.
    const res = http.get(`${BASE}/does-not-exist-${Math.random()}`, {
      responseCallback: http.expectedStatuses(404),
    });
    routeLatency.notFound.add(res.timings.duration);
    const ok = check(res, {
      "status 404": (r) => r.status === 404,
      "body is Not Found": (r) => r.body === "Not Found",
    });
    errorRate.add(!ok);
  });

  // Small pause to avoid thundering-herd on a single VU loop
  sleep(0.05);
}

// ── Teardown: print a leak-detection summary ──────────────────────────────────
//
// k6 doesn't expose time-bucketed metrics in teardown, but the built-in
// --out csv or Grafana dashboards let you plot latency_* over time and
// see whether p99 drifts upward during the sustained-30-VU window.
//
// To check for a leak manually after the run:
//   grep "latency_root" results.csv | awk -F, '{print $2, $3}' | sort -n
//
export function teardown() {
  console.log("=== Stress test complete ===");
  console.log("To detect memory leaks, graph latency_* metrics over time.");
  console.log("A rising p99 trend during the sustained peak stage (120s at 30 VUs)");
  console.log("indicates heap growth. If p99 is flat, ARC is working correctly.");
}
