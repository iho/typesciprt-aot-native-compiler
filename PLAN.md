# Development Plan & Roadmap

**Last Updated**: 2026-03-21

---

## Vision

Compile TypeScript to native binaries with zero VM, zero JIT, and zero Node.js dependency. Primary target: running production web frameworks (Hono, NestJS-style) as native HTTP servers backed by Rust's hyper and tokio.

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
- **NaN-boxing**: All TS values are `TsVal = i64`. Heap objects tagged by pointer.
- **ARC**: Every identifier read produces an owned reference via `ts_retain_val`; temporaries released via `ts_release_val`.
- **Tokio runtime**: Async functions return a `TsPromise` (heap tag 3). The tokio rt-multi-thread executor backs all async I/O.
- **hyper 1.x**: HTTP server via `ts_serve(port, fetchFn)` — blocks in the tokio event loop.
- **MLIR phi nodes**: All scope variables normalized to `i64` before merge blocks.

---

## Current Status (2026-03-21)

**86 integration tests pass.** Hono (unmodified), NestJS-style DI, and a Vendure-like REST benchmark all compile and serve correctly.

### What works
- Full TypeScript class system: inheritance, private fields/methods, static fields, getters/setters, decorators, parameter properties
- `emitDecoratorMetadata` (`design:paramtypes`) for NestJS-style DI
- `Reflect.defineMetadata` / `getMetadata` / `hasMetadata` / `deleteMetadata`
- `Map`, `Set`, `WeakMap`, `WeakSet`, `WeakRef` (strong-ref backed)
- `RegExp`: literals, `new RegExp()`, `.test()`, `.exec()`, `.match()`, `.replace()`
- `for...of` (Array, String, Map entries, Set), `for...in` (object keys)
- Destructuring (nested, with defaults, rest) in variable declarations and function params
- `switch`/`case` with fallthrough, `do...while`, `while`, `for`
- `try`/`catch`/`finally` for synchronous exceptions
- `async`/`await` (Tokio-backed), `Promise.all` / `allSettled` / `any` / `race`
- `fetch()` global HTTP client (hyper-backed)
- `node:http` / `node:https` / `node:path` / `node:fs` / `node:crypto` / `node:os` + 10 more
- npm package resolution (CJS + ESM, TS source preferred over compiled JS)
- Generator functions (`function*` / `yield`) — eager-collection model
- `import.meta` (url, dirname, filename, env)
- `Array.from(iterable)` — Array, Set, Map, string, array-likes
- Optional chaining (`?.`) on members and method calls
- Nullish coalescing (`??`) and assignment (`??=`, `||=`, `&&=`)
- Spread into rest parameters and class method rest params
- `arguments` object as synthetic array
- `instanceof`, `typeof`, `in`, `delete`
- `console.log/error/warn/info/debug`

### Performance
- **~4.9 MB RSS** at startup (vs ~37 MB for bare Node.js, ~100-300 MB for a framework app)
- **1.5–1.85× faster** than Node.js v22 on REST API benchmarks (Vendure-like workload)

---

## Known Limitations

### Semantic gaps (most impactful)

**1. Rejected promise propagation in `try/catch`** *(Priority 1)*
`await Promise.reject(new Error("oops"))` inside a try block does not throw into the catch handler — rejection is silently swallowed. This blocks error-handling middleware in any framework.

**2. True lazy generators** *(Priority 2)*
`function*` compiles and `for...of` / spread work, but all values are eagerly collected on first iteration. Infinite sequences and side-effecting stateful generators don't work.

**3. `Proxy`** *(Priority 3)*
`new Proxy(target, handler)` is not implemented. Needed for `NestFactory.create()` to work with unmodified `@nestjs/core`.

**4. Custom `Symbol.iterator` protocols** *(Priority 3)*
`for...of` works on built-in types only. User-defined iterables implementing `[Symbol.iterator]()` are not supported.

**5. Dynamic `import()` expressions** *(Priority 3)*
Only static `import` declarations at the top of a file are handled.

**6. `Object.defineProperty` with descriptors** *(Priority 4)*
Class getters/setters work. Property descriptors on plain objects (value/writable/enumerable/configurable/get/set) are not implemented.

**7. True weak-reference semantics**
`WeakRef`, `WeakMap`, `WeakSet` use ARC strong references. They work as containers but don't enable GC-lifecycle management.

**8. `eval()` / `new Function()`**
Not possible in an AOT compiler.

### Standard library gaps
- `Intl` APIs (locale-aware formatting, collation)
- `console.group` / `console.table` / `console.time` / `console.timeEnd`
- `Promise.withResolvers()`
- `RegExp` named capture groups and lookbehind assertions
- `structuredClone` with circular references (currently panics)

### Tooling gaps
- No source maps / DWARF debug info — stack traces are Rust-level
- No incremental compilation — all imported files re-lowered on each run
- Single-file output only

---

## Roadmap

### Priority 1 — Promise rejection propagation

