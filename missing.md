# Missing / Unimplemented Features

Features not yet implemented. Items marked ✅ were previously missing but are now done.

---

## Language Features

### Control Flow
- `switch` / `case` — not supported
- `do...while` — only `while` and `for` are supported
- Labeled statements (`outer: for ...`) — ignored

### Functions & Closures
- Async class methods — top-level async functions work; class async methods do not
- Generator functions (`function*`, `yield`) — not supported
- Shared mutable capture across multiple closures — each closure gets its own snapshot of captured values; mutations in one closure are not visible in another (requires mutable cell semantics)
- Forward-hoisted inner function declarations — if a function calls an inner function declared later in source order, the call will receive `undefined` instead of the closure (sequential processing; true hoisting requires mutable cells)

### Types & Values
- `Symbol` — not supported
- `BigInt` — not supported
- Floating-point numbers — all numbers are i32 (f64 partial: stored as NaN-boxed but arithmetic ops use i32 truncation)

### Classes
- Async class methods — `async` on class methods is ignored, body runs synchronously
- Static class fields with complex initializers — basic static methods work, static field initializers may not

### Modules
- Circular imports — would cause infinite recursion in the lowering pass
- `export * from './bar'` — re-exports from another module are not tracked
- `import * as ns` — namespace imports are not bound
- External/npm modules — silently skipped

---

## Built-in Web APIs

### URL / URLSearchParams
- `new URL(href, base?)` — constructor not implemented; causes compilation failure
- `URLSearchParams` — not implemented
- `URL.pathname`, `URL.search`, `URL.origin`, etc. — not accessible until constructor exists

### Fetch / HTTP
- `fetch()` — not implemented
- `Request.text()` / `Request.json()` / `Request.formData()` / `Request.arrayBuffer()` — not implemented
- `Response.text()` / `Response.json()` / `Response.status` / `Response.ok` — not implemented
- `Headers.get()` / `Headers.set()` / `Headers.has()` / `Headers.delete()` / `Headers.forEach()` / `Headers.entries()` — codegen dispatch missing (runtime functions exist via ts_map_* but not wired)

### Service Worker / Browser APIs
- `addEventListener` / `removeEventListener` — currently a no-op (evaluates args, returns undefined); should register a real fetch handler for service-worker pattern
- `queueMicrotask` — not implemented
- `structuredClone` — not implemented
- `crypto` — not implemented
- `performance` — not implemented
- `AbortController` / `AbortSignal` — not implemented
- `FormData` — not implemented
- `Blob` / `File` — not implemented
- `ReadableStream` / `WritableStream` — not implemented
- `WebSocket` — not implemented

### Other Built-ins
- `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` — in free-var exclusion list but not callable
- `WeakMap` / `WeakRef` — not implemented
- `Proxy` / `Reflect` — not implemented
- Tagged template literals — not supported
- `Symbol.iterator` / custom iterators — not supported

---

## What IS Implemented

For reference, these were previously missing and are now working:

### Core Language
- ✅ Arrow functions and closures (free-variable capture, env TsArray)
- ✅ Rest parameters (`...args`), rest destructuring (`const { a, ...rest } = obj`)
- ✅ Default parameters
- ✅ Spread in function calls (`fn(...arr)`)
- ✅ Nested function declarations (hoisted at declaration position, mutable env writeback)
- ✅ Recursive inner functions (self-reference patched via `ts_closure_get_env`)
- ✅ `for...of` (arrays, strings, Map entries)
- ✅ `for...in` (object keys)
- ✅ Destructuring (object and array patterns, nested, with defaults)
- ✅ Optional chaining (`?.`)
- ✅ Nullish coalescing (`??`)
- ✅ Logical assignment (`&&=`, `||=`, `??=`)
- ✅ `typeof` / `instanceof` operators
- ✅ `in` operator (`key in obj`)
- ✅ Template literals (`` `${expr}` ``)
- ✅ try / catch / finally / throw
- ✅ `delete` operator

### Classes
- ✅ Classes with constructor, methods, inheritance (`extends`, `super`)
- ✅ Private fields and methods (`#field`, `#method()`)
- ✅ Static methods
- ✅ Classes extending built-in Error types

### Built-ins
- ✅ `Array`: push/pop/indexOf/includes/join/map/filter/forEach/reduce/find/findIndex/some/every/sort/flat/flatMap/length/splice/slice/concat/reverse
- ✅ `String`: replace/replaceAll/startsWith/endsWith/padStart/padEnd/charAt/charCodeAt/repeat/fromCharCode/slice/split/trim/toUpperCase/toLowerCase/includes/indexOf/substring
- ✅ `Object`: assign/create/fromEntries/keys/values/entries
- ✅ `Math`: abs/floor/ceil/round/sqrt/trunc/log/log2/log10/sin/cos/tan/min/max/pow/atan2/hypot/random
- ✅ `Map`: new/get/set/has/delete/clear/size/keys/values/entries/forEach
- ✅ `RegExp`: literal `/pattern/flags`, `new RegExp(str)`, `.test()`, `.exec()`, `str.match(re)`, `str.replace(re, s)`
- ✅ `JSON.stringify` / `JSON.parse`
- ✅ `Number()` / `String()` / `Boolean()` coercion
- ✅ `parseInt` / `parseFloat`
- ✅ `encodeURIComponent` / `decodeURIComponent` / `encodeURI` / `decodeURI`
- ✅ `Promise.resolve` / `Promise.reject` / `Promise.all`
- ✅ `Error` / `TypeError` / `RangeError` / `SyntaxError`
- ✅ `console.log` (multiple args, all types)

### Async / HTTP
- ✅ `async` / `await` (top-level async functions, tokio-backed)
- ✅ HTTP server via `serve(port, fetchFn)` — hyper 1.x + tokio
- ✅ `ts_request_new` / `ts_response_new` / `ts_response_clone`
- ✅ `ts_headers_new` / `ts_headers_append` / `ts_headers_get_set_cookie`
- ✅ Module-level state via `ts_set_module_global` / `ts_get_module_global`

### Hono Framework Compilation
- ✅ `hono/src/context.ts` — compiles and links successfully
- ✅ `hono/src/compose.ts` — compiles and links successfully
- ✗ `hono/src/hono-base.ts` — fails on `new URL(...)` constructor
- ✗ `hono/src/hono.ts` — fails on `new URL(...)` constructor

---

## Highest Impact Next Steps

1. **`new URL(href)`** and **`new URLSearchParams(str)`** — unblocks hono-base.ts and hono.ts compilation
2. **`Response.text()` / `Response.json()`** — needed for route handlers to read bodies
3. **`Headers.get()` full method dispatch** — needed for middleware that reads headers
4. **`addEventListener('fetch', handler)`** — real implementation for service-worker entry point
5. **Async class methods** — needed for Hono middleware class pattern
6. **Floating-point arithmetic** — all numbers currently truncated to i32
