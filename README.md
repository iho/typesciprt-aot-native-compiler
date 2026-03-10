# TypeScript AOT Native Compiler

A TypeScript-to-native compiler that generates high-performance binaries via MLIR and LLVM. Write TypeScript, compile to native code.

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
./target/debug/tscc examples/arithmetic.ts

# Run the generated binary
./examples/arithmetic
echo $?  # Shows: 5 (result of 2 + 3)
```

## Language Features

### Arithmetic
```typescript
2 + 3;          // Addition
10 - 4;         // Subtraction
5 * 6;          // Multiplication
20 / 4;         // Integer division
```

### Variables
```typescript
let x = 10;
const y = 20;
var z = x + y;
z;              // Returns 30
```

### Complex Expressions
```typescript
let a = 2;
let b = 3;
let c = a + b;
c * 2;          // Returns 10 (via exit code)
```

### Return Values
The last expression in the program is returned as the exit code (0-255).

## Project Structure

```
.
├── crates/
│   ├── tscc/              # Compiler driver & CLI
│   │   ├── main.rs        # CLI and pipeline orchestration
│   │   └── emit.rs        # MLIR→LLVM→Binary translation
│   ├── ts-frontend/       # Parser
│   │   └── lib.rs         # OXC parser wrapper
│   ├── ts-codegen/        # Code generation
│   │   ├── lib.rs         # Main module
│   │   ├── lowering.rs    # AST → MLIR lowering
│   │   ├── passes.rs      # MLIR pass pipeline
│   │   └── context.rs     # MLIR context setup
│   └── ts-runtime/        # Runtime (future)
├── examples/              # Example programs
├── .cargo/config.toml     # LLVM configuration
├── Cargo.toml             # Workspace manifest
├── STATUS.md              # Current development status
└── README.md              # This file
```

## Compilation Pipeline

```
TypeScript Source
       ↓
┌──────────────────────┐
│  Parse (OXC)        │  → OXC AST
└──────────────────────┘
       ↓
┌──────────────────────┐
│  Lower to MLIR      │  → arith/func dialects
└──────────────────────┘
       ↓
┌──────────────────────┐
│  Optimize           │  → canonicalize, lowering passes
└──────────────────────┘
       ↓
┌──────────────────────┐
│  MLIR → LLVM IR     │  → mlir-translate
└──────────────────────┘
       ↓
┌──────────────────────┐
│  LLVM → Object Code │  → llc
└──────────────────────┘
       ↓
┌──────────────────────┐
│  Link to Binary     │  → clang/ld
└──────────────────────┘
       ↓
   Native Binary
```

## CLI Usage

```bash
tscc [OPTIONS] <input.ts>

OPTIONS:
  -o, --output <PATH>     Output file path (default: replaces .ts)
  --emit-mlir            Print MLIR and exit
  --emit-llvm            Print LLVM IR and exit
  -O <LEVEL>             Optimization level: 0-3 (default: 2)
  -v, --verbose          Enable debug logging
  -h, --help             Show help message
```

### Examples

```bash
# Compile to native binary
tscc examples/arithmetic.ts -o my_program

# View generated MLIR
tscc examples/arithmetic.ts --emit-mlir

# View generated LLVM IR
tscc examples/arithmetic.ts --emit-llvm

# Maximum optimization
tscc examples/arithmetic.ts -O3

# Verbose logging
tscc examples/arithmetic.ts -v
```

## Examples

### Basic Arithmetic
```typescript
// examples/arithmetic.ts
2 + 3;
```

```bash
$ ./target/debug/tscc examples/arithmetic.ts
✓  compiled to examples/arithmetic
$ ./examples/arithmetic
$ echo $?
5
```

### Variable Operations
```typescript
// examples/variables.ts
let x = 10;
let y = 5;
x + y;
```

```bash
$ ./target/debug/tscc examples/variables.ts
✓  compiled to examples/variables
$ ./examples/variables; echo $?
15
```

### Complex Calculations
```typescript
// examples/var_arithmetic.ts
let a = 2;
let b = 3;
let c = a + b;
c * 2;
```

```bash
$ ./target/debug/tscc examples/var_arithmetic.ts
✓  compiled to examples/var_arithmetic
$ ./examples/var_arithmetic; echo $?
10
```

## Type Support

Currently implemented:
- **i32**: 32-bit signed integers (primary type)

Planned:
- **boolean**: true/false values
- **f64**: 64-bit floating point
- **string**: String literals and operations
- **Array<T>**: Typed arrays
- **Object**: Object literals and properties

## Debugging

### Enable tracing
```bash
./target/debug/tscc examples/arithmetic.ts -v
```

Shows:
- Parsing progress
- Variable declarations and references
- Operation lowering steps
- Pass execution

### Inspect intermediate representations

```bash
# View MLIR IR
./target/debug/tscc examples/arithmetic.ts --emit-mlir

