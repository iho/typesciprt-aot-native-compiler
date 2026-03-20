# TypeScript AOT Native Compiler

A TypeScript-to-native compiler that compiles TypeScript directly to native binaries via MLIR and LLVM — no VM, no JIT, no Node.js. The primary goal is running production TypeScript web frameworks (Hono, NestJS-style) as native HTTP servers backed by Rust's hyper and tokio.

Values are represented using NaN-boxing (`TsVal = i64`) and memory is managed with Automatic Reference Counting (ARC).

## Quick Start

### Prerequisites
- Rust 1.70+
- LLVM 21 (install via Homebrew on macOS: `brew install llvm`)
- macOS with ARM64 CPU (or update the env vars below for your LLVM path)

### Configure LLVM paths

Copy the example config and update the paths to your LLVM 21 installation:

```bash
cp .cargo/example.toml .cargo/config.toml
```

Then edit `.cargo/config.toml` and set the three environment variables to point at your LLVM prefix:

```toml
[env]
MLIR_SYS_210_PREFIX  = "/opt/homebrew/opt/llvm"   # ← update if different
LLVM_SYS_210_PREFIX  = "/opt/homebrew/opt/llvm"
TABLEGEN_210_PREFIX  = "/opt/homebrew/opt/llvm"
```

On a default Homebrew ARM64 Mac the paths above are correct. On Intel Mac or a custom install, replace `/opt/homebrew/opt/llvm` with the output of `brew --prefix llvm`.

### Build & Run

```bash
# Build the compiler
cargo build -p tscc

# Compile a TypeScript program
cargo run -p tscc -- examples/closures.ts -o my_program

# Run the generated binary
./my_program

# Run all tests (69 tests, requires LLVM)
cargo test -p tscc -- --include-ignored --test-threads=1
```

---

## Compilation Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                     TypeScript Source                            │
│                       input.ts                                   │
└──────────────────────────┬──────────────────────────────────────┘
                           │  OXC parser  (ts-frontend)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                        OXC AST                                   │
│  Program → FunctionDeclaration, ClassDeclaration, Expressions…   │
└──────────────────────────┬──────────────────────────────────────┘
                           │  ts-codegen (lowering/)
                           │
                           │  Pass 1:  collect_function_signatures
                           │           collect_class_definitions
                           │           collect_enum_definitions
                           │
                           │  Pass 2:  lower_class_declaration (methods)
                           │           lower_function_declaration
                           │           lower_imported_module_init
                           │
                           │  Pass 3:  lower_main_function (statements)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                MLIR (func / arith / cf / llvm dialects)          │
│                                                                  │
│  func.func @__ts_main(%arg0: i64, ...) -> i64 {                  │
│    %0 = func.call @ts_obj_new() : () -> i64                      │
│    %1 = func.call @ts_obj_set(%0, @"x", %arg0) : (...)           │
│    cf.br ^bb1(%0 : i64)                                          │
│  }                                                               │
└──────────────────────────┬──────────────────────────────────────┘
                           │  mlir-opt (passes in tscc/emit.rs)
                           │    - canonicalize
                           │    - convert-func-to-llvm
                           │    - convert-arith-to-llvm
                           │    - convert-cf-to-llvm
                           │    - finalize-memref-to-llvm
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                         LLVM IR                                  │
│  define i64 @__ts_main(i64 %arg0, ...) {                         │
│    %0 = call i64 @ts_obj_new()                                   │
│    call void @ts_obj_set(i64 %0, ptr @.str, i64 %arg0)           │
│    br label %bb1                                                  │
│  }                                                               │
└──────────────────────────┬──────────────────────────────────────┘
                           │  mlir-translate → llc  (or LLVM API)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                       Object File (.o)                           │
└──────────────────────────┬──────────────────────────────────────┘
                           │  clang  (links libts_runtime.a)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Native Binary                                │
│  Statically linked with ts-runtime (Rust, ~2 MB)                 │
│  Includes tokio async runtime for async/await, HTTP serving      │
└─────────────────────────────────────────────────────────────────┘
```

### How a TypeScript value flows

```
TypeScript:   const x = 42
                │
                ▼
OXC AST:      NumericLiteral(42)
                │  lower_numeric_literal(42)
                ▼
