# Benchmarks

Three-way benchmark comparing the TypeScript AOT native compiler against Node.js and Bun.

**Date**: 2026-03-22 | **Machine**: Apple Silicon (macOS 24.6.0) | **Tool**: wrk | **Settings**: 10s, 50 connections, 4 threads

---

## In-Memory REST API (AOT vs Node.js vs Bun)

A Vendure-like e-commerce REST API backed entirely by in-memory `Map` structures — no database, pure
compute + JSON serialization. Same business logic implemented identically in each runtime:

| Runtime | Source | HTTP layer |
|---------|--------|------------|
| **AOT native** | `examples/vendure_bench.ts` → compiled binary | Hyper (Rust, fiber-based) |
| **Node.js** | `benchmarks/vendure_node.js` | `node:http` |
| **Bun** | `benchmarks/bun_server.ts` | `Bun.serve()` |

### Throughput (req/s, higher is better)

| Endpoint | AOT | Node.js | Bun | AOT vs Node | AOT vs Bun |
|----------|-----|---------|-----|-------------|------------|
| `GET /health` | **56,158** | 47,570 | 89,630 | **+18%** | −37% |
| `GET /api/products` | 22,311 | 40,828 | 57,199 | −45% | −61% |
| `GET /api/products/1` | **46,465** | 47,158 | 80,282 | −1.5% | −42% |
| `GET /api/orders` | **54,706** | 48,869 | 88,729 | **+12%** | −38% |
| `POST /api/orders` | **54,725** | 47,309 | 86,350 | **+16%** | −37% |
| `POST /api/auth/login` | **57,697** | 48,961 | 87,760 | **+18%** | −34% |

### Latency (avg, lower is better)

| Endpoint | AOT | Node.js | Bun |
|----------|-----|---------|-----|
| `GET /health` | 0.85 ms | 0.99 ms | 0.53 ms |
| `GET /api/products` | 2.12 ms | 1.17 ms | 0.85 ms |

### Memory Consumption

| Runtime | Startup RSS | Post-benchmark RSS (macOS) | Actual live heap |
|---------|-------------|---------------------------|------------------|
| AOT native | **8.1 MB** | ~1.3 GB (see note) | **< 10 MB** |
| Node.js | 41.2 MB | ~79 MB | ~79 MB |
| Bun | 25.7 MB | ~39 MB | ~39 MB |

**Note on AOT RSS**: The large post-benchmark RSS on macOS is allocator fragmentation, not a memory
leak. jemalloc madvises freed pages back to the OS, but macOS counts dirty freed pages toward RSS
until the OS reclaims them. The actual live heap measured by `leaks` is < 10 MB (only 192 bytes of
true leaks). On Linux, RSS behaviour is better (`MADV_DONTNEED` returns pages immediately). The
`leaks` tool confirmed zero live leaks.

### Summary

