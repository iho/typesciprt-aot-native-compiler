# Development Plan & Roadmap

**Last Updated**: 2026-03-13

---

## Vision

Compile TypeScript to native binaries with zero VM, zero JIT, and zero Node.js dependency. The primary target is running Hono web framework as a native HTTP server backed by Rust's hyper and tokio.

---

## Architecture

```
TypeScript Source
       ↓  OXC parser
    OXC AST
       ↓  ts-codegen (lowering/)
  MLIR (func/arith/cf/llvm dialects)
       ↓  mlir-opt + mlir-translate
    LLVM IR
       ↓  clang (links ts-runtime)
  Native Binary
```

**Key design decisions:**
- **NaN-boxing**: All TS values are `TsVal = i64`. Heap objects tagged by pointer. No separate type tags needed for function calls.
- **ARC**: Every identifier read produces an owned reference via `ts_retain_val`; temporaries released via `ts_release_val`.
- **Tokio runtime**: Async functions return a `TsPromise` (heap tag 3). The tokio rt-multi-thread executor backs all async I/O.
- **hyper 1.x**: HTTP server via `ts_serve(port, fetchFn)` — blocks in the tokio event loop.
- **MLIR phi nodes**: All scope variables normalized to `i64` before merge blocks (if/while/for/try/logical ops).

---

## Completed Milestones

### ✅ Milestone 1 — Core Language (Shipped)
Arithmetic, variables, control flow, functions, strings, arrays, objects, closures.

### ✅ Milestone 2 — Advanced Language (Shipped)
Rest params, destructuring, spread, optional chaining, nullish coalescing, logical assignment, for-of, for-in, template literals, typeof, instanceof, in, delete.

### ✅ Milestone 3 — Classes & Error Handling (Shipped)
Classes with inheritance, private fields/methods, super calls, static methods, try/catch/finally/throw, classes extending Error types, TypeScript overload signatures.

### ✅ Milestone 4 — Built-in APIs (Shipped)
Array/String/Object/Math/Map full method support, RegExp, JSON.stringify/parse, encodeURI/decodeURI, Number/String/Boolean coercion, Promise.resolve/reject/all.

### ✅ Milestone 5 — Async HTTP Server (Shipped)
async/await with tokio, HTTP server via hyper (`serve(port, fn)`), Request/Response/Headers construction, module-level globals.

### ✅ Milestone 6 — Hono context.ts (Shipped)
All features needed to compile `hono/src/context.ts`:
- Private class fields and methods
- Logical assignment to member targets (`this.x ??= val`)
- TypeScript method overloads (bodyless signatures skipped)
- Classes extending Error (`HTTPException extends Error`)
- Promise namespace calls (`Promise.resolve`, `Promise.reject`, `Promise.all`)
- Boolean/Error/Number/String coercions
- All built-in methods from Array/String/Map/Object/Math

### ✅ Milestone 7 — Nested Functions & Mutable Capture (Current)
Inner function declarations within function bodies:
- Closures created at declaration position (sequential order preserves variable initialization)
- Self-referential recursive inner functions via `ts_closure_get_env` + `ts_arr_set` env patching
- Mutable captured variables: assignments inside closures write back to shared env array via `ts_arr_set`
- `__env` stored in scope but excluded from ARC release (env is caller-owned, not closure-owned)
- `hono/src/compose.ts` compiles successfully (async dispatch with recursion)

**Test suite**: 49 tests, all pass.

---

## Current Status

### Compiles
- `hono/src/context.ts` ✅
- `hono/src/compose.ts` ✅
- `hono/src/http-exception.ts` ✅

### Blocked on `new URL(...)`
- `hono/src/hono-base.ts` — line 366: `const url = new URL(request.url)`
- `hono/src/hono.ts` — transitively imports hono-base.ts

---

## Next Milestone: Full Hono HTTP Server

### Phase A — URL / URLSearchParams (Unblocks hono-base.ts)

**Priority 1: `new URL(href)` constructor**

Add `url = "2"` to `ts-runtime/Cargo.toml`. Implement `ts_url_new(href: TsVal) -> TsVal` which:
- Parses the URL with Rust's `url::Url::parse()`
- Creates a `TsObject` (tag 0) with string properties: `href`, `protocol`, `host`, `hostname`, `port`, `pathname`, `search`, `hash`, `origin`
- Creates a URLSearchParams from the search string and stores as `searchParams` property
- Declares the constructor in codegen `lower_new_expression` for class name `"URL"`

