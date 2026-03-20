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

---

## NestJS Full Support Plan

### Strategy

Use **real `@nestjs/common` decorators** from the submodule + a **native bootstrap** that replaces `@nestjs/core`. The `@nestjs/core` injector machinery (`scanner.ts`, `injector.ts`, `container.ts`) requires `Proxy`, prototype-chain walking, `Object.create(proto)`, and 5+ npm packages — too complex to compile directly. The decorator layer is almost compilable today.

### Phase 1 — Fix Reflect API gaps *(~3-4 days)*

The metadata scanner uses `Reflect.getPrototypeOf` to walk prototype chains and `Reflect.getOwnPropertyDescriptor` to inspect methods. Both are unimplemented.

**1a. `Reflect.getPrototypeOf(obj)`**
- Add `("Reflect", "getPrototypeOf")` dispatch in `expressions.rs`
- Add `ts_reflect_get_prototype_of(val: i64) -> i64` in `ts-runtime/src/value/reflect.rs`
- For user class instances, return a stable per-class "prototype sentinel" TsObject (stored on the class function itself as `__proto__`)
- Terminate the chain: sentinel's prototype is UNDEFINED
- Fixes the `while (proto = Reflect.getPrototypeOf(proto)) && proto !== Object.prototype` loop

**1b. `Reflect.getOwnPropertyDescriptor(proto, key)`**
- Add dispatch + `ts_reflect_get_own_property_descriptor(obj: i64, key: i64) -> i64` runtime
- Returns `{value: fn, writable: true, enumerable: true, configurable: true}` for existing properties, UNDEFINED otherwise

**1c. `Object.prototype.hasOwnProperty.call(obj, key)` pattern**
- Detect this 4-level call chain in the call dispatch in `expressions.rs`
- Emit `ts_val_has_key(obj, key)` directly (already exists in runtime)

### Phase 2 — `emitDecoratorMetadata` (`design:paramtypes`) *(~1 week)*

The real NestJS DI resolves constructor dependencies by reading `Reflect.getMetadata('design:paramtypes', ServiceClass)`. TypeScript with `emitDecoratorMetadata: true` emits this automatically; the compiler currently emits nothing.

In `lower_class_declaration_with_name` (classes.rs):
- For each decorated class, inspect constructor parameter `TSTypeAnnotation` nodes in the OXC AST
- For each type annotation that names a known class (look up in `classes` map), resolve it to the constructor `TsFunction` value
- Build a `TsArray` of those constructor values
- Emit `Reflect.defineMetadata('design:paramtypes', array, class_ctor)` before applying class decorators

Simple cases first: named class types as constructor params. Skip interfaces and primitive types (`string` → `String`, `number` → `Number`).

### Phase 3 — Parameter decorators *(~4-5 days)*

`@Inject(token)` is a `ParameterDecorator` — called as `decorator(targetClass, propertyKey, parameterIndex)`. Currently unimplemented.

In `lower_class_constructor` (classes.rs):
- After emitting the constructor body, for each `FormalParameter` that has `.decorators`, emit calls: `decorator(class_ctor, undefined, param_index)`
- For method parameters (decorated with `@Body()`, `@Param()`, `@Query()`): emit `decorator(class_prototype, method_name_string, param_index)`

### Phase 4 — `this.constructor` and `.name` *(~2 days)*

Used by `HttpException.initName()` and error formatting.

In `lower_class_constructor` (classes.rs):
- After `ts_obj_new`, emit:
  - `ts_obj_set(this, "__class_name__", ts_string_new("ClassName"))`
  - `ts_obj_set(this, "__class_ctor__", class_ctor_fn_val)`

In `StaticMemberExpression` handler (expressions.rs):
- When property is `constructor` on a TsObject → return `__class_ctor__`
- When property is `name` on something from `constructor` → return `__class_name__`

### Phase 5 — `process.*` event emitter stubs *(~1-2 days)*

`NestApplicationContext.enableShutdownHooks()` calls `process.on('SIGTERM', fn)`.

Add to `globals.rs` in ts-runtime:
- `ts_process_on(signal: i64, fn: i64)` — registers a signal handler or stub
- `ts_process_once(signal: i64, fn: i64)` — same, one-shot
- `ts_process_remove_listener(signal: i64, fn: i64)` — no-op
- `ts_process_kill(pid: i64, signal: i64)` — stub
- `ts_process_abort()` — calls `std::process::abort()`
- `process.pid` → `ts_process_pid()` returns i32

Wire in `expressions.rs` under the existing `process.*` dispatch.

### Phase 6 — ES `Proxy` *(~1-2 weeks)*

`NestFactory.create()` wraps the app in `new Proxy(target, { get, set })`. Without this, `NestFactory` is broken.