MLIR:         %x = arith.constant 72057594054180864 : i64
                │   ─────────────────────────────
                │   That constant = TAG_INT | 42
                │   TAG_INT = 0x7FFE_0000_0000_0000
                │   so 0x7FFE_0000_0000_002A = 72057594054180906
                ▼
Runtime:      TsVal(i64) — extracted with as_i32() → 42
```

---

## Architecture

### Crate Layout

```
crates/
├── tscc/              CLI driver
│   ├── main.rs        arg parsing, calls codegen + emit
│   └── emit.rs        MLIR → LLVM IR → object → link via clang
│
├── ts-frontend/       Thin OXC parser wrapper
│   └── lib.rs         parse_typescript() → OXC Program
│
├── ts-codegen/        AST → MLIR  (the bulk of the compiler)
│   └── lowering/
│       ├── mod.rs         Program lowering, function signatures,
│       │                  class/module init, declare_runtime_funcs
│       ├── expressions.rs All expression lowering: closures, builtins,
│       │                  call dispatch, object/array literals (~6000 lines)
│       ├── statements.rs  Statement lowering, lower_main_function,
│       │                  for-of/for-in, switch, destructuring
│       ├── operators.rs   Binary/unary/assignment/update operators,
│       │                  cell-based mutable captures
│       ├── classes.rs     Class declarations, constructors, inheritance,
│       │                  static fields, private fields/methods
│       ├── literals.rs    Object/array/template literal lowering
│       └── enums.rs       TypeScript enum definitions
│
└── ts-runtime/        Static Rust library linked into every binary
    ├── alloc.rs       ARC allocator (ts_alloc_rc)
    ├── console.rs     console.log implementation
    └── value/
        ├── mod.rs         TsVal type, NaN-boxing, heap tags, ts_retain/release_val
        ├── object.rs      TsObject: ts_obj_get/set/delete, getter/setter support
        ├── array.rs       TsArray: push/pop/map/filter/reduce/sort/…
        ├── string_val.rs  TsString: all string prototype methods
        ├── func.rs        TsFunction: closures, dispatch_callback, method calls
        ├── map.rs         TsMap: Map built-in
        ├── set.rs         TsSet: Set built-in
        ├── weak.rs        TsWeakMap, TsWeakSet
        ├── regexp.rs      TsRegExp: regex operations via `regex` crate
        ├── promise.rs     TsPromise: async/await on tokio multi-thread runtime
        ├── operators.rs   Math.*, Number conversions, comparison helpers
        ├── globals.rs     console, process, setTimeout/Interval, fetch
        ├── container.rs   Polymorphic dispatch: Map/Set/Array .keys/.values/…
        ├── date.rs        Date built-in
        ├── symbol.rs      Symbol built-in
        ├── uri.rs         encodeURIComponent / decodeURIComponent / …
        └── http.rs        TsHeaders, TsResponse, HTTP server (hyper)
```

### Value Representation (NaN-Boxing)

All TypeScript values are `TsVal = i64`. The upper 16 bits encode the type tag:

```
Floats:     any bit pattern where bits 51-62 are NOT all 1s  (standard IEEE 754)
────────────────────────────────────────────────────────────────────────────────
NaN space:  0x7FF?_????_????_????

