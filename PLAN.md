# Development Plan & Roadmap

## Vision
Create a practical TypeScript AOT compiler that generates fast, native binaries. Start with core language features (arithmetic, variables, control flow) and expand to support larger programs.

## Architecture Decisions

### Why MLIR?
- **Abstraction levels**: Allows high-level source optimization before lowering
- **Extensibility**: Easy to add new dialects for future features
- **Maturity**: Battle-tested by LLVM ecosystem
- **Performance**: Multi-level optimization passes
- **Future-proof**: Active LLVM development ensures long-term support

### Why OXC Parser?
- **Speed**: Fastest Rust TypeScript parser (3x faster than alternatives)
- **Compliance**: Nearly complete ECMAScript specification support
- **Ecosystem**: Production-grade (used by web-infra)
- **Accuracy**: Better error recovery than swc

### Type Strategy
**Phase 1** (current): Single type (i32)
**Phase 2**: Add boolean, expand to f64
**Phase 3**: Discriminated unions for polymorphism
**Phase 4**: Generics and type parameters

## Release Plan

### Alpha v0.1 ✅ COMPLETED
**Status**: Shipped (basic arithmetic, variables, expressions)

**Features**:
- ✅ Numeric literals (i32)
- ✅ Arithmetic operators (+, -, *, /)
- ✅ Variable declarations (let, const, var)
- ✅ Binary expressions with proper precedence
- ✅ Scope management and undefined variable detection

**Testing**: 7 example programs with verified output
**Metrics**: ~270 lines of core lowering code

**Known Issues**:
- Only i32 supported (expanded to TsVal for heap objects)
- Control flow implemented (if/else, while, for)
- console.log implemented for all TsVal types via C interop

---

### Alpha v0.2 ✅ COMPLETED
**Target**: Add comparison operators and conditional logic

**Features to add**:
1. **Comparison Operators** (Phase 2a: Weeks 1-2)
   - Operators: `<`, `>`, `<=`, `>=`, `==`, `!=`
   - Return i32 (0 = false, 1 = true)
   - MLIR: Use arith.cmpi operation

2. **Boolean Type** (Phase 2b: Weeks 2-3)
   - Internal: Still i32 (0 = false, non-zero = true)
   - Add parsing for true/false literals
   - Type checking: comparisons → boolean

3. **If/Else Statements** (Phase 2c: Weeks 3-4)
   - AST: Statement::IfStatement
   - MLIR: cf.cond_br (conditional branch)
   - Control flow graph management
   - Block creation for branches

**Implementation order**:
```typescript
// Week 1: Comparison operators
let x = 5;
x < 10;  // → 1 (true)

// Week 2: Boolean operations
5 > 3 && 2 < 4;  // && operator

// Week 3-4: If/else
if (x > 0) {
  10;
} else {
  20;
}
```

**Testing Strategy**:
- Unit tests for comparison operators
- CFG correctness tests for branches
- Integration tests: complex conditions

**Effort Estimate**: 4 weeks
**Risk**: MLIR block management complexity

---

### Alpha v0.3 ✅ COMPLETED
**Target**: Functions and loops

**Features to add**:
1. **Function Declarations** (Phase 3a: Weeks 1-3)
   - AST: Statement::FunctionDeclaration
   - Signature parsing (parameters, return type)
   - MLIR: func.func operation
   - Symbol table for function references

2. **Function Calls** (Phase 3b: Weeks 2-4)
   - AST: Expression::CallExpression → FunctionCall
   - Parameter passing and return values
   - Call stack management in MLIR
   - Type matching for arguments

3. **Loops** (Phase 3c: Weeks 4-6)
   - For loops: `for (let i = 0; i < 10; i++)`
   - While loops: `while (condition) { ... }`
   - MLIR: cf.br (unconditional branch)
   - Loop variable management

**Implementation order**:
```typescript
// Week 1-3: Functions (declaration + basics)
function add(x: number, y: number): number {
  x + y;
}
add(5, 3);  // → 8

// Week 4-6: Loops
let sum = 0;
for (let i = 0; i < 10; i = i + 1) {
  sum = sum + i;
}
sum;  // → 45
```