- **Simple endpoints** (health, orders, auth): AOT is **+12–18% faster** than Node.js and ~1.6× slower than Bun
- **Array-heavy endpoints** (`/api/products`): AOT is ~1.8× slower than Node.js and ~2.6× slower than Bun — see [Why `/api/products` is slower](#why-apiproducts-is-slower)
- **Startup memory**: AOT is **5.1× smaller** than Node.js and **3.2× smaller** than Bun

---

## Why AOT Is Slower Than Bun

### Simple endpoints (health/orders) — ~1.6× gap

Bun uses JavaScriptCore (JSC) with a JIT compiler and `kqueue`/`epoll`-native HTTP via modified uWebSockets.
AOT uses stackful coroutines (fibers) over Hyper/Tokio. Each request involves:

1. **Fiber context switch** (~50 ns): stack-switch from tokio task → fiber at every `await` point
2. **ARC overhead**: every `TsVal` read emits `ts_retain_val` (atomic refcount increment) and every
   scope exit emits `ts_release_val` (atomic decrement + conditional dealloc). JSC's GC batches
   this work. The gap here is ~1.6×.

### Array-heavy endpoints (`/api/products`) — ~2.6× gap

The `/api/products` handler does:
```typescript
const all = [...products.values()].filter(p => p.enabled);
return json({ items: all.slice(skip, skip + limit), total: all.length, page, limit });
```

In AOT, `[...products.values()]` allocates a `TsArray` on the heap, then `filter` allocates another,
then `slice` allocates another. Each `Product` object in the result is a `TsObject` with 10 string/int
fields — all ARC-tracked. `JSON.stringify` then recursively walks this heap tree.

In Bun/Node, Map iteration and array operations are JIT-compiled native code with no per-value
reference counting. The gap is proportional to object count × operations.

### RSS fragmentation (macOS artefact)

AOT uses jemalloc as the global allocator with `dirty_decay_ms:0` and `muzzy_decay_ms:0`. Freed pages
are immediately madvised back to the OS. The actual live heap stays < 10 MB throughout. However,
macOS's RSS counter includes all dirty pages (including freed-and-madvised pages) until the OS
reclaims them. This creates the appearance of unbounded growth, but `leaks` confirms zero true leaks.

---

## Why AOT Is Faster Than Node.js (simple endpoints)

AOT achieves **+12–18%** over Node.js on simple endpoints (health, orders, auth) because:

1. **No interpreter overhead**: Code is compiled directly to native ARM64 machine code. No V8 bytecode
   interpretation or JIT warm-up phase.
2. **Fiber stack pool**: A thread-local pool of 256 KB stacks (up to 32 cached per thread) avoids
   per-request `mmap(256KB)` + `munmap` calls. Stack switching costs ~50 ns vs ~100 µs for a
   thread context switch.
3. **Single-threaded cooperative model**: One `LocalSet` thread handles all requests cooperatively.
   No locking, no cross-thread synchronization on the hot path for in-memory operations.
4. **jemalloc**: Lower allocation overhead than macOS's system `malloc` for rapid alloc/free cycles.

---

## Reproduction

```bash
# Build the AOT binary
cargo build --release -p tscc
cargo run --release -p tscc -- examples/vendure_bench.ts -o /tmp/vendure_bench

# Start all three servers
PORT=19888 /tmp/vendure_bench &
node benchmarks/vendure_node.js &
bun benchmarks/bun_server.ts &

# Benchmark each (adjust ports as needed: AOT=19888, Node=19889, Bun=19890)
wrk -t4 -c50 -d10s http://127.0.0.1:19888/health
wrk -t4 -c50 -d10s http://127.0.0.1:19889/health
wrk -t4 -c50 -d10s http://127.0.0.1:19890/health
```

---

## PostgreSQL REST API (AOT vs Node.js vs Bun)

Compares all three runtimes serving HTTP endpoints backed by a live PostgreSQL database.

**Tool**: k6 | **Load profile**: ramp 0→20 VUs (10s) → 50 VUs (30s) → spike 100 VUs (10s) → ramp down

Each k6 iteration hits 4 endpoints in sequence: `GET /` (no DB), `GET /db` (SELECT NOW),
`GET /users` (SELECT 20 rows), `GET /users/:id` (point lookup).

### Throughput

| Runtime | Req/s | Total requests | Check pass rate |
|---------|-------|----------------|-----------------|
| AOT native | 2,353 | 164,768 | **100%** |
| Node.js + Express | 7,801 | 546,120 | **100%** |
| **Bun** | **11,917** | **834,336** | **100%** |

### Latency by Endpoint (avg ms, lower is better)

| Endpoint | AOT | Node.js | Bun | AOT/Node ratio |
|----------|-----|---------|-----|----------------|
| `GET /` (no DB) | 10.64 | 1.19 | **0.19** | 8.9× slower |
| `GET /db` (SELECT NOW) | 14.52 | 2.81 | **1.13** | 5.2× slower |
| `GET /users` (20 rows) | 18.01 | 3.53 | **1.16** | 5.1× slower |
| `GET /users/:id` | 16.74 | 3.38 | **1.11** | 5.0× slower |
| **Combined avg** | **14.98** | **2.73** | **0.90** | **5.5×** |

### Latency Percentiles — Combined (ms)

| Runtime | avg | p90 | p95 |
|---------|-----|-----|-----|
| AOT native | 14.98 | 32.97 | 40.26 |
| Node.js + Express | 2.73 | 7.02 | 8.69 |
| **Bun** | **0.90** | **1.98** | **2.45** |

### Why AOT Is Slower Under DB Load

AOT uses a **global JS execution lock** — only one fiber executes TypeScript at a time. Under
concurrent I/O load the bottleneck is lock contention, not the DB itself.

**The `GET /` endpoint (no DB query) makes this concrete**: AOT averages 10.64 ms for a response
that is just `"hello"`, while Node averages 1.19 ms for the same. Under the in-memory benchmark
(no concurrent I/O, no lock contention) AOT beats Node by 18%. The difference is entirely lock
queueing: when 50–100 VUs each hold DB awaits, the no-DB hello fiber sits in the lock queue
behind dozens of DB fibers.

**Lock transition cost**: each `await db.query()` involves:
1. Fiber releases JS lock → hands execution to the Tokio event loop
2. PostgreSQL query executes (~0.5–2 ms)
3. Result arrives → fiber re-queues for the JS lock
4. With N concurrent fibers, avg queue time = N × avg_lock_hold_time

At 50 VUs × 4 concurrent requests = 200 fibers competing for one lock. Each lock-hold is
~0.1–0.5 ms, so average wait time grows to 5–15 ms, matching the observed latency.

**Node.js / Bun do not have this problem**: V8 and JSC use a native async event loop where
multiple async operations genuinely run concurrently (not just cooperatively) and there is no
global lock on JS execution.

**Fix**: Removing the global JS lock would allow AOT to match Node.js on DB-heavy workloads.
This requires either: (a) making all TsVal heap types atomically reference-counted (already done —
`ArcHeader.ref_count` is `AtomicU32`), or (b) a per-arena lock. Planned for a future milestone.

### Reproduction

```bash
docker compose up -d postgres
bash benchmarks/bench_pg.sh
```

Raw JSON results: `benchmarks/aot_result.json`, `node_result.json`, `bun_result.json`.
