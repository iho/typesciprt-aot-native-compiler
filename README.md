# TypeScript AOT Native Compiler

A TypeScript-to-native compiler that compiles TypeScript directly to native binaries via MLIR and LLVM, with no VM or JIT. Values are represented using NaN-boxing (`TsVal = i64`) and memory is managed with Automatic Reference Counting (ARC).

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
./target/debug/tscc examples/closures.ts -o my_program

# Run the generated binary
./my_program
echo $?  # Exit code is the value of the last expression
```

## Language Features

### Primitives & Variables
```typescript
let x = 42;
const y = 3.14;
const flag = true;
const name = "hello";
const nothing = null;
```

### Arithmetic & Operators
```typescript
2 + 3;          // integer add (also string concat if either is string)
10 - 4 * 2;     // precedence respected
20 / 4;         // division
7 % 3;          // modulo
2 ** 8;         // exponentiation
```

### Comparisons & Logic
```typescript
a === b;  a !== b;  a < b;  a <= b;  a > b;  a >= b;
a && b;   a || b;   !a;
a ?? b;           // nullish coalescing
a &&= b;  a ||= b;  a ??= b;  // logical assignment
```

### Control Flow
```typescript
if (cond) { ... } else { ... }
while (cond) { ... }
for (let i = 0; i < 10; i++) { ... }
for (const v of arr) { ... }
for (const k in obj) { ... }
cond ? a : b;     // ternary
break; continue;  // loop control
```

### Functions
```typescript
function add(a: number, b: number) {
  return a + b;
}

// Default parameters
function greet(name: string = "world") {
  return name;
}

// Async/await
async function fetchData() {
  const result = await somePromise;
  return result;
}
```

### Arrow Functions & Closures
```typescript
const double = (x: number) => x * 2;

// Closures capture outer variables
function makeAdder(x: number) {
  return (y: number) => x + y;
}
const add5 = makeAdder(5);
add5(3); // 8
```

### Arrays
```typescript
const arr = [1, 2, 3];
arr.push(4);
arr.pop();
arr.length;
arr.indexOf(2);
arr.includes(3);
arr.join(",");
arr.map(x => x * 2);
arr.filter(x => x > 1);
arr.reduce((acc, x) => acc + x, 0);
arr.forEach(x => console.log(x));
arr.find(x => x > 2);
arr.some(x => x > 2);
arr.every(x => x > 0);
arr.flat(2);
arr.flatMap(x => [x, x * 2]);
[...arr, 4, 5];  // spread
```

### Objects
```typescript
const obj = { x: 1, y: 2 };
obj.x;
obj["y"];
const { x, y } = obj;           // destructuring
const { a = 10, ...rest } = obj; // with defaults and rest (partial)
Object.assign(target, source);
Object.create(proto);
Object.fromEntries(entries);
```

### Destructuring
```typescript
const [a, b, c] = [1, 2, 3];    // array destructuring
const { x, y } = point;          // object destructuring
```

### Template Literals
```typescript
const msg = `Hello, ${name}! You are ${age} years old.`;
```

### Optional Chaining & Nullish
```typescript
const val = obj?.prop?.nested;
const safe = maybeNull ?? "default";
```

### String Methods
```typescript
str.indexOf("x");    str.includes("x");
str.slice(1, 5);     str.toUpperCase();  str.toLowerCase();
str.trim();          str.split(",");
str.replace("a","b"); str.replaceAll("a","b");
str.startsWith("x"); str.endsWith("x");
str.padStart(5,"0"); str.padEnd(5,"_");
str.charAt(0);       str.charCodeAt(0);  str.repeat(3);
String.fromCharCode(65);
```

### Map
```typescript
const m = new Map();
m.set("key", 42);
m.get("key");       // 42
m.has("key");       // true
m.delete("key");
m.size;
m.keys();
m.values();
```

### Classes
```typescript
class Animal {
  #name: string;       // private field

  constructor(name: string) {
    this.#name = name;
  }

  #validate() { ... } // private method

  speak() {
    return this.#name;
  }
}

class Dog extends Animal {
  constructor(name: string) {
    super(name);
  }
}
```

### Error Handling
```typescript
try {
  throw new Error("oops");
} catch (e) {
  console.log("caught");
} finally {
  console.log("cleanup");
}
```

### console.log
```typescript
console.log("hello", 42, true);
```

## Project Structure

```
.
├── crates/
│   ├── tscc/              # Compiler driver & CLI
│   ├── ts-frontend/       # OXC parser wrapper
│   ├── ts-codegen/        # AST → MLIR lowering
│   │   └── lowering/      # Lowering modules (expressions, statements, classes, …)
│   └── ts-runtime/        # Runtime library (ARC, NaN-boxing, builtins)
│       ├── value.rs        # TsVal, all runtime functions
│       ├── alloc.rs        # ARC allocator
│       └── console.rs      # console.log implementation
├── examples/              # Example TypeScript programs
└── README.md
```

## Compilation Pipeline

```
TypeScript Source
       ↓  OXC parser
    OXC AST
       ↓  ts-codegen (lowering/)
  MLIR (func/arith/cf/llvm dialects)
       ↓  mlir-opt + mlir-translate
    LLVM IR
       ↓  llc
   Object file
       ↓  clang (links ts-runtime)
  Native Binary
```

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

## Value Representation

All TypeScript values are represented as `i64` using **NaN-boxing**:

| Type      | Encoding                                  |
|-----------|-------------------------------------------|
| Integer   | `0x7FFE_0000_0000_0000 \| value`          |
| Pointer   | `0x7FFC_0000_0000_0000 \| ptr (48-bit)`   |
| Boolean   | `0x7FF8_0002_0000_0000 \| (0 or 1)`       |
| null      | `0x7FF9_0000_0000_0000`                   |
| undefined | `0x7FF8_0000_0000_0000`                   |

Heap objects (Object, Array, String, Function, Map) are ref-counted via ARC. Every value read from scope produces an owned reference; temporaries are released after use.

## Testing

```bash
# Run all tests (requires LLVM)
cargo test -p tscc -- --include-ignored --test-threads=1
```

## Building from Source

```bash
git clone <repo-url>
cd typesciprt-aot-native-compiler
cargo build -p tscc
./target/debug/tscc examples/closures.ts -o out && ./out
```

## License

MIT

## References

- [MLIR Documentation](https://mlir.llvm.org/)
- [OXC Parser](https://github.com/web-infra-dev/oxc)
- [melior Crate](https://github.com/raparic/melior)
- [LLVM Documentation](https://llvm.org/docs/)