**Testing Strategy**:
- Function with multiple parameters
- Recursive function calls
- Nested loops
- Loop variable shadowing

**Effort Estimate**: 6 weeks
**Risk**: MLIR scope/block management with multiple functions

---

### Beta v0.1 ✅ COMPLETED
**Target**: Standard library and improved I/O

**Features to add**:
1. **Alternative console.log** (Phase 4a: Weeks 1-2)
   - Approach 1: Write integer to stdout via syscall
   - Approach 2: Link against C standard library
   - Approach 3: Use llvm.call to C functions

2. **String Type** (Phase 4b: Weeks 2-4)
   - String literals: `"hello world"`
   - String concatenation: `"hello" + "world"`
   - MLIR: llvm.mlir.global for static strings

3. **Array Type** (Phase 4c: Weeks 4-6)
   - Array literals: `[1, 2, 3]`
   - Array indexing: `arr[0]`
   - Array length: `arr.length`
   - MLIR: llvm.alloca for heap arrays

**Testing Strategy**:
- output.ts with various data types
- Array bounds checking
- String memory safety

**Effort Estimate**: 6 weeks
**Risk**: Memory layout and allocation

---

### Beta v1.0 (In Planning)
**Target**: Production features and optimization

**Features to add**:
1. **Objects & Properties**
   - Object literals: `{ x: 1, y: 2 }`
   - Property access: `obj.x`
   - Type annotations for object shapes

2. **Classes** (Subset)
   - Class declarations
   - Constructor methods
   - Instance properties and methods
   - Inheritance (single)

3. **Advanced Control Flow**
   - Switch statements
   - Try/catch error handling
   - Break/continue in nested loops

4. **Optimization Passes**
   - Dead code elimination
   - Constant folding
   - Inlining small functions
   - Loop unrolling

---

## Technical Roadmap by Component

### ts-frontend (Parser)
**Current**: OXC parser wrapper
**Next**: Type annotation expansion
**Future**: Semantic analysis phase

**Planned changes**:
- Expand supported syntax (currently skips many constructs)
- Error recovery improvements
- Position tracking for diagnostics

### ts-codegen (Code Generation)
**Current**: Arithmetic + variables
**Next**: Control flow + functions
**Future**: Optimization passes

**Architecture**:
```rust
// Current structure:
lower_program()
  └→ lower_main_function()
     └→ lower_statement()        // Only ExpressionStatement, VariableDeclaration
        └→ lower_expression()

// Future structure:
lower_program()
  ├→ build_symbol_table()        // Function signatures
  └→ lower_statement()
     ├→ lower_if_statement()
     ├→ lower_for_statement()
     ├→ lower_function_decl()
     ├→ lower_variable_declaration()
     └→ lower_expression()
```

**Scope Management Evolution**:
- Current: Single HashMap for main function scope
- v0.2: Branch-aware scope for if/else
- v0.3: Function-level scope table
- v0.4: Closure support with captured variables

### tscc (Compiler Driver)
**Current**: Basic CLI
**Next**: Error diagnostics
**Future**: Incremental compilation

**Planned features**:
- Source location tracking in errors
- Pretty-printed error messages with source context
- Build cache for faster recompilation
- Parallel compilation of multiple files

### ts-runtime (Runtime Library)
**Current**: Empty (reserved)
**v1.0**:
- Standard library functions
- Runtime type information
- Exception handling

---

## Blocked Work

### console.log Implementation
**Status**: Blocked on melior API limitations
**Issue**: melior v0.26 doesn't expose LLVM global variables or address_of operations

**Blocking factors**:
- ODS bindings only expose public operations
- Raw LLVM Operation API not accessible
- Would require melior upgrade or custom bindings

**Workarounds explored**:
1. ✗ Use melior ODS (operations not exposed)
2. ✗ Generate raw LLVM dialect (no way to create globals)
3. Potential: Link against C's printf (requires calling convention setup)
4. Potential: Syscall-based output (platform-specific)

