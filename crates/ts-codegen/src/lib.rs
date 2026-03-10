//! MLIR code generation for the TypeScript AOT compiler.
//!
//! Pipeline
//! --------
//!   OXC AST
//!     ↓  (lowering.rs)
//!   MLIR Module  (func + arith + cf + llvm dialects)
//!     ↓  (passes: inlining, canonicalize, convert-to-llvm, …)
//!   MLIR LLVM dialect
//!     ↓  (mlir-translate)
//!   LLVM IR
//!     ↓  (llc / lld)
//!   native binary

pub mod context;
pub mod lowering;
pub mod passes;

pub use context::CodegenContext;
