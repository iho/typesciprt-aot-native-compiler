# Features Required to Compile Hono

Priority-ordered list of TypeScript/JS language features needed to compile
`hono/src/context.ts` (and its transitive dependencies) to a native binary.

---

## ✅ COMPLETED FEATURES

All features needed to compile `hono/src/context.ts` have been implemented.

### Core Language
- [x] Private class fields/methods (`#field`, `#method()`)
- [x] Closures (free-variable capture with env TsArray)
- [x] Rest parameters (`function foo(a, ...rest)`)
- [x] Destructuring rest: `const { a, ...rest } = obj` / `const [a, ...rest] = arr`
- [x] `for...of` over arrays, strings, Map entries
- [x] Destructuring assignment (object/array patterns, nested)
- [x] `??` nullish coalescing
- [x] `?.` optional chaining
- [x] `??=` / `||=` / `&&=` logical assignment operators
- [x] Default parameter values
- [x] Computed property assignment (`obj[expr] = val`, compound `obj[i] += val`)
- [x] `in` operator (`key in obj` → `ts_val_has_key`)
- [x] TypeScript method overloads (skip bodyless signatures)
- [x] Classes extending built-in Error (`new HTTPException extends Error`)
- [x] `Promise.resolve` / `Promise.reject` / `Promise.all` namespace calls
- [x] `Boolean(v)` coercion (`ts_coerce_bool`)
- [x] `Error(msg)` / `new Error(msg)` / `new TypeError(msg)` etc.

### Built-in Objects
- [x] `Map` built-in: new/get/set/has/delete/clear/size/keys/values/entries/forEach
- [x] `RegExp`: `/pattern/flags`, `new RegExp(s)`, `.test()`, `.exec()`, `str.match(re)`, `str.replace(re, s)`
- [x] `Error` built-in (as TsObject with `message` property)
- [x] `JSON.stringify` / `JSON.parse`
- [x] `Number(v)` / `String(v)` coercion
- [x] `Boolean(v)` coercion
- [x] `encodeURIComponent` / `decodeURIComponent` / `encodeURI` / `decodeURI`

### Runtime Functions (Web APIs - basic)
- [x] `ts_response_new` / `ts_response_clone`
- [x] `ts_request_new`
- [x] `ts_headers_new` / `ts_headers_append` / `ts_headers_get_set_cookie`
- [x] `ts_set_module_global` / `ts_get_module_global` (module-level state sharing)

### Array/String/Object Methods
- [x] Array: push/pop/indexOf/includes/join/map/filter/forEach/reduce/find/findIndex/some/every/sort/flat/flatMap
- [x] String: replace/replaceAll/startsWith/endsWith/padStart/padEnd/charAt/charCodeAt/repeat/fromCharCode/slice/split/trim/toUpperCase/toLowerCase/includes/indexOf
- [x] `Object.assign` / `Object.create` / `Object.fromEntries` / `Object.keys` / `Object.values` / `Object.entries`
- [x] `Math.*` (abs/floor/ceil/round/sqrt/trunc/log/log2/log10/sin/cos/tan/min/max/pow/atan2/hypot/random)
- [x] Spread in function calls (`fn(...arr)` via `ts_func_spread_call`)

### MLIR / Codegen Correctness
- [x] All scope variables normalized to i64 before merge/phi blocks in:
      if-else, while, for, for-of, for-in, try-catch, logical (&&/||/??), nullish coalescing
- [x] Function return type always i64 (required for heap object returns)
- [x] `fn_return_type` reset happens AFTER `terminate_with_return` (was a bug)
- [x] Duplicate class lowering guard (`lowered_classes` HashSet)
- [x] Duplicate file processing guard (`visited` PathBuf HashSet)

---

## Compilation Status

```
cargo run -p tscc -- hono/src/context.ts -o /tmp/context_test
# ✓  compiled to /tmp/context_test
```

**Test suite:** 49 tests, all pass
```
cargo test -p tscc -- --include-ignored --test-threads=1
# test result: ok. 49 passed; 0 failed
```

---

## Remaining Work (for full Hono app)

These features are needed beyond `context.ts` to build a working Hono HTTP server:

### Web APIs (full implementation)
- [ ] `Request` full API: `.text()`, `.json()`, `.formData()`, `.arrayBuffer()`, `.blob()`
- [ ] `Response` full API: `.text()`, `.json()`, `.status`, `.headers`, `.ok`, `.body`
- [ ] `Headers` full API: `.get()`, `.set()`, `.has()`, `.delete()`, `.forEach()`, `.entries()`
- [ ] `URL` / `URLSearchParams`
- [ ] `fetch()` global

### Hono-specific
- [ ] Async class methods (currently only async standalone functions work)
- [ ] `Symbol.iterator` / custom iterators
- [ ] `WeakMap` (used by some Hono middleware)
- [ ] Tagged template literals
