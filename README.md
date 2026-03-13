# TypeScript AOT Native Compiler

A TypeScript-to-native compiler that compiles TypeScript directly to native binaries via MLIR and LLVM — no VM, no JIT, no Node.js. The primary goal is running production TypeScript web frameworks (Hono) as native HTTP servers backed by Rust's hyper and tokio.

Values are represented using NaN-boxing (`TsVal = i64`) and memory is managed with Automatic Reference Counting (ARC).

## Quick Start

### Prerequisites
- Rust 1.70+
- LLVM 21 (install via Homebrew on macOS: `brew install llvm`)
- macOS with ARM64 CPU (or modify `.cargo/config.toml` for your architecture)

### Build & Run

```bash
# Build the compiler
cargo build -p tscc

# Compile a TypeScript program
cargo run -p tscc -- examples/closures.ts -o my_program

# Run the generated binary
./my_program

# Run all tests
cargo test -p tscc -- --include-ignored --test-threads=1
```

## Supported Language Features

### Primitives & Variables
```typescript
let x = 42;
const y = 3.14;
const flag = true;
const name = "hello";
const nothing = null;
let undef: undefined;
```

### Arithmetic & Operators
```typescript
2 + 3;          // integer add (also string concat)
10 - 4 * 2;     // precedence respected
20 / 4;         // division
7 % 3;          // modulo
2 ** 8;         // exponentiation (256)
typeof x;       // typeof
x instanceof Foo; // instanceof
"key" in obj;   // in operator
delete obj.key; // delete
```

### Comparisons & Logic
```typescript
a === b;  a !== b;  a < b;  a <= b;  a > b;  a >= b;
a && b;   a || b;   !a;
a ?? b;            // nullish coalescing
a &&= b;  a ||= b;  a ??= b;  // logical assignment
```

### Control Flow
```typescript
if (cond) { ... } else { ... }
while (cond) { ... }
for (let i = 0; i < 10; i++) { ... }
for (const v of arr) { ... }   // arrays, strings, Map entries
for (const k in obj) { ... }   // object keys
cond ? a : b;                   // ternary
break; continue;                // loop control
try { ... } catch (e) { ... } finally { ... }
throw new Error("msg");
```

### Functions & Closures
```typescript
// Regular function
function add(a: number, b: number) { return a + b; }

// Default parameters
function greet(name: string = "world") { return `Hello, ${name}!`; }

// Rest parameters
function sum(...args: number[]) { return args.reduce((a, b) => a + b, 0); }

// Arrow functions (capture outer scope)
const double = (x: number) => x * 2;
function makeAdder(x: number) { return (y: number) => x + y; }
const add5 = makeAdder(5);
add5(3); // 8

// Nested function declarations (hoisted at declaration position)
function makeCounter() {
  let count = 0;
  function increment() { count = count + 1; return count; }
  return increment;
}

// Recursive inner functions
function makeFactorial() {
  function factorial(n: number): number {
    return n <= 1 ? 1 : n * factorial(n - 1);
  }
  return factorial;
}

// Async/await (tokio-backed)
async function fetchData() {
  const result = await somePromise;
  return result;
}
```

### Template Literals
```typescript
const msg = `Hello, ${name}! Count: ${count + 1}`;
```

### Arrays
```typescript
const arr = [1, 2, 3];
arr.push(4); arr.pop();
arr.length;  arr[0];
arr.indexOf(2);   arr.includes(3);
arr.join(",");    arr.slice(1, 3);
arr.map(x => x * 2);
arr.filter(x => x > 1);
arr.reduce((acc, x) => acc + x, 0);
arr.forEach(x => console.log(x));
arr.find(x => x > 2);      arr.findIndex(x => x > 2);
arr.some(x => x > 2);      arr.every(x => x > 0);
arr.sort((a, b) => a - b);
arr.flat(2);                arr.flatMap(x => [x, x * 2]);
[...arr, 4, 5];             // spread
fn(...arr);                 // spread in call
```

### Objects
```typescript
const obj = { x: 1, y: 2, name: "test" };
obj.x;          obj["y"];
obj.x = 10;     obj["y"] = 20;
const { x, y } = obj;                    // destructuring
const { a = 10, ...rest } = obj;         // defaults + rest
const copy = { ...obj, z: 3 };           // spread

Object.assign(target, source);
Object.create(proto);
Object.fromEntries(entries);
Object.keys(obj); Object.values(obj); Object.entries(obj);
```

### Destructuring
```typescript
const [a, b, c] = [1, 2, 3];             // array
const [head, ...tail] = arr;             // rest
const { x, y } = point;                  // object
const { a: renamed, b: { c } } = nested; // rename + nested
for (const [k, v] of map.entries()) { }  // in for-of
```

### Optional Chaining & Nullish
```typescript
const val = obj?.prop?.nested;
const safe = maybeNull ?? "default";
obj?.method?.();
arr?.[0];
```

### String Methods
```typescript
str.indexOf("x");     str.includes("x");     str.slice(1, 5);
str.toUpperCase();    str.toLowerCase();      str.trim();
str.split(",");       str.replace("a", "b");  str.replaceAll("a", "b");
str.startsWith("x"); str.endsWith("x");
str.padStart(5, "0"); str.padEnd(5, "_");
str.charAt(0);        str.charCodeAt(0);      str.repeat(3);
str.substring(1, 4);
String.fromCharCode(65);
```

### Map
```typescript
const m = new Map();
m.set("key", 42);
m.get("key");         // 42
m.has("key");         // true
m.delete("key");
m.size;               m.clear();
for (const [k, v] of m.entries()) { }
m.keys();  m.values();  m.forEach((v, k) => { });
```