TAG_UNDEFINED = 0x7FF8_0000_0000_0000   (quiet NaN base)
TAG_NULL      = 0x7FF9_0000_0000_0001
TAG_BOOL      = 0x7FFA_0000_0000_000?   (bit 0: 0=false, 1=true)
TAG_PTR       = 0x7FFC_????_????_????   (lower 48 bits: heap pointer)
TAG_INT       = 0x7FFE_0000_????_????   (lower 32 bits: i32 value)
```

Heap objects are all reference-counted (ARC). Each heap allocation has an `ArcHeader` prefix with ref-count and heap tag:

| Tag | Rust Type       | JavaScript Type                    |
|-----|-----------------|------------------------------------|
|  0  | TsObject        | {}, class instances, Error         |
|  1  | TsArray         | [], closure environment arrays     |
|  2  | TsString        | string                             |
|  3  | TsPromise       | Promise                            |
|  4  | TsFunction      | function, arrow, closure           |
|  5  | TsMap           | Map                                |
|  6  | TsRegExp        | RegExp                             |
|  7  | TsHeaders       | Headers                            |
|  8  | TsResponse      | Response                           |
|  9  | URLSearchParams | URLSearchParams                    |
| 10  | TsSymbol        | Symbol                             |
| 11  | TsSet           | Set                                |
| 12  | TsWeakMap       | WeakMap                            |
| 13  | TsWeakSet       | WeakSet                            |

### ARC Rules

- Every variable **read** produces an owned reference via `ts_retain_val`.
- Every `ExpressionStatement` result is released via `ts_release_val`.
- `ThisExpression` must call `ts_retain_val` (same as Identifier).
- Expression-body arrows (`(x) => expr`) preserve the value without extra release.

### Closures & Cell-Based Mutable Captures

Closures capture free variables into a `TsArray` environment:

```
function makeCounter() {
  let count = 0                   // count is "cell-ified" (single-element TsArray)
  return () => {
    count += 1                    // writes to the cell
    return count                  // reads from the cell
  }
}
```

When a variable is assigned inside a nested closure, it is wrapped in a single-element `TsArray` (a "cell"). All reads/writes go through `ts_arr_get(cell, 0)` / `ts_arr_set(cell, 0, val)`.

### Async/Await

`async function` compiles to a function returning `i64` (a `TsPromise`). The tokio multi-thread runtime is initialized at program startup. `ts_promise_resolve(val)` and `ts_promise_await(expr)` bridge synchronous codegen to tokio.

---

## Supported Language Features

### Primitives & Variables
```typescript
let x = 42;          const y = 3.14;      const flag = true;
const name = "hello"; const nothing = null; let undef: undefined;
```

### Arithmetic & Operators
```typescript
2 + 3;    10 - 4 * 2;    20 / 4;    7 % 3;    2 ** 8;
typeof x;    x instanceof Foo;    "key" in obj;    delete obj.key;
~x;  x << 2;  x >> 2;  x >>> 2;  x & 3;  x | 3;  x ^ 3;  // bitwise
```

### Comparisons & Logic
```typescript
a === b;  a !== b;  a < b;  a <= b;  a > b;  a >= b;
a && b;   a || b;   !a;
a ?? b;              // nullish coalescing
a &&= b;  a ||= b;  a ??= b;  // logical assignment
```

### Control Flow
```typescript
if (cond) { ... } else { ... }
while (cond) { ... }
do { ... } while (cond);
for (let i = 0; i < 10; i++) { ... }
for (const v of arr) { ... }          // arrays, strings, Map entries
for (const k in obj) { ... }          // object keys
switch (val) { case 1: ...; break; default: ...; }
cond ? a : b;                          // ternary
break; continue;   break label; continue label;  // labeled
try { ... } catch (e) { ... } finally { ... }
throw new Error("msg");
```

### Functions & Closures
```typescript
function add(a: number, b: number) { return a + b; }
function greet(name = "world") { return `Hello, ${name}!`; }     // defaults
function sum(...args: number[]) { return args.reduce(...); }      // rest
const double = (x: number) => x * 2;                             // arrow
function makeAdder(x: number) { return (y: number) => x + y; }  // closure

// Async/await (tokio-backed)
async function fetchData() { const r = await somePromise; return r; }

