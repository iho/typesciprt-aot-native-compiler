//! Emit helpers: MLIR → LLVM IR → object file → binary.
//!
//! These functions shell out to `mlir-translate` and `llc` / `clang` from the
//! LLVM installation.  We use the Homebrew LLVM at `/opt/homebrew/opt/llvm`.

use std::{path::Path, process::Command};

use anyhow::{bail, Context, Result};
use melior::ir::Module;
use melior::ir::operation::OperationLike;

const LLVM_PREFIX: &str = "/opt/homebrew/opt/llvm";

fn llvm_bin(tool: &str) -> String {
    format!("{LLVM_PREFIX}/bin/{tool}")
}

// ── MLIR → LLVM IR ───────────────────────────────────────────────────────────

/// Use `mlir-translate --mlir-to-llvmir` to produce a `.ll` file.
pub fn mlir_to_llvm_ir(module: &Module<'_>, out: &Path) -> Result<()> {
    // Write the MLIR module to a temp file first.
    let mlir_path = out.with_extension("mlir");
    let flags = melior::ir::operation::OperationPrintingFlags::new();
    let mlir_text = module
        .as_operation()
        .to_string_with_flags(flags)
        .context("serialise MLIR module")?;
    std::fs::write(&mlir_path, &mlir_text)
        .with_context(|| format!("write {}", mlir_path.display()))?;

    let status = Command::new(llvm_bin("mlir-translate"))
        .arg("--mlir-to-llvmir")
        .arg(&mlir_path)
        .arg("-o")
        .arg(out)
        .status()
        .context("spawn mlir-translate")?;

    if !status.success() {
        bail!("mlir-translate exited with {status}");
    }
    Ok(())
}

// ── LLVM IR → object file ─────────────────────────────────────────────────────

/// Use `llc` to compile `.ll` → `.o`.
pub fn llvm_ir_to_object(ll: &Path, out: &Path, opt: u8) -> Result<()> {
    let status = Command::new(llvm_bin("llc"))
        .args(["-filetype=obj", &format!("-O{opt}")])
        .arg(ll)
        .arg("-o")
        .arg(out)
        .status()
        .context("spawn llc")?;

    if !status.success() {
        bail!("llc exited with {status}");
    }
    Ok(())
}

// ── Object file → native binary ───────────────────────────────────────────────

/// Link the object file using `clang` (which drives `lld`/`ld`).
pub fn link_binary(obj: &Path, out: &Path) -> Result<()> {
    let status = Command::new(llvm_bin("clang"))
        .arg(obj)
        .arg("-o")
        .arg(out)
        // Link the TS runtime static library once it exists.
        // .arg("-L").arg("target/release").arg("-lts_runtime")
        .status()
        .context("spawn clang (linker)")?;

    if !status.success() {
        bail!("clang linker exited with {status}");
    }
    Ok(())
}