**Resolution plan**:
- Monitor melior releases for improved LLVM operation coverage
- Consider contributing to melior if needed
- Implement phase 4a (Beta v0.1) using C interop

---

## Design Principles

### 1. Correctness First
- Type safety in codegen (Rust prevents many bugs)
- Comprehensive error messages
- Extensive testing before release

### 2. Simple Before Complex
- Start with single types (i32) before generalizing
- Linear control flow before loops/recursion
- Basic functions before closures

### 3. Out of Scope / Future Considerations
- **Garbage Collection (GC)**: Not required. Current implementation uses stack allocation (`llvm.alloca`). Future dynamic memory management for objects/arrays will likely rely on Automatic Reference Counting (ARC) or Arena Allocators rather than a heavy tracing GC.
- **HTTP / Networking Libraries**: Out of scope for the native standard library. The focus is on implementing core language features (control flow, arrays, strings) rather than supporting web backends.

### 3. Incremental Compilation
- Each version adds self-contained features
- Backward compatibility within major versions
- Clear migration path for breaking changes

### 4. Performance Awareness
- MLIR optimizations at each level
- Avoid unnecessary allocations in codegen
- Profile before over-optimizing

---

## Testing Strategy

### Unit Tests
- Individual lowering functions
- Expression evaluation
- Type checking (when added)

### Integration Tests
- Complete programs with expected output
- Error handling and diagnostics
- Compilation time benchmarks

### Suites
- **correctness_suite/**: Verified computation results
- **error_suite/**: Expected compilation errors
- **optimization_suite/**: Performance benchmarks

---

## Known Risks

### High Risk
1. **MLIR Block/Region Management**
   - Challenge: Complex lifetime and borrowing rules
   - Mitigation: Start simple, incrementally add features
   - Impact: Could block v0.3 (functions) release

2. **Scope/Symbol Management**
   - Challenge: Multiple nested scopes with MLIR values
   - Mitigation: Use value-based approach (no variable addresses)
   - Impact: Limits heap allocation features

### Medium Risk
1. **Melior API Stability**
   - Challenge: Breaking changes in melior between versions
   - Mitigation: Lock specific version, plan upgrades
   - Impact: May require codegen rewrites

2. **LLVM Version Compatibility**
   - Challenge: Features may vary between LLVM versions
   - Mitigation: Target LLVM 21+, document requirements
   - Impact: Compiler only works on specific LLVM versions

### Low Risk
1. **Performance**: Can always optimize later
2. **Memory efficiency**: Not critical for v0.x
3. **Compatibility**: Single architecture target (ARM64 macOS)

---

## Success Metrics (v1.0)

✓ **Feature Completeness**
- [x] Variables and functions (including recursive)
- [x] Control flow (if/else, loops)
- [ ] Basic data types (int, bool, string, array)
- [ ] Object-oriented features (classes, methods)

✓ **Code Quality**
- [ ] 100% test coverage for core features
- [ ] Zero unsafe Rust code
- [ ] Comprehensive error messages
- [ ] <10ms compilation for typical programs

✓ **Performance**
- [ ] Generated code matches hand-written C in speed
- [ ] Compilation time <1 second for average programs
- [ ] Binary size <1MB for typical programs

✓ **Documentation**
- [ ] Complete README with examples
- [ ] Architecture documentation
- [ ] Contributing guidelines
- [ ] API documentation

---

## Maintenance Guidelines

### Code Review Checklist
- [ ] All tests passing
- [ ] No unsafe Rust
- [ ] Error messages are clear
- [ ] MLIR operations are correct
- [ ] No performance regressions

### Release Checklist
- [ ] Update VERSION
- [ ] Update STATUS.md and PLAN.md
- [ ] Run full test suite
- [ ] Benchmark compilation time
- [ ] Build release binary successfully

### Dependency Updates
- Lock melior to known good version
- Test LLVM version compatibility before updating
- Update OXC monthly (or on demand for bug fixes)

---

**Last Updated**: 2026-03-10
**Next Review**: After v0.2 completion
**Maintainer**: ihor