**Priority 2: `new URLSearchParams(init?)` constructor**

Implement as a new heap type (tag 9, same layout as TsMap). Extend `ts_map_*` guards to accept tag 9 so `get`, `set`, `has`, `delete`, `entries`, `keys`, `values`, `forEach` all work via existing codegen dispatch. Add `ts_urlsearchparams_new(init: TsVal)`, `ts_urlsearchparams_to_string`, `ts_urlsearchparams_append`, `ts_urlsearchparams_get_all`.

### Phase B — Headers Full API

`ts_map_get/set/has/delete/keys/values/entries/for_each` already accept tag 7 (Headers) at the Rust level. Wire codegen dispatch for method names `"get"`, `"set"`, `"has"`, `"delete"`, `"forEach"`, `"entries"`, `"keys"`, `"values"` on Headers objects. These route to the existing `ts_map_*` functions.

### Phase C — Request/Response Body Methods

```rust
// value.rs
pub unsafe extern "C" fn ts_request_text(req: TsVal) -> TsVal   // -> Promise<string>
pub unsafe extern "C" fn ts_request_json(req: TsVal) -> TsVal   // -> Promise<any>
pub unsafe extern "C" fn ts_response_text(resp: TsVal) -> TsVal // -> Promise<string>
pub unsafe extern "C" fn ts_response_json(resp: TsVal) -> TsVal // -> Promise<any>
```

Wire `"text"` and `"json"` method names in codegen `is_builtin` dispatch. Route by receiver heap tag (0 = Request, 8 = Response).

### Phase D — addEventListener (Real Implementation)

Replace the current no-op with a real global fetch handler registration:
```rust
static FETCH_LISTENER: AtomicU64 = AtomicU64::new(UNDEFINED_BITS);

pub unsafe extern "C" fn ts_add_event_listener(event: TsVal, handler: TsVal) -> TsVal
pub unsafe extern "C" fn ts_serve_worker(port: i32) -> TsVal  // uses FETCH_LISTENER
```

Wire `addEventListener` and `serve(port)` (1-arg form) in codegen to call these.

### Phase E — Async Class Methods

In `lower_class_declaration`, when a method has `value.r#async == true`, emit it with async semantics (wrap return in `ts_promise_resolve`, handle `await` expressions). Currently the `is_async` flag is only set for top-level function declarations.

---

## Future Work (Post Hono)

### Floating-Point Arithmetic
Currently all arithmetic is i32 (truncated). NaN-boxing supports f64 (TAG_FLOAT), but the arithmetic lowering uses `arith.addi`, `arith.subi`, etc. Switching to `ts_add(a, b)` runtime dispatch (which uses f64) would fix all number operations.

### Proper Mutable Shared Closures
Multiple closures capturing the same variable should share a mutable cell. Currently each closure snapshots the value at creation time. Fix: allocate a `TsCell` (single-element TsArray) for each shared-mutable variable and pass the cell reference (not the value) to all closures that capture it.

### True Hoisting for Forward-Referenced Inner Functions
Currently inner `function` declarations are processed at their source position. To handle the pattern `return dispatch(0); function dispatch(i) {...}`, true hoisting would require processing function declarations before other statements — which requires mutable cells for variables captured by those functions.

### switch / case
```typescript
switch (expr) {
  case 1: ...; break;
  default: ...;
}
```

### do...while
```typescript
do { ... } while (condition);
```

### WeakMap / WeakSet
Used by some Hono middleware and many JS patterns.

### Symbol.iterator / Custom Iterators
Needed for for-of over user-defined iterables.

### Full fetch() API
Implement `fetch(url, options?)` using tokio + hyper client.

### Tagged Template Literals
`` html`<div>${content}</div>` ``

---

## Design Principles

1. **Correctness over shortcuts** — Implement features correctly for long-term use, not as no-ops
2. **Hono as the integration test** — Compiling a real production framework is the acceptance criterion
3. **ARC discipline** — Every heap allocation follows retain/release; no GC
4. **Sequential compilation** — No parallelism in the codegen pass; use `--test-threads=1`
5. **MLIR phi correctness** — All scope variables normalized to `i64` before any merge block

---

## Testing

```bash
# Full test suite (49 tests)
cargo test -p tscc -- --include-ignored --test-threads=1

# Compile a specific file
cargo run -p tscc -- path/to/file.ts -o /tmp/out && /tmp/out
```

All tests must pass before merging any change.