// Function.prototype.bind
const bound = fn.bind(thisArg, arg1);
```

### Template Literals
```typescript
const msg = `Hello, ${name}! Count: ${count + 1}`;
String.raw`raw\nstring`;   // tagged templates
```

### Destructuring
```typescript
const [a, b = 10, ...rest] = [1, undefined, 3, 4];  // array + defaults + rest
const { x, y: renamed, z = 0 } = obj;                // object + rename + default
const { a, ...others } = obj;                         // object rest
for (const [k, v] of map.entries()) { }              // in for-of
```

### Optional Chaining & Nullish
```typescript
const val = obj?.prop?.nested;
const safe = maybeNull ?? "default";
obj?.method?.();
arr?.[0];
```

### Arrays
```typescript
const arr = [1, 2, 3];
arr.push(4);         arr.pop();         arr.shift();       arr.unshift(0);
arr.length;          arr[0];            arr.at(-1);
arr.indexOf(2);      arr.includes(3);   arr.lastIndexOf(2);
arr.join(",");       arr.slice(1, 3);   arr.splice(1, 1);
arr.map(x => x*2);   arr.filter(x => x>1);   arr.reduce((acc,x) => acc+x, 0);
arr.forEach(x => {}); arr.find(x => x>2);      arr.findIndex(x => x>2);
arr.findLast(x => x<3);  arr.findLastIndex(x => x<3);
arr.some(x => x>2);  arr.every(x => x>0);
arr.sort((a,b) => a-b);  arr.reverse();   arr.flat(2);   arr.flatMap(x => [x]);
arr.fill(0, 1, 3);   arr.copyWithin(0, 2);
arr.keys();  arr.values();  arr.entries();
arr.reduceRight((acc,x) => acc+x, 0);
arr.toSorted();   arr.toReversed();   arr.with(1, 99);
arr.concat([4, 5]);
[...arr, 4, 5];      fn(...arr);        // spread
Array.from(iterable);   Array.isArray(x);   Array.of(1, 2, 3);
```

### Objects
```typescript
const obj = { x: 1, y: 2, name: "test" };
const obj2 = {
  get value() { return this._v; },   // getter
  set value(v) { this._v = v; },     // setter
};
obj.x;   obj["y"];   obj.x = 10;
const { x, y } = obj;               // destructuring
const { a = 10, ...rest } = obj;    // defaults + rest
const copy = { ...obj, z: 3 };      // spread

Object.keys(obj);  Object.values(obj);  Object.entries(obj);
Object.assign(target, source);
Object.create(proto);
Object.fromEntries(entries);
Object.freeze(obj);   Object.seal(obj);
Object.is(a, b);      Object.hasOwn(obj, key);
Object.getPrototypeOf(obj);
Object.getOwnPropertyNames(obj);
Object.defineProperty(obj, key, descriptor);
```

### Strings
```typescript
str.length;
str.indexOf("x");    str.lastIndexOf("x");   str.includes("x");
str.slice(1, 5);     str.substring(1, 4);    str.at(-1);
str.toUpperCase();   str.toLowerCase();       str.trim();
str.trimStart();     str.trimEnd();
str.split(",");      str.replace("a","b");   str.replaceAll("a","b");
str.startsWith("x"); str.endsWith("x");
str.padStart(5,"0"); str.padEnd(5,"_");
str.charAt(0);       str.charCodeAt(0);       str.repeat(3);
str.match(/\d+/g);   str.matchAll(/\d+/g);    str.search(/\d+/);
str.concat(other);   str.localeCompare(other);
String.fromCharCode(65);
```

### Map
```typescript
const m = new Map();
const m2 = new Map([[k1, v1], [k2, v2]]);   // initial entries
m.set("key", 42);   m.get("key");    m.has("key");
m.delete("key");    m.size;          m.clear();
for (const [k, v] of m.entries()) { }
m.keys();  m.values();  m.forEach((v, k) => { });
```

### Set
```typescript
const s = new Set([1, 2, 3]);
s.add(4);   s.has(2);   s.delete(1);   s.size;   s.clear();
s.keys();   s.values();  s.entries();
s.forEach(v => { });
```

### WeakMap / WeakSet
```typescript
const wm = new WeakMap();
wm.set(obj, value);   wm.get(obj);   wm.has(obj);   wm.delete(obj);

const ws = new WeakSet();
ws.add(obj);   ws.has(obj);   ws.delete(obj);
```

### RegExp
```typescript
const re = /foo(\w+)/gi;
const re2 = new RegExp("foo(\\w+)", "gi");
re.test("foobar");          // true
re.exec("foobar");          // ["foobar", "bar"]
str.match(/\d+/g);
str.matchAll(/\d+/g);
str.replace(/foo/g, "bar");
str.search(/\d+/);
```

### Math & Number
```typescript
Math.abs(-5);    Math.floor(3.7);   Math.ceil(3.2);    Math.round(3.5);
Math.sqrt(16);   Math.pow(2, 8);    Math.min(1, 2);    Math.max(1, 2);
Math.sin(x);     Math.cos(x);       Math.tan(x);       Math.atan2(y, x);
Math.log(x);     Math.log2(x);      Math.log10(x);     Math.random();
Math.trunc(3.7); Math.hypot(3, 4);  Math.sign(-5);     Math.cbrt(8);
Math.asin(x);    Math.acos(x);      Math.atan(x);
Math.sinh(x);    Math.cosh(x);      Math.tanh(x);
Math.exp(x);     Math.expm1(x);     Math.log1p(x);
Math.clz32(x);   Math.fround(x);    Math.imul(a, b);
Math.PI;  Math.E;  Math.LN2;  Math.SQRT2;

