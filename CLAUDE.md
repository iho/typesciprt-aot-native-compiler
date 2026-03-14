# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build the compiler
cargo build -p tscc

# Run all integration tests (ALWAYS use --test-threads=1 — concurrent LLVM codegen has race conditions)
cargo test -p tscc -- --include-ignored --test-threads=1

# Run a single test
cargo test -p tscc <test_name> -- --include-ignored --test-threads=1

# Compile a TypeScript file
cargo run -p tscc -- input.ts
cargo run -p tscc -- input.ts --emit-mlir   # dump MLIR and exit
cargo run -p tscc -- input.ts --emit-llvm   # dump LLVM IR and exit

# Dump MLIR to /tmp/hono_debug.mlir before passes (useful for debugging)
DUMP_MLIR=1 cargo run -p tscc -- input.ts
```

## Architecture

**Pipeline:** TypeScript → OXC AST → MLIR → LLVM IR → object file → native binary (via clang)

### Crates
- **`tscc`**: CLI driver. Orchestrates parsing → lowering → LLVM emission → linking. `emit.rs` handles MLIR→LLVM IR translation and `clang`-based linking.
- **`ts-frontend`**: Thin wrapper over OXC parser. Single entry point: `parse_typescript()`.
- **`ts-codegen`**: The bulk of the compiler. Transforms OXC AST to MLIR using the melior crate. The `lowering/` subdirectory contains the core logic split across:
  - `mod.rs` — top-level program lowering, function signatures, import resolution, class/module init infrastructure
  - `expressions.rs` — all expression lowering including closures, builtins, call dispatch (~5800 lines)
  - `statements.rs` — statement lowering, `lower_main_function`, module init calls
  - `operators.rs` — binary/unary/assignment operators
  - `classes.rs` — class declarations, constructors, inheritance
  - `literals.rs`, `enums.rs`
- **`ts-runtime`**: Rust static library linked into every compiled binary. Provides all runtime support functions callable from generated MLIR as `#[no_mangle]` C functions.

### Value Representation: NaN-Boxing
All TypeScript values are `TsVal` (`i64`, NaN-boxed). At the MLIR level, **all function return types are `i64`**.

```
TAG_UNDEFINED = 0x7FF8_0000_0000_0000
TAG_NULL      = 0x7FF9_0000_0000_0000
TAG_BOOL      = 0x7FFA_0000_0000_0000  (bit 0 = value)
TAG_PTR       = 0x7FFC_0000_0000_0000  (lower 48 bits = heap pointer)
TAG_INT       = 0x7FFE_0000_0000_0000  (lower 32 bits = i32 value)
```

Heap objects are reference-counted (ARC). Heap tags: TsObject(0), TsArray(1), TsString(2), TsPromise(3), TsFunction(4), TsMap(5), TsRegExp(6), TsHeaders(7), TsResponse(8).

### ARC Rules
- Every variable read emits `ts_retain_val` (produces an owned reference).
- Every `ExpressionStatement` result is released via `ts_release_val`.
- `ThisExpression` must call `ts_retain_val` (same as `Identifier`).
- Block-body arrow functions return `UNDEFINED` unless they have an explicit `return`. Expression-body arrows (`(x) => expr`) preserve the expression value without releasing it.

### Closures
Free variables are captured into a `TsArray` environment. Closure MLIR functions take `(env: i64, param0, ...)`. Created via `ts_closure_new(fn_ptr, arity, env)`.

### Imports & Module Init
`process_import_recursive` loads imported `.ts` files. Non-function module-level const declarations are initialized via `__init_module_N` functions emitted by `lower_imported_module_init`, called at the start of `main`.

### Builtin Dispatch
Static method calls on known namespaces (`Math`, `Object`, `Array`, `String`, `JSON`, `Promise`) are dispatched statically. **Important:** builtin method dispatch (e.g. Map's `get`/`set`) is skipped when the receiver is a known user-defined class — checked via `var_class_types` + `classes`.

## Implementation Standard

**Always implement features properly and completely — no stubs, simplified workarounds, or placeholder implementations.**

When adding a new built-in type (e.g. `Set`, `WeakMap`):
1. Add a proper heap-allocated Rust struct in `ts-runtime/src/value/`
2. Assign it the next available heap tag and register its destructor in `ts_release_val`
3. Implement the full method set as `#[no_mangle]` C functions
4. Wire up `new BuiltIn()` in `lower_new_expression` in codegen
5. Wire up method calls (`.add()`, `.has()`, etc.) in the builtin dispatch in `expressions.rs`

Do not use `let _ = ...` to silently ignore unimplemented cases. Do not leave `// TODO` or `// For now skip` in newly written code.

### Async/Await
`async function` returns `i64` (a Promise). Backed by a global Tokio multi-thread runtime. `ts_promise_resolve` / `ts_promise_await` in the runtime.