**Goal:** `await Promise.reject(err)` throws into the surrounding `try/catch`.

**Approach:**
- Add a thread-local "thrown error" cell in the runtime (`ts_get_thrown()` / `ts_clear_thrown()`).
- In `ts_promise_await`: if the promise is in rejected state, store the rejection value in the TL cell and return a sentinel i64 tag.
- In `lower_await_expression` (codegen): after every `await`, check `ts_is_thrown()`. If set, branch to the current catch block (or propagate as an error return if none).
- In `lower_try_statement`: set/restore the throw-recovery branch address before/after the try body.

**Effort:** ~3-4 days

---

### Priority 2 — True lazy generators

**Goal:** `function*` with infinite or stateful sequences.

**Approach:**
- Compile each `function*` body as a state machine: each `yield` point becomes a resumable state (`i32`).
- The generator MLIR function signature: `(env: i64, state: i32, send_val: i64) -> i64`.
- Wrap in a `TsGenerator` heap type (new tag) with `{ fn_ptr, env, state, done }`.
- `ts_generator_next(gen, send_val)` calls the function with saved state; returns `{ value, done }` object.
- `for...of` and spread detect `TsGenerator` tag and call `ts_generator_next` in a loop.

**Effort:** ~1-2 weeks

---

### Priority 3 — `Proxy`

**Goal:** `new Proxy(target, handler)` for `NestFactory.create()` support.

**Approach:**
1. New heap type `TsProxy` (next available tag) with `{ target: TsVal, handler: TsVal }`.
2. Register destructor in `ts_release_val`.
3. `ts_proxy_new(target, handler) -> TsVal`.
4. In `ts_val_get_key(obj, key)`: if `obj` is `TsProxy`, call `handler.get(target, key, proxy)`.
5. In `ts_obj_set(obj, key, val)`: if `obj` is `TsProxy`, call `handler.set(target, key, val, proxy)`.
6. Wire `new Proxy(target, handler)` in `lower_new_expression`.

**Effort:** ~1-2 weeks

---

### Priority 3 — Custom `Symbol.iterator`

**Goal:** `for...of` on user-defined iterables.

**Approach:**
- In `lower_for_of_statement`, after the builtin checks, call `ts_val_get_key(obj, Symbol.iterator)`.
- If the result is a callable, call it to get an iterator object.
- Loop calling `iterator.next()` until `{ done: true }`.

**Effort:** ~2-3 days

---

### Priority 3 — Dynamic `import()`

**Goal:** `const mod = await import('./plugin')`.

**Approach:**
- Treat `import(specifier)` as a call to `ts_dynamic_import(spec: TsVal) -> TsVal` (returns Promise).
- At compile time, register all modules in a global name-to-init-fn map (`ts_cjs_register_ns` style).
- At runtime, `ts_dynamic_import` looks up the module, calls its init function if not yet run, returns the namespace object.

**Effort:** ~3-4 days

---

### Priority 4 — `Object.defineProperty` with descriptors

**Goal:** Support `{ get, set, enumerable, configurable, writable }` descriptors on plain objects.

**Approach:**
- `ts_obj_define_property(obj, key, descriptor)` runtime function.
- Store getter/setter closures in `TsObject`'s field map alongside values using a `FieldEntry` enum.
- In `ts_obj_get` / `ts_obj_set`, check for accessor entries and call the getter/setter.

**Effort:** ~1 week

---

### Priority 4 — Source maps / debug info

**Goal:** Meaningful stack traces from compiled binaries.

**Approach:**
- Attach OXC span (file/line/col) to MLIR operations as `loc` attributes.
- Pass `--debugify` to `mlir-opt` and `--emit-debug-entry-values` to `llc`.
- Output DWARF debug info in the final `.o` file.

**Effort:** ~1 week

---

### Priority 5 — Incremental compilation

**Goal:** Re-use compiled modules between runs.

**Approach:**
- Hash each source file's content and cache per-file MLIR under `target/tscc-cache/<hash>.mlir.bc`.
- On recompile, skip lowering for files whose hash matches; link the cached `.o` directly.
- Invalidate transitively when imports change.

**Effort:** ~1 week

---

## Design Principles

1. **Correctness over shortcuts** — Implement features properly for long-term use, not as stubs
2. **Real frameworks as acceptance criteria** — Hono, NestJS, Vendure as compilation targets
3. **ARC discipline** — Every heap allocation follows retain/release; no GC
4. **Sequential compilation** — No parallelism in the codegen pass; always use `--test-threads=1`
5. **MLIR phi correctness** — All scope variables normalized to `i64` before any merge block

---

## Testing

```bash
# Full test suite (86 tests, requires LLVM)
cargo nextest run -p tscc --run-ignored all --test-threads 1

# Or with cargo test
cargo test -p tscc -- --include-ignored --test-threads=1

# Compile a specific file
cargo run -p tscc -- path/to/file.ts && ./path/to/file.exe
```

All 86 tests must pass before merging any change.
