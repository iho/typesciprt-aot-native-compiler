# Benchmarks

Three-way benchmark comparing the TypeScript AOT native compiler against Node.js and Bun.

**Date**: 2026-03-22 | **Machine**: Apple Silicon (macOS 24.6.0) | **Tool**: wrk / k6 | **Settings**: 10s, 50 connections, 4 threads

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
| `GET /health` | **77,690** | 45,523 | 90,541 | **+71%** | −14% |
| `GET /api/products` | **48,810** | 40,668 | 57,163 | **+20%** | −15% |
| `GET /api/products/1` | **67,996** | 46,567 | 81,916 | **+46%** | −17% |
| `GET /api/orders` | **74,013** | 49,160 | 92,148 | **+51%** | −20% |
| `POST /api/orders` | **68,177** | 39,323 | 50,485 | **+73%** | **+35%** |
| `POST /api/auth/login` | **63,847** | 44,267 | 76,175 | **+44%** | −16% |

### Latency (avg, lower is better)

| Endpoint | AOT | Node.js | Bun |
|----------|-----|---------|-----|
| `GET /health` | **614 µs** | 1.08 ms | 525 µs |
| `GET /api/products` | **0.98 ms** | 1.18 ms | 837 µs |
| `GET /api/products/1` | **703 µs** | 1.03 ms | 582 µs |
| `GET /api/orders` | **645 µs** | 0.98 ms | 516 µs |
| `POST /api/orders` | **701 µs** | 1.24 ms | 1.40 ms |
| `POST /api/auth/login` | **749 µs** | 1.09 ms | 626 µs |

### Memory Consumption

| Runtime | Startup RSS |
|---------|-------------|
| AOT native | **5.0 MB** |
| Node.js | 39.4 MB |
| Bun | 26.3 MB |

AOT is **7.9× smaller** than Node.js and **5.3× smaller** than Bun at startup.

**Note on AOT post-benchmark RSS**: macOS RSS includes jemalloc's freed-but-madvised pages. The
actual live heap measured by `leaks` is < 10 MB (zero true leaks). On Linux, RSS behaviour is
better (`MADV_DONTNEED` returns pages immediately).

### Summary

- AOT beats Node.js on **all 6 endpoints** by **+20–73%**
- AOT beats **Bun** on POST endpoints (`POST /api/orders`: **+35%** faster)
- AOT is within **14–20%** of Bun on GET endpoints — the gap is primarily HTTP/TLS stack overhead
- **Startup memory**: AOT is **7.9× smaller** than Node.js and **5.3× smaller** than Bun

---

## PostgreSQL REST API (AOT vs Node.js vs Bun)

Compares all three runtimes serving HTTP endpoints backed by a live PostgreSQL database.

**Tool**: k6 | **Load profile**: ramp 0→20 VUs (10s) → 50 VUs (30s) → spike 100 VUs (10s) → ramp down

Each k6 iteration hits 4 endpoints in sequence: `GET /` (no DB), `GET /db` (SELECT NOW),
`GET /users` (SELECT 20 rows), `GET /users/:id` (point lookup).

### Throughput

| Runtime | Req/s | Total requests | Check pass rate |
|---------|-------|----------------|-----------------|
| **AOT native** | **8,800** | **616,132** | **100%** |
| Node.js + Express | 7,852 | 549,568 | **100%** |
| **Bun** | **11,838** | **828,800** | **100%** |

AOT now **outperforms Node.js** by **+12%** in throughput. Bun leads at 1.35× AOT throughput.

### Latency by Endpoint (avg ms, lower is better)

| Endpoint | AOT | Node.js | Bun | AOT vs Node |
|----------|-----|---------|-----|-------------|
| `GET /` (no DB) | **0.77** | 1.19 | **0.19** | **1.54× faster** |
| `GET /db` (SELECT NOW) | **2.26** | 2.75 | **1.14** | **1.22× faster** |
| `GET /users` (20 rows) | **2.71** | 3.49 | **1.20** | **1.29× faster** |
| `GET /users/:id` | **2.79** | 3.38 | **1.16** | **1.21× faster** |
| **Combined avg** | **2.13** | 2.70 | **0.92** | **1.27× faster** |

### Latency Percentiles — Combined (ms)

| Runtime | avg | p(95) | p(99) |
|---------|-----|-------|-------|
| **AOT native** | **2.13** | **6.96** | n/a |
| Node.js + Express | 2.70 | 8.63 | n/a |
| **Bun** | **0.92** | **2.52** | n/a |

---

## Performance History

These results reflect two critical bug fixes found by profiling under 100 VU load:

### Bug 1: LIFO Pool Starvation (max latency 13.54s → 8ms)

`Pool._release` used `this._waiters.pop()` (LIFO stack order). Under sustained 100 VU load with
only 10 DB connections, early-queued requests starved while later requests were served first.
Fix: `shift()` for FIFO dispatch. Max latency dropped from **13.54 seconds to ~8 ms** (1700×).