New heap type `TsProxy` (tag 16):
1. Add `TsProxy { target: TsVal, handler: TsVal }` struct in `ts-runtime/src/value/proxy.rs`
2. Register destructor in `ts_release_val`
3. `ts_proxy_new(target: i64, handler: i64) -> i64`
4. In `ts_val_get_key(obj, key)`: if `obj` is `TsProxy`, call `handler.get(target, key, proxy)` trap
5. In `ts_obj_set(obj, key, val)`: if `obj` is `TsProxy`, call `handler.set(target, key, val, proxy)` trap
6. Wire `new Proxy(target, handler)` in `lower_new_expression` (expressions.rs)
7. Declare all functions in `declare_runtime_funcs` (mod.rs)

### Phase 7 — npm package shims *(~3-4 days)*

The compiler resolves only `.ts` files. Add an import alias table in `ts-codegen/src/lowering/mod.rs` that maps bare specifiers to local shim paths.

Shims needed for `@nestjs/common`:

| Package | Used for | Shim |
|---|---|---|
| `uid` | `mixin()` unique IDs | `function uid(n) { return Math.random().toString(36).slice(2, 2+n) }` |
| `iterare` | Set/Map iteration chains | Thin wrapper class converting to array then using native array methods |
| `path-to-regexp` | Versioned route matching | Not needed for minimal server |
| `perf_hooks` | `performance.now()` | Already exists as global |
| `util` | `util.inspect` in logger | Stub: `inspect = JSON.stringify` |
| `os` | `os.platform()` | Returns `'linux'` or `'darwin'` |

Implementation: Add a `shim_paths: HashMap<&str, PathBuf>` table in `process_import_recursive`. Before trying to load a file, check if the specifier matches a known package name and redirect to the shim.

### Phase 8 — `Object.create(proto)` support *(~1 week)*

The real NestJS injector creates instances via `Object.create(metatype.prototype)` + `metatype.apply(instance, args)`. This is a fundamental mismatch with the compiler's model.

**Option A (simpler):** In the native bootstrap, call `new ClassName(...args)` directly — avoids the issue entirely, already what `nest-native.ts` does.

**Option B (complete):** Add `ts_obj_create_with_proto(proto: i64) -> i64` and `Function.prototype.apply(thisArg, argsArray)` dispatch. Enables the two-phase injector path.

### Milestones

| Milestone | What it enables | Effort |
|---|---|---|
| **M1** | Phase 1 (Reflect API) + Phase 7 (shims) | ~1 week | Real `@nestjs/common` decorator imports compile |
| **M2** | + Phase 2 (paramtypes) + Phase 3 (param decorators) | +1 week | DI metadata fully populated without `@Inject()` everywhere |
| **M3** | + Phase 4 (constructor.name) + Phase 5 (process.on) | +3 days | `HttpException` works; shutdown hooks work |
| **M4** | + Phase 6 (Proxy) | +1-2 weeks | `NestFactory.create()` works; real `@nestjs/core` DI |
| **M5** | + Phase 8 (Object.create) | +1 week | Fully real `@nestjs/core` injector path |

**Recommended path:** After M2, the real `@nestjs/common` decorators work with the existing native bootstrap (`nest-native.ts`). M4/M5 are needed only to compile `NestFactory.create()` unmodified.

---

## Language Gap Backlog (2026-03-20)

Gaps identified by static analysis of the current lowering code. Ordered by practical impact.

### [x] 1. `++`/`--` on member/computed targets
`arr[i]++`, `this.count++` — implemented in `operators.rs:lower_update_expression`.

### [x] 2. Compound assignment (`+=`, `-=`, etc.) on computed members
`arr[i] += 1`, `obj[key] -= 2` — implemented via `lower_computed_member_assignment` in `operators.rs`.

### [x] 3. Compound assignment on private fields
`this.#field += 1` — implemented via `lower_private_field_assignment` in `operators.rs`.

### [x] 4. Logical assignment (`||=`, `&&=`, `??=`) to member targets
`obj.x ??= y`, `this.x ||= y` — already implemented in `operators.rs`.

### [x] 5. Nested destructuring
`const { a: { b } } = obj`, `const [[x, y]] = arr`, function params — implemented via `lower_bind_pattern` recursive helper in `statements.rs`.

### [x] 6. Async class methods
`async doSomething() { ... }` — already implemented in `classes.rs` (sets `is_async`, wraps return in `ts_promise_resolve`).

### [x] 7. `import.meta`
`import.meta.url`, `import.meta.dirname`, `import.meta.env` — implemented via `ts_import_meta_new()` runtime function; `Expression::MetaProperty` handled in `expressions.rs`.

### [x] 8. Generator functions
`function*` / `yield` — already implemented. Generator collects all yielded values into a TsArray and returns it (eager evaluation model). `for...of`, spread `[...gen()]` work correctly.

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
