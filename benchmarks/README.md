# Benchmarks

Compares the TypeScript AOT native compiler output against Node.js for a Vendure-like REST API server.

## Setup

```bash
# Install wrk (HTTP benchmarking tool)
brew install wrk        # macOS
apt install wrk         # Ubuntu/Debian
```

## Run

```bash
./benchmarks/bench.sh
```

This will:
1. Build the AOT-compiled binary from `examples/vendure_bench.ts`
2. Start the AOT server on port 19888
3. Start the equivalent Node.js server (`vendure_node.js`) on port 19889
4. Run `wrk` against both servers for each endpoint
5. Print a comparison table

## Endpoints Benchmarked

| Endpoint | Description |
|----------|-------------|
| `GET /health` | Health check (in-memory data size) |
| `GET /api/products` | List all products (paginated) |
| `GET /api/products/1` | Get single product by ID |
| `GET /api/orders` | List all orders |

## Parameters

Edit `bench.sh` to change:
- `DURATION` — benchmark duration in seconds (default: 10)
- `CONNECTIONS` — concurrent connections (default: 50)
- `THREADS` — wrk threads (default: 4)

## Latest Results

**Machine**: Apple M-series, macOS
**Date**: 2026-03-20
**Settings**: 10s duration, 50 connections, 4 threads
**wrk** 4.2.0

| Endpoint | AOT (tscc) req/s | Node.js req/s | Speedup |
|----------|-----------------|---------------|---------|
| `GET /health` | 80,980 | 45,335 | **1.79x** |
| `GET /api/products` | 68,167 | 38,553 | **1.77x** |
| `GET /api/products/1` | 78,688 | 43,873 | **1.79x** |
| `GET /api/orders` | 67,382 | 46,477 | **1.45x** |

| Endpoint | AOT latency | Node.js latency |
|----------|-------------|-----------------|
| `GET /health` | 646 µs | 1.09 ms |
| `GET /api/products` | 1.04 ms | 1.26 ms |
| `GET /api/products/1` | 635 µs | 1.10 ms |
| `GET /api/orders` | 704 µs | 1.04 ms |

AOT-compiled binary is consistently **~1.5–1.8x faster** than Node.js across all endpoints.