### Bug 2: Exception Spin in Loop Bodies (CPU pinned at 99%)

When the PostgreSQL socket closed unexpectedly, `ts_throw()` set the exception flag but
`while`/`for`/`for-of`/`for-in`/`do-while` loop bodies had no exception check. The loop body
called `_pullData()`, which returned immediately via an already-resolved Promise, hit `ts_throw`,
and continued — spinning indefinitely at full CPU. Fix: added `ts_check_exception()` + `cf.cond_br`
to the exit block at every loop body termination.

### Before vs After

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| PG throughput (AOT) | 1,980 req/s | **8,800 req/s** | **4.4× faster** |
| `GET /` latency (AOT) | 12.71 ms | **0.77 ms** | **16.5× faster** |
| Combined avg latency (AOT) | 18.28 ms | **2.13 ms** | **8.6× faster** |
| Max latency (100 VU spike) | 13,540 ms | **~8 ms** | **1700× faster** |
| AOT vs Node throughput | 3.5× slower | **+12% faster** | reversed |

### Optimizations Applied (in addition to bug fixes)

1. **Per-fiber bump-pointer arena** (64 KB/fiber): Short-lived objects within a request handler
   use `ARENA_RC` — retain/release are no-ops; the arena bulk-frees on fiber exit. Eliminates
   jemalloc churn for temporary allocations.
2. **ARC elision for non-escaping allocations**: Compiler analysis skips emit of retain/release
   for variables provably scoped to a single function. Extends the scalar-variable optimization
   to heap-allocated temporaries.
3. **Promise callbacks as cooperative fibers**: `.then()` / `.finally()` callbacks now schedule
   as `JsFiber` tasks on the LocalSet instead of `spawn_blocking` OS threads, eliminating
   cross-thread lock contention.
4. **`TsRequest` struct** (tag=19): Incoming HTTP requests are stored as a compact 4-field struct
   instead of a `TsObject` with a `HashMap`, eliminating 2 HashMap allocations per request.

---

## Why AOT Is Faster Than Node.js

AOT achieves **+20–73%** over Node.js on in-memory endpoints and **+12–54%** on PG-backed
endpoints because:

1. **No interpreter overhead**: Code is compiled directly to native ARM64 machine code. No V8
   bytecode interpretation or JIT warm-up phase.
2. **Fiber stack pool**: A thread-local pool of coroutine stacks avoids per-request `mmap` + `munmap`
   calls. Stack switching costs ~50 ns vs ~100 µs for a thread context switch.
3. **Single-threaded cooperative model**: One `LocalSet` thread handles all requests cooperatively.
   No locking, no cross-thread synchronization on the hot path.
4. **jemalloc**: Lower allocation overhead than macOS's system `malloc` for rapid alloc/free cycles.
5. **Per-fiber arena allocator**: Short-lived allocations within a request use bump-pointer
   allocation — O(1) with no lock, vs jemalloc's bin lookup + TLS state.

---

## Why Bun Is Still Faster Than AOT

Bun (JSC) outperforms AOT by 1.2–1.4× on GET endpoints and 1.35× on PG throughput:

1. **Tracing GC vs ARC**: JSC uses a tracing GC — zero atomic ops per value access on the hot
   path. AOT emits `ts_retain_val` / `ts_release_val` (atomic fetch_add/sub) on every heap value
   read. Under 50 concurrent VUs, hundreds of in-flight requests hammer the same cache lines.

2. **No string interning**: `"GET"`, `"content-type"`, `"/"` are heap-allocated `TsString` objects
   each with an ARC refcount. JSC interns small strings — method == `"GET"` hits a pointer
   comparison. AOT allocates a fresh `TsString` each time.

3. **JIT-specialized HashMap lookups**: V8/JSC use hidden classes (shapes) so property accesses
   on same-shaped objects compile to fixed-offset reads. AOT goes through `HashMap<String, TsVal>`
   for every property.

### What Would Close The Gap

Targeted mitigations ranked by expected impact:

1. **String interning** (intern table for strings ≤ 64 bytes): eliminates heap allocation + ARC
   for all common HTTP strings. Expected: 2–3× on HTTP-heavy paths.
2. **ARC elision extension** (escape analysis for string temporaries): already done for integers
   and non-escaping allocations; needs extension to `TsString` and `TsObject` temporaries.
3. **Hidden-class property access** (shape-indexed field slots): eliminates HashMap lookups for
   typed objects. Expected: 2× on property-heavy paths.
4. **Specialized JSON serializer** (direct struct walk): eliminates the generic `TsVal` tree walk
   for `{key: stringVal}` responses that cover 90% of HTTP response bodies.

---

## Reproduction

```bash
# In-memory benchmark
cargo build --release -p tscc
bash benchmarks/bench.sh

# PostgreSQL benchmark
docker compose up -d postgres
bash benchmarks/bench_pg.sh
```

Raw k6 JSON results: `benchmarks/aot_result.json`, `benchmarks/node_result.json`, `benchmarks/bun_result.json`.
