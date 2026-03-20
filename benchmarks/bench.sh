#!/usr/bin/env bash
# Benchmark: TypeScript AOT native compiler vs Node.js
# Compares vendure_bench.ts (AOT compiled) against vendure_node.js (Node.js)
#
# Requirements: wrk, node, cargo
# Install wrk: brew install wrk (macOS) / apt install wrk (Linux)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

AOT_PORT=19888
NODE_PORT=19889
DURATION=10
CONNECTIONS=100
THREADS=4

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
RESET='\033[0m'

info()    { echo -e "${BOLD}[bench]${RESET} $*"; }
success() { echo -e "${GREEN}[bench]${RESET} $*"; }
warn()    { echo -e "${YELLOW}[bench]${RESET} $*"; }
error()   { echo -e "${RED}[bench]${RESET} $*" >&2; }

cleanup() {
  info "Stopping servers..."
  kill "$AOT_PID" 2>/dev/null || true
  kill "$NODE_PID" 2>/dev/null || true
  wait "$AOT_PID" 2>/dev/null || true
  wait "$NODE_PID" 2>/dev/null || true
}

check_deps() {
  if ! command -v wrk &>/dev/null; then
    error "wrk not found. Install with: brew install wrk (macOS) / apt install wrk (Linux)"
    exit 1
  fi
  if ! command -v node &>/dev/null; then
    error "node not found"
    exit 1
  fi
}

wait_for_port() {
  local port=$1 name=$2 pid=$3
  local attempts=0
  while ! curl -sf "http://localhost:$port/health" >/dev/null 2>&1; do
    if ! kill -0 "$pid" 2>/dev/null; then
      error "$name server (pid=$pid) died before becoming ready"
      exit 1
    fi
    sleep 0.2
    attempts=$((attempts + 1))
    if [[ $attempts -gt 50 ]]; then
      error "$name server did not start on port $port within 10s"
      exit 1
    fi
  done
  success "$name server ready on :$port"
}

run_wrk() {
  local url=$1
  wrk -t"$THREADS" -c"$CONNECTIONS" -d"${DURATION}s" "$url" 2>&1
}

extract_rps() {
  # Extract requests/sec from wrk output
  grep "Requests/sec:" | awk '{print $2}'
}

extract_latency() {
  # Extract avg latency from wrk output
  grep "Latency" | awk '{print $2}'
}

check_deps

info "Building AOT binary..."
cargo build --release -p tscc 2>/dev/null
"$REPO_ROOT/target/release/tscc" "$REPO_ROOT/examples/vendure_bench.ts" 2>/dev/null
AOT_BIN="$REPO_ROOT/examples/vendure_bench.exe"

info "Starting AOT server (port $AOT_PORT)..."
"$AOT_BIN" &>/dev/null &
AOT_PID=$!

info "Starting Node.js server (port $NODE_PORT)..."
PORT=$NODE_PORT node "$SCRIPT_DIR/vendure_node.js" &>/dev/null &
NODE_PID=$!

trap cleanup EXIT

wait_for_port $AOT_PORT "AOT" $AOT_PID
wait_for_port $NODE_PORT "Node.js" $NODE_PID

echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}  TypeScript AOT Native Compiler vs Node.js Benchmark    ${RESET}"
echo -e "${BOLD}════════════════════════════════════════════════════════${RESET}"
echo -e "  Duration: ${DURATION}s | Connections: ${CONNECTIONS} | Threads: ${THREADS}"
echo ""

ENDPOINTS=(
  "GET /health"
  "GET /api/products"
  "GET /api/products/1"
  "GET /api/orders"
)

URLS_AOT=(
  "http://localhost:$AOT_PORT/health"
  "http://localhost:$AOT_PORT/api/products"
  "http://localhost:$AOT_PORT/api/products/1"
  "http://localhost:$AOT_PORT/api/orders"
)

URLS_NODE=(
  "http://localhost:$NODE_PORT/health"
  "http://localhost:$NODE_PORT/api/products"
  "http://localhost:$NODE_PORT/api/products/1"
  "http://localhost:$NODE_PORT/api/orders"
)

declare -A AOT_RPS NODE_RPS

for i in "${!ENDPOINTS[@]}"; do
  endpoint="${ENDPOINTS[$i]}"
  info "Benchmarking: $endpoint"

  aot_out=$(run_wrk "${URLS_AOT[$i]}")
  aot_rps=$(echo "$aot_out" | extract_rps)
  aot_lat=$(echo "$aot_out" | extract_latency)

  node_out=$(run_wrk "${URLS_NODE[$i]}")
  node_rps=$(echo "$node_out" | extract_rps)
  node_lat=$(echo "$node_out" | extract_latency)

  AOT_RPS[$i]=$aot_rps
  NODE_RPS[$i]=$node_rps

  ratio=$(awk "BEGIN {printf \"%.2f\", $aot_rps / $node_rps}" 2>/dev/null || echo "?")

  echo ""
  echo -e "  ${BOLD}$endpoint${RESET}"
  printf "  %-20s  %12s req/s  %10s avg latency\n" "AOT (tscc):" "$aot_rps" "$aot_lat"
  printf "  %-20s  %12s req/s  %10s avg latency\n" "Node.js:" "$node_rps" "$node_lat"
  if (( $(echo "$aot_rps > $node_rps" | awk '{print ($1 > $2)}' 2>/dev/null || echo 0) )); then
    echo -e "  ${GREEN}AOT is ${ratio}x faster${RESET}"
  else
    inv=$(awk "BEGIN {printf \"%.2f\", $node_rps / $aot_rps}" 2>/dev/null || echo "?")
    echo -e "  ${YELLOW}Node.js is ${inv}x faster${RESET}"
  fi
done

echo ""
echo -e "${BOLD}════════════════════════════════════════════════════════${RESET}"
echo ""