### RegExp
```typescript
const re = /foo(\w+)/gi;
const re2 = new RegExp("foo(\\w+)", "gi");
re.test("foobar");          // true
re.exec("foobar");          // ["foobar", "bar", ...]
str.match(/\d+/g);
str.replace(/foo/g, "bar");
```

### Math & Number
```typescript
Math.abs(-5);    Math.floor(3.7);  Math.ceil(3.2);   Math.round(3.5);
Math.sqrt(16);   Math.pow(2, 8);   Math.min(1, 2);   Math.max(1, 2);
Math.sin(x);     Math.cos(x);      Math.tan(x);      Math.atan2(y, x);
Math.log(x);     Math.log2(x);     Math.log10(x);    Math.random();
Math.trunc(3.7); Math.hypot(3, 4); Math.sqrt(25);
Number(val);     parseInt("42");   parseFloat("3.14");
```

### JSON
```typescript
JSON.stringify(obj);  // → string
JSON.parse(str);       // → value
```

### Classes
```typescript
class Animal {
  #name: string;        // private field

  constructor(name: string) { this.#name = name; }

  #validate() { ... }   // private method
  speak() { return this.#name; }
  static create(n: string) { return new Animal(n); }
}

class Dog extends Animal {
  constructor(name: string) { super(name); }
  bark() { return "Woof!"; }
}

class AppError extends Error {
  constructor(message: string) { super(message); }
}
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

### console.log
```typescript
console.log("hello", 42, true, null, [1, 2], { x: 1 });
```

### Encoding
```typescript
encodeURIComponent("hello world");   // "hello%20world"
decodeURIComponent("hello%20world"); // "hello world"
encodeURI(url);  decodeURI(encoded);
```

### Async & HTTP Server
```typescript
// Built-in HTTP server (hyper + tokio)
serve(3000, async (req: Request) => {
  return new Response("Hello!", { status: 200 });
});
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
│   │       ├── mod.rs         # Program & function lowering
│   │       ├── expressions.rs # Expression lowering, closures, builtins
│   │       ├── statements.rs  # Statement lowering
│   │       ├── operators.rs   # Binary/unary/assignment operators
│   │       ├── literals.rs    # Literal values
│   │       ├── classes.rs     # Class declarations
│   │       └── enums.rs       # TypeScript enums
│   └── ts-runtime/        # Static runtime library (Rust)
│       ├── value.rs        # TsVal, NaN-boxing, ARC, all runtime fns
│       ├── alloc.rs        # ARC allocator
│       └── console.rs      # console.log implementation
├── examples/              # Example TypeScript programs
├── hono/                  # Hono framework (compilation target)
├── PLAN.md                # Development roadmap
├── missing.md             # Unimplemented features
├── HONO_FEATURES.md       # Hono-specific feature tracker
└── README.md
```

---

## Compilation Pipeline

```
TypeScript Source
       ↓  OXC parser
    OXC AST
       ↓  ts-codegen (lowering/)
  MLIR (func/arith/cf/llvm dialects)
       ↓  mlir-opt (canonicalize + convert-to-llvm)
    LLVM IR
       ↓  mlir-translate + llc
   Object file
       ↓  clang (links ts-runtime staticlib)
  Native Binary
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

---

## Value Representation

All TypeScript values are `i64` using **NaN-boxing**:

| Type      | Encoding                                       |
|-----------|------------------------------------------------|
| Integer   | `TAG_INT (0x7FFE_...) \| value (lower 32-bit)` |
| Pointer   | `TAG_PTR (0x7FFC_...) \| ptr (lower 48-bit)`   |
| Boolean   | `TAG_BOOL (0x7FF8_0002_...) \| (0 or 1)`        |
| null      | `0x7FF9_0000_0000_0001`                         |
| undefined | `0x7FF8_0000_0000_0000`                         |

Heap objects (Object, Array, String, Function, Map, Promise, RegExp) are ref-counted via ARC. Every variable read produces an owned reference (`ts_retain_val`); temporaries are released after use (`ts_release_val`).

### Heap Tags

| Tag | Type       | Description                          |
|-----|------------|--------------------------------------|
| 0   | TsObject   | JS objects, classes, Request, Error  |
| 1   | TsArray    | JS arrays, closure env arrays        |
| 2   | TsString   | Interned strings                     |
| 3   | TsPromise  | async/await promises                 |
| 4   | TsFunction | Function pointer + env (closures)    |
| 5   | TsMap      | Map built-in                         |
| 6   | TsRegExp   | RegExp                               |
| 7   | TsHeaders  | Headers (same layout as TsMap)       |
| 8   | TsResponse | HTTP Response with status/body       |

---

## Testing

```bash
# Run all tests (49 tests, requires LLVM)
cargo test -p tscc -- --include-ignored --test-threads=1

# Run specific test file
cargo test -p tscc --test closures -- --include-ignored
```

Always use `--test-threads=1` to avoid race conditions in concurrent LLVM codegen.

---

## What's Not Yet Implemented

See [missing.md](./missing.md) for the full list. Key gaps:

- `new URL(href)` / `URLSearchParams` — needed for Hono routing
- `Response.text()` / `Response.json()` — needed for HTTP body handling
- `fetch()` global — no HTTP client yet
- Async class methods — class methods run synchronously
- Floating-point arithmetic — numbers use i32 truncation
- `switch` / `case` — not implemented
- `WeakMap`, `Symbol`, generators — not implemented

---

## References

- [MLIR Documentation](https://mlir.llvm.org/)
- [OXC Parser](https://github.com/web-infra-dev/oxc)
- [melior Crate](https://github.com/raparic/melior)
- [Hono Framework](https://hono.dev/)
- [hyper HTTP](https://hyper.rs/)
