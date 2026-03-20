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
- `CONNECTIONS` — concurrent connections (default: 100)
- `THREADS` — wrk threads (default: 4)