# View LLVM IR
./target/debug/tscc examples/arithmetic.ts --emit-llvm

# Compiler will also save intermediate files:
# - examples/arithmetic.mlir (MLIR text format)
# - examples/arithmetic.ll    (LLVM IR text format)
# - examples/arithmetic.o     (Object file)
```

## Architecture

### ts-frontend
The parser layer wraps OXC (the fastest Rust TypeScript/JavaScript parser).
- Parses TypeScript → OXC AST
- Handles syntax errors and recovery
- No type checking (deferred to codegen)

### ts-codegen
The code generation layer lowers OXC AST to MLIR.
- **lowering.rs**: AST → MLIR dialect operations
  - NumericLiteral → arith.constant
  - BinaryExpression → arith.addi/subi/muli/divsi
  - VariableDeclaration → scope management
  - Identifier → scope lookup
- **passes.rs**: MLIR transformation pipeline
- **context.rs**: MLIR context and dialect registration

### tscc
The compiler driver orchestrates the pipeline.
- CLI argument parsing (clap)
- Pipeline orchestration
- MLIR → LLVM translation (mlir-translate)
- LLVM compilation (llc)
- Binary linking (clang)

## Performance

### Compilation Time
- Simple programs (10-20 LOC): ~2 seconds
- Includes LLVM optimization passes

### Generated Code
- Optimization levels: -O0 to -O3 (passed to llc)
- MLIR canonicalization and lowering passes
- Full LLVM backend optimization

### Binary Size
- Minimal program (~arithmetic.ts): ~20KB
- No runtime overhead
- Direct native code execution

## Limitations

### Current
- Only i32 integers supported
- Single return value (exit code)
- No dynamic memory allocation
- No standard library
- Function scope only (no global state)

### By Design
- Ahead-of-time compilation only (no JIT)
- No garbage collection required
- Direct native execution (no VM)
- Rust safety guarantees in codegen

## Building from Source

```bash
# Clone repository
git clone <repo-url>
cd typesciprt-aot-native-compiler

# Build all crates
cargo build -p tscc

# Run tests
cargo test

# Build release binary
cargo build --release -p tscc

# Binary location
./target/release/tscc
```

## Configuration

### LLVM Path
Edit `.cargo/config.toml` to use different LLVM installation:

```toml
[build]
rustflags = [
  "-L/path/to/llvm/lib",
  "-L/path/to/llvm/lib64",
]

[env]
MLIR_SYS_210_PREFIX = "/path/to/llvm"
LLVM_SYS_210_PREFIX = "/path/to/llvm"
```

### Optimization Levels
- `-O0`: No optimization (fastest compilation)
- `-O1`: Light optimization
- `-O2`: Standard optimization (default)
- `-O3`: Aggressive optimization

## Contributing

### Code Style
- Follow Rust conventions (checked by rustfmt)
- No unsafe code unless necessary
- Comprehensive error messages
- Debug logging for development

### Testing
Add test cases to `examples/` directory:
```typescript
// examples/test_feature.ts
// Expected exit code: <value>
```

Run manual tests:
```bash
./target/debug/tscc examples/test_feature.ts
./examples/test_feature; echo "Exit code: $?"
```

## Roadmap

See [PLAN.md](PLAN.md) for detailed development roadmap.

**Short term** (v0.2):
- Comparison operators
- If/else statements
- Boolean type

**Medium term** (v0.3):
- Function declarations
- For/while loops
- Proper scoping

**Long term** (v1.0):
- String support
- Array type
- Standard library
- console.log alternative

## License

MIT

## References

- [MLIR Documentation](https://mlir.llvm.org/)
- [OXC Parser](https://github.com/web-infra-dev/oxc)
- [melior Crate](https://github.com/raparic/melior)
- [LLVM Documentation](https://llvm.org/docs/)
