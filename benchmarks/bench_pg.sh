#!/usr/bin/env bash
# bench_pg.sh — 3-way benchmark: AOT native compiler vs Node.js+Express vs Bun
#               with PostgreSQL + k6
#
# Prerequisites:
#   docker / docker compose
#   k6  (brew install k6 / snap install k6)
#   cargo (Rust toolchain)
#   node + npm
#   bun  (bun.sh)
#
# Usage:
#   bash benchmarks/bench_pg.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/.." && pwd)"

AOT_PORT=17001
NODE_PORT=17002
BUN_PORT=17003
PGHOST=localhost
PGPORT=5432
PGUSER=bench
PGPASSWORD=bench
PGDATABASE=bench

K6_SCRIPT="$SCRIPT_DIR/k6_bench_pg.js"
AOT_BIN="$REPO/examples/bench_pg.exe"
NODE_SERVER="$SCRIPT_DIR/node_express_server.js"
BUN_SERVER="$SCRIPT_DIR/bun_pg_server.ts"

BOLD='\033[1m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; RESET='\033[0m'
info()    { echo -e "${BOLD}[bench]${RESET} $*"; }
success() { echo -e "${GREEN}[bench]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[bench]${RESET} $*"; }
err()     { echo -e "${RED}[bench]${RESET} $*" >&2; }

AOT_PID=0
NODE_PID=0
BUN_PID=0

cleanup() {
  [[ $AOT_PID  -ne 0 ]] && kill "$AOT_PID"  2>/dev/null || true
  [[ $NODE_PID -ne 0 ]] && kill "$NODE_PID" 2>/dev/null || true
  [[ $BUN_PID  -ne 0 ]] && kill "$BUN_PID"  2>/dev/null || true
  wait "$AOT_PID"  2>/dev/null || true
  wait "$NODE_PID" 2>/dev/null || true
  wait "$BUN_PID"  2>/dev/null || true
  docker compose -f "$REPO/docker-compose.yml" down --timeout 5 2>/dev/null || true
}
trap cleanup EXIT

# ── 1. Check deps ─────────────────────────────────────────────────────────────

for cmd in docker k6 cargo node bun; do
  command -v "$cmd" &>/dev/null || { err "Required: $cmd"; exit 1; }
done

# ── 2. Start PostgreSQL ───────────────────────────────────────────────────────

info "Starting PostgreSQL via docker compose..."
docker compose -f "$REPO/docker-compose.yml" up -d postgres

info "Waiting for PostgreSQL to be ready..."
for i in $(seq 1 30); do
  if docker compose -f "$REPO/docker-compose.yml" exec -T postgres \
      pg_isready -U "$PGUSER" -d "$PGDATABASE" &>/dev/null; then
    success "PostgreSQL ready"
    break
  fi
  [[ $i -eq 30 ]] && { err "PostgreSQL did not become ready"; exit 1; }
  sleep 1
done

# ── 3. Build AOT binary ───────────────────────────────────────────────────────

info "Building AOT compiler (release)..."
cargo build --release -p tscc 2>&1 | tail -3

info "Compiling bench_pg.ts → native binary..."
"$REPO/target/release/tscc" "$REPO/examples/bench_pg.ts"

# ── 4. Install deps ───────────────────────────────────────────────────────────

info "Installing Node.js dependencies (pg + express)..."
cd "$REPO/examples" && npm install --silent 2>/dev/null || true
cd "$REPO"

info "Installing Bun dependencies (pg)..."
cd "$REPO/examples" && bun add pg --silent 2>/dev/null || true
cd "$REPO"

# ── 5. Start servers ──────────────────────────────────────────────────────────

wait_for_port() {
  local port=$1 name=$2 pid=$3
  for i in $(seq 1 60); do
    if curl -sf "http://localhost:${port}/health" &>/dev/null; then
      success "$name ready on :$port"
      return 0
    fi
    kill -0 "$pid" 2>/dev/null || { err "$name (pid=$pid) exited early"; return 1; }
    sleep 0.2
  done
  err "$name did not start on :$port within 12s"
  return 1
}

info "Starting AOT server on :$AOT_PORT..."
PGHOST=$PGHOST PGPORT=$PGPORT PGUSER=$PGUSER PGPASSWORD=$PGPASSWORD PGDATABASE=$PGDATABASE \
  "$AOT_BIN" &
AOT_PID=$!
wait_for_port "$AOT_PORT" "AOT" "$AOT_PID"

info "Starting Node.js+Express server on :$NODE_PORT..."
PGHOST=$PGHOST PGPORT=$PGPORT PGUSER=$PGUSER PGPASSWORD=$PGPASSWORD PGDATABASE=$PGDATABASE \
  PORT=$NODE_PORT node "$NODE_SERVER" &
NODE_PID=$!
wait_for_port "$NODE_PORT" "Node.js+Express" "$NODE_PID"

info "Starting Bun+pg server on :$BUN_PORT..."
PGHOST=$PGHOST PGPORT=$PGPORT PGUSER=$PGUSER PGPASSWORD=$PGPASSWORD PGDATABASE=$PGDATABASE \
  PORT=$BUN_PORT bun run "$BUN_SERVER" &
BUN_PID=$!
wait_for_port "$BUN_PORT" "Bun+pg" "$BUN_PID"

# ── 6. Warm-up ────────────────────────────────────────────────────────────────

info "Warming up (10s each)..."
k6 run --quiet --no-summary --env TARGET="http://localhost:${AOT_PORT}" \
  --duration 10s --vus 10 "$K6_SCRIPT" &>/dev/null || true
k6 run --quiet --no-summary --env TARGET="http://localhost:${NODE_PORT}" \
  --duration 10s --vus 10 "$K6_SCRIPT" &>/dev/null || true
k6 run --quiet --no-summary --env TARGET="http://localhost:${BUN_PORT}" \
  --duration 10s --vus 10 "$K6_SCRIPT" &>/dev/null || true
success "Warm-up done"

# ── 7. Run benchmark ──────────────────────────────────────────────────────────

AOT_SUMMARY="$SCRIPT_DIR/aot_result.json"
NODE_SUMMARY="$SCRIPT_DIR/node_result.json"
BUN_SUMMARY="$SCRIPT_DIR/bun_result.json"

echo ""
echo -e "${BOLD}══════════════════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}  TypeScript AOT Native Compiler vs Node.js+Express vs Bun — pg+k6  ${RESET}"
echo -e "${BOLD}══════════════════════════════════════════════════════════════════════${RESET}"
echo ""

info "Benchmarking AOT server on :$AOT_PORT..."
k6 run \
  --env TARGET="http://localhost:${AOT_PORT}" \
  --summary-export "$AOT_SUMMARY" \
  "$K6_SCRIPT" 2>&1 | grep -E "✓|✗|http_req_duration|db_query|users_query|hello_lat|user_by_id|checks|iterations" || true

echo ""
info "Benchmarking Node.js+Express server on :$NODE_PORT..."
k6 run \
  --env TARGET="http://localhost:${NODE_PORT}" \
  --summary-export "$NODE_SUMMARY" \
  "$K6_SCRIPT" 2>&1 | grep -E "✓|✗|http_req_duration|db_query|users_query|hello_lat|user_by_id|checks|iterations" || true

echo ""
info "Benchmarking Bun+pg server on :$BUN_PORT..."
k6 run \
  --env TARGET="http://localhost:${BUN_PORT}" \
  --summary-export "$BUN_SUMMARY" \
  "$K6_SCRIPT" 2>&1 | grep -E "✓|✗|http_req_duration|db_query|users_query|hello_lat|user_by_id|checks|iterations" || true

# ── 8. Compare p99 results ────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}══════════════════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}  p99 Comparison (lower is better)                                   ${RESET}"
echo -e "${BOLD}══════════════════════════════════════════════════════════════════════${RESET}"
echo ""

python3 - "$AOT_SUMMARY" "$NODE_SUMMARY" "$BUN_SUMMARY" <<'PYEOF'
import json, sys

def load(path):
    with open(path) as f:
        return json.load(f)

aot  = load(sys.argv[1])
node = load(sys.argv[2])
bun  = load(sys.argv[3])

metrics = [
    ('http_req_duration',   'ALL endpoints p99'),
    ('hello_latency',       'GET /           (no DB)'),
    ('db_query_latency',    'GET /db         (SELECT NOW)'),
    ('users_query_latency', 'GET /users      (SELECT 20 rows)'),
    ('user_by_id_latency',  'GET /users/:id  (point lookup)'),
]

W = 36
print(f"  {'Endpoint':<{W}} {'AOT (ms)':>10} {'Node (ms)':>10} {'Bun (ms)':>10}  {'Winner'}")
print('  ' + '─' * (W + 46))
for key, label in metrics:
    av = aot.get('metrics',  {}).get(key, {}).get('values', {})
    nv = node.get('metrics', {}).get(key, {}).get('values', {})
    bv = bun.get('metrics',  {}).get(key, {}).get('values', {})
    ap99 = av.get('p(99)', 0)
    np99 = nv.get('p(99)', 0)
    bp99 = bv.get('p(99)', 0)
    if ap99 == 0 and np99 == 0 and bp99 == 0:
        print(f"  {label:<{W}} {'n/a':>10} {'n/a':>10} {'n/a':>10}")
        continue
    vals = {'AOT': ap99, 'Node': np99, 'Bun': bp99}
    best_name = min(vals, key=lambda k: vals[k] if vals[k] > 0 else float('inf'))
    best_val  = vals[best_name]
    parts = []
    for name, val in vals.items():
        if val > 0 and val != best_val:
            parts.append(f'{name} {val/best_val:.2f}x slower')
    winner = f'{best_name} fastest' + (f' ({", ".join(parts)})' if parts else '')
    print(f"  {label:<{W}} {ap99:>9.1f}  {np99:>9.1f}  {bp99:>9.1f}  {winner}")
print()

# Throughput
at = aot.get('metrics',  {}).get('http_reqs', {}).get('values', {}).get('rate', 0)
nt = node.get('metrics', {}).get('http_reqs', {}).get('values', {}).get('rate', 0)
bt = bun.get('metrics',  {}).get('http_reqs', {}).get('values', {}).get('rate', 0)
if at and nt and bt:
    print(f"  Throughput (req/s):  AOT={at:.1f}  Node={nt:.1f}  Bun={bt:.1f}")
    best_t = max(at, nt, bt)
    if best_t == at:
        print(f"  → AOT highest ({at/nt:.2f}x Node, {at/bt:.2f}x Bun)")
    elif best_t == bt:
        print(f"  → Bun highest ({bt/at:.2f}x AOT, {bt/nt:.2f}x Node)")
    else:
        print(f"  → Node highest ({nt/at:.2f}x AOT, {nt/bt:.2f}x Bun)")

# Check pass rate
ac = aot.get('metrics',  {}).get('checks', {}).get('values', {})
nc = node.get('metrics', {}).get('checks', {}).get('values', {})
bc = bun.get('metrics',  {}).get('checks', {}).get('values', {})
if ac and nc and bc:
    def pct(d): p = d.get('passes',0); f = d.get('fails',0); return p/(p+f)*100 if p+f else 0
    print(f"  Check pass rate:     AOT={pct(ac):.1f}%  Node={pct(nc):.1f}%  Bun={pct(bc):.1f}%")
print()
PYEOF

echo ""
success "Done. Raw JSON results:"
echo "  AOT:  $AOT_SUMMARY"
echo "  Node: $NODE_SUMMARY"
echo "  Bun:  $BUN_SUMMARY"
echo ""