Number(val);    parseInt("42");    parseFloat("3.14");
isNaN(x);       isFinite(x);
Number.isInteger(x);  Number.isFinite(x);  Number.isNaN(x);
Number.isSafeInteger(x);  Number.parseInt("42");  Number.parseFloat("3.14");
Number.MAX_VALUE;  Number.MIN_VALUE;  Number.EPSILON;
Number.MAX_SAFE_INTEGER;  Number.MIN_SAFE_INTEGER;
(42).toFixed(2);  (3.14159).toPrecision(4);  (1234).toExponential(2);
```

### JSON
```typescript
JSON.stringify(obj);   JSON.parse(str);
```

### Promise / Async
```typescript
Promise.resolve(val);   Promise.reject(err);
Promise.all([p1, p2]);   Promise.allSettled([p1, p2]);
Promise.race([p1, p2]);  Promise.any([p1, p2]);
queueMicrotask(() => { });
```

### Classes
```typescript
class Animal {
  #name: string;          // private field
  static count = 0;       // static field

  constructor(name: string) {
    this.#name = name;
    Animal.count++;
  }

  #validate() { ... }     // private method
  speak() { return this.#name; }
  static create(n: string) { return new Animal(n); }
}

class Dog extends Animal {
  constructor(name: string) { super(name); }
  bark() { return "Woof!"; }
}

// Class decorators
@Controller("/api")
class UserController {
  @Get("/users")
  getUsers() { return []; }
}
```

### TypeScript Enums
```typescript
enum Direction { Up, Down, Left, Right }
enum Status { Active = 1, Inactive = 2 }
const d = Direction.Up;   // 0
```

### Error Handling
```typescript
try {
  throw new Error("oops");
} catch (e) {
  console.log((e as Error).message);
} finally {
  console.log("cleanup");
}
throw new TypeError("bad type");
throw new RangeError("out of range");
```

### Date
```typescript
const now = Date.now();
const d = new Date();
d.getFullYear();  d.getMonth();  d.getDate();  d.getDay();
d.getHours();     d.getMinutes(); d.getSeconds();
d.toISOString();  d.toLocaleDateString();
```

### Symbol
```typescript
const sym = Symbol("description");
const sym2 = Symbol.for("global");
```

### Globals
```typescript
console.log("hello", 42, true, [1,2], {x:1});
console.error("err");   console.warn("warn");

setTimeout(() => { }, 1000);
setInterval(() => { }, 500);
clearTimeout(id);    clearInterval(id);

process.exit(0);
process.argv;        // string array
process.env;         // object of env vars

structuredClone(val);

encodeURIComponent("hello world");   decodeURIComponent("hello%20world");
encodeURI(url);   decodeURI(encoded);
```

### HTTP Server
```typescript
// Low-level built-in HTTP server (hyper + tokio)
serve(3000, async (req: Request) => {
  const url = new URL(req.url);
  const name = url.searchParams.get("name") ?? "world";
  return new Response(`Hello, ${name}!`, { status: 200 });
});
```

### Hono (unmodified)

The compiler can compile and run the [Hono](https://hono.dev/) web framework from source without modifications:

```typescript
import { Hono } from './hono/src/index'

const app = new Hono()
app.get('/', (c) => c.text('Hello World'))
app.get('/hello/:name', (c) => c.text(`Hello ${c.req.param('name')}!`))

Deno.serve({ port: 8080 }, app.fetch)
```

```bash
cargo run -p tscc -- my_app.ts -o my_app
./my_app
curl http://localhost:8080/        # Hello World
curl http://localhost:8080/hello/Alice  # Hello Alice!
```

---

## Project Structure

```
.
├── crates/
│   ├── tscc/              # Compiler driver & CLI
│   ├── ts-frontend/       # OXC parser wrapper
│   ├── ts-codegen/        # AST → MLIR lowering
│   │   └── lowering/
│   │       ├── mod.rs         # Program & function lowering, runtime decls
│   │       ├── expressions.rs # All expression lowering (~6000 lines)
│   │       ├── statements.rs  # Statement lowering
│   │       ├── operators.rs   # Operators, cell captures
│   │       ├── literals.rs    # Object/array/template literals
│   │       ├── classes.rs     # Class declarations
│   │       └── enums.rs       # TypeScript enums
│   └── ts-runtime/        # Static runtime library (Rust)
│       ├── alloc.rs        # ARC allocator
│       ├── console.rs      # console.log
│       └── value/          # All runtime functions (no_mangle C ABI)
├── examples/              # Example TypeScript programs
└── hono/                  # Hono framework (compilation target)
```

---

## CLI

```bash
tscc [OPTIONS] <input.ts>

OPTIONS:
  -o, --output <PATH>   Output file path (default: strips .ts extension)
  --emit-mlir           Print MLIR and exit
  --emit-llvm           Print LLVM IR and exit
  -O <LEVEL>            Optimization level: 0-3 (default: 2)
  -v, --verbose         Enable debug logging
  -h, --help            Show help
```

Debugging:
```bash
cargo run -p tscc -- input.ts --emit-mlir   # dump MLIR before passes
cargo run -p tscc -- input.ts --emit-llvm   # dump LLVM IR
DUMP_MLIR=1 cargo run -p tscc -- input.ts   # dump to /tmp/debug.mlir
```

---

## Benchmarks

Compared against Node.js v22 on Apple M-series — Vendure-like REST API (`examples/vendure_bench.ts`), 50 concurrent connections, 10s runs with [`wrk`](https://github.com/wg/wrk):

| Endpoint              | AOT (tscc) req/s | Node.js req/s | Ratio          |
|-----------------------|-----------------|---------------|----------------|
| `GET /health`         | 82,902          | 45,353        | **1.83× AOT**  |
| `GET /api/products`   | 66,129          | 38,576        | **1.71× AOT**  |
| `GET /api/products/1` | 79,014          | 42,801        | **1.85× AOT**  |
| `GET /api/orders`     | 70,862          | 45,845        | **1.55× AOT**  |

AOT native compilation is consistently 1.5–1.85× faster than Node.js across all endpoints.

Run benchmarks yourself:
```bash
./benchmarks/bench.sh
```

---

## Testing

```bash
# Run all 86 integration tests (requires LLVM)
cargo test -p tscc -- --include-ignored --test-threads=1

# Run a single test
cargo test -p tscc closures -- --include-ignored --test-threads=1
```

Always use `--test-threads=1` to avoid race conditions in concurrent LLVM codegen.

---

## What's Not Yet Implemented

**Language semantics:**
- Lazy generator semantics — `function*` / `yield` compiles but eagerly collects all yielded values into an array; infinite or stateful generators won't work correctly
- Rejected promise propagation — `await Promise.reject(err)` inside a `try/catch` does not throw; rejection is silently swallowed
- `Proxy` and full `Reflect` API (only `Reflect.defineMetadata` / `getMetadata` are implemented)
- Custom `Symbol.iterator` protocols — `for...of` works on `Array`, `String`, `Map`, `Set`, and `Map.entries()` but not on user-defined iterables
- Dynamic `import()` expressions — only static `import` declarations are supported
- True weak-reference semantics — `WeakRef`, `WeakMap`, `WeakSet` are implemented but use strong (ARC) references, so they don't enable GC-style lifecycle management
- `eval()` and `new Function()` — not possible in an AOT compiler

**Standard library gaps:**
- `Intl` APIs (locale-aware formatting, collation, etc.)
- `console.group` / `console.table` / `console.time` / `console.timeEnd`
- `Object.defineProperty` with getter/setter descriptors on plain objects (class getters/setters work)
- `RegExp` named capture groups and lookbehind assertions

**Tooling:**
- Source maps / debug info — generated binaries have no DWARF; stack traces are Rust-level
- Single-file output only — no tree-shaking or bundle splitting

---

## References

- [MLIR Documentation](https://mlir.llvm.org/)
- [OXC Parser](https://github.com/web-infra-dev/oxc)
- [melior Crate](https://github.com/mlir-rs/melior)
- [Hono Framework](https://hono.dev/)
- [hyper HTTP](https://hyper.rs/)
