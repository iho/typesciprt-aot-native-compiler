//! Emit helpers: MLIR → LLVM IR → object file → binary.
//!
//! These functions shell out to `mlir-translate` and `llc` / `clang` from the
//! LLVM installation.  We use the Homebrew LLVM at `/opt/homebrew/opt/llvm`.

use std::{path::{Path, PathBuf}, process::Command};

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

// ── Rust runtime ─────────────────────────────────────────────────────────────

/// Build the `ts-runtime` Rust crate and return the path to its static
/// archive (`libts_runtime.a`).
///
/// The runtime is built in the same profile (debug/release) as the running
/// `tscc` binary.  Its location is determined from the executable path:
///   `<exe_dir>/libts_runtime.a`
/// which is exactly where `cargo build` places it.
pub fn build_runtime() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("locate tscc executable")?;
    // target/{debug,release}/tscc  →  target/{debug,release}
    let target_dir = exe_path
        .parent()
        .context("tscc executable has no parent directory")?;
    // Walk up to the workspace root: target/{debug,release} → target → root
    let workspace_root = target_dir
        .parent()
        .and_then(|p| p.parent())
        .context("cannot determine workspace root from executable path")?;

    // Detect whether we're running from a release build by checking the target dir name.
    let is_release = target_dir.file_name().map_or(false, |n| n == "release");

    // Allow extra features (e.g. "dhat-heap") via TSCC_RUNTIME_FEATURES env var.
    let mut args = vec!["build", "-p", "ts-runtime"];
    if is_release {
        args.push("--release");
    }
    let extra_features = std::env::var("TSCC_RUNTIME_FEATURES").unwrap_or_default();
    if !extra_features.is_empty() {
        args.extend(["--features", extra_features.as_str()]);
    }
    let status = Command::new("cargo")
        .args(&args)
        .current_dir(workspace_root)
        .status()
        .context("spawn `cargo build -p ts-runtime`")?;

    if !status.success() {
        bail!("`cargo build -p ts-runtime` failed");
    }

    let lib = target_dir.join("libts_runtime.a");
    if !lib.exists() {
        bail!("expected {} but it was not produced", lib.display());
    }
    Ok(lib)
}

// ── Object file → native binary ───────────────────────────────────────────────

/// Link the object files using `clang` (which drives `lld`/`ld`).
///
/// On macOS, linking a Rust staticlib requires a few extra system frameworks
/// and libraries that Rust's std/tokio depend on at a lower level.
pub fn link_binary(objs: &[&Path], out: &Path) -> Result<()> {
    let mut cmd = Command::new(llvm_bin("clang"));
    for obj in objs {
        cmd.arg(obj);
    }
    cmd.arg("-o").arg(out);

    // macOS: extra libraries required by the Rust std + tokio runtime.
    // (-lSystem is implicit via clang, so we omit it to avoid the "duplicate
    //  libraries" linker warning.)
    if cfg!(target_os = "macos") {
        cmd.args([
            "-liconv",
            "-lc++",
            "-framework", "CoreFoundation",
            "-framework", "Security",
        ]);
    }

    let status = cmd.status().context("spawn clang (linker)")?;

    if !status.success() {
        bail!("clang linker exited with {status}");
    }
    Ok(())
}
