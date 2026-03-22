# Benchmarks

Two benchmark suites comparing the TypeScript AOT native compiler against Node.js and Bun.

---

## Benchmark 1: In-Memory REST API (no DB)

Compares AOT vs Node.js for a Vendure-like REST API serving in-memory data — no database, pure compute + JSON.

**Date**: 2026-03-20 | **Tool**: wrk | **Settings**: 10s, 50 connections, 4 threads

| Endpoint | AOT req/s | Node.js req/s | Speedup |
|----------|-----------|---------------|---------|
| `GET /health` | 80,980 | 45,335 | **1.79×** |
| `GET /api/products` | 68,167 | 38,553 | **1.77×** |
| `GET /api/products/1` | 78,688 | 43,873 | **1.79×** |
| `GET /api/orders` | 67,382 | 46,477 | **1.45×** |

| Endpoint | AOT latency | Node.js latency |
|----------|-------------|-----------------|
| `GET /health` | 646 µs | 1.09 ms |
| `GET /api/products` | 1.04 ms | 1.26 ms |
| `GET /api/products/1` | 635 µs | 1.10 ms |
| `GET /api/orders` | 704 µs | 1.04 ms |

**AOT is ~1.5–1.8× faster than Node.js** on pure compute/JSON workloads (no database I/O).

Run: `bash benchmarks/bench.sh`

---

## Benchmark 2: PostgreSQL REST API (AOT vs Node.js+Express vs Bun)

Compares all three runtimes serving HTTP endpoints backed by a live PostgreSQL database.
Uses a native PostgreSQL wire protocol client (`pg_client.ts`) compiled AOT,
versus Node.js with `express`+`pg`, and Bun with `pg`.

**Date**: 2026-03-22 | **Tool**: k6 | **Machine**: Apple Silicon (macOS 24.6.0)

### k6 Load Profile

```
Ramp 0→20 VUs (10s) → hold 50 VUs (30s) → spike 100 VUs (10s) → recover 50 (10s) → ramp down (10s)
```

Peak 100 VUs, ~70s total. Each VU iteration hits: `GET /`, `/db`, `/users`, `/users/:id`.
Pool size: 10 connections per runtime.

### Throughput

| Runtime | Requests/s | Iterations | Check pass rate |
|---------|-----------|------------|-----------------|
| AOT native | 2,720 | 47,617 | 100% |
| Node.js + Express | 7,831 | 137,058 | 100% |
| **Bun** | **11,761** | **205,839** | **100%** |

Bun is **4.3× faster** than AOT and **1.5× faster** than Node.js in throughput.

### Latency — All Endpoints Combined (ms, lower is better)

| Runtime | avg | p90 | p95 |
|---------|-----|-----|-----|
| AOT native | 12.62 | 12.09 | 17.18 |
| Node.js + Express | 2.71 | 7.24 | 8.83 |
| Bun | **0.94** | **2.06** | **2.60** |

### Latency by Endpoint (ms)

| Endpoint | AOT avg | AOT p90 | Node avg | Node p90 | Bun avg | Bun p90 |
|----------|---------|---------|---------|---------|---------|---------|
| `GET /` (no DB) | 2.15 | 3.92 | 1.19 | 2.39 | **0.20** | **0.35** |
| `GET /db` (SELECT NOW) | 11.90 | 12.65 | 2.77 | 7.11 | **1.17** | **2.29** |
| `GET /users` (20 rows) | 16.50 | 13.43 | 3.51 | 8.54 | **1.22** | **2.29** |
| `GET /users/:id` (lookup) | 19.91 | 15.36 | 3.39 | 8.23 | **1.18** | **2.24** |

### Why AOT Is Slower Under DB Load

The AOT compiler implements **cooperative concurrency via a global JS execution lock** — only one thread runs TypeScript at a time, matching the semantics of a single-threaded JS event loop. This design works well for pure compute (see Benchmark 1), but causes queuing under concurrent I/O:

- With 50–100 VUs, 50+ OS threads compete for the JS lock simultaneously
- Every `await` point (each DB call) requires a lock release + re-acquire
- Each PostgreSQL query requires ~8–10 lock transitions (startup + data delivery)
- At ~100µs per transition: **~0.8–1ms added latency per query**
- Combined with PG RTT and queue depth → 10–20ms p90

The **pull-mode TCP socket** (implemented 2026-03-22) reduces this from 2 lock transitions per received chunk to 1 per `readBytes()` call. Without this optimization, DB latency was significantly worse.

For pure in-memory workloads (no I/O awaits), the lock is uncontested and AOT achieves **1.5–1.8× Node.js** (Benchmark 1).

### Future Improvements

- **Lock-free I/O delivery**: Buffer socket data in Rust without touching the JS lock
- **Parallel JS threads**: Partition the lock per-isolate for multi-core utilization
- **Optimized HTTP layer**: Reduce per-request overhead in the Hyper-based HTTP handler

### Reproduction

```bash
# Ensure Docker is running
docker compose up -d postgres

# Run the 3-way benchmark (AOT vs Node.js vs Bun)
bash benchmarks/bench_pg.sh
```

Raw JSON results written to `benchmarks/aot_result.json`, `node_result.json`, `bun_result.json`.
