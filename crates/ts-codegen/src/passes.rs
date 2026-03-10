//! MLIR pass pipeline.
//!
//! Converts the high-level MLIR module produced by `lowering` down to the
//! `llvm` dialect, which can then be translated to LLVM IR and compiled.

use anyhow::Result;
use melior::{
    ir::Module,
    pass::{self, PassManager},
    Context,
};

/// Run the standard lowering pipeline:
///   canonicalize → arith-to-llvm → convert-func-to-llvm → …
pub fn run_lowering_pipeline<'c>(ctx: &'c Context, module: &mut Module<'c>) -> Result<()> {
    let pm = PassManager::new(ctx);

    // Canonicalize / simplify first.
    pm.add_pass(pass::transform::create_canonicalizer());

    // Convert arith to llvm dialect.
    pm.add_pass(pass::conversion::create_arith_to_llvm());

    // Convert cf (control-flow) dialect to llvm dialect.
    pm.add_pass(pass::conversion::create_control_flow_to_llvm());

    // Convert func to llvm dialect.
    pm.add_pass(pass::conversion::create_func_to_llvm());

    // Resolve any remaining unrealized casts inserted by dialect conversions.
    pm.add_pass(pass::conversion::create_reconcile_unrealized_casts());

    // Run all passes.
    pm.enable_verifier(true);
    pm.run(module)?;

    Ok(())
}
