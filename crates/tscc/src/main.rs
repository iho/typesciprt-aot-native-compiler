//! `tscc` – TypeScript AOT native compiler driver.
//!
//! Usage:
//!   tscc [OPTIONS] <input.ts>
//!
//! The compiler currently supports a tiny subset of TypeScript (Hello-World
//! level).  The plan is to grow it incrementally.

mod emit;

use std::{fs, path::PathBuf, process::Command};

use anyhow::{bail, Context, Result};
use clap::Parser as ClapParser;
use melior::ir::operation::OperationPrintingFlags;
use melior::ir::operation::OperationLike;
use oxc_allocator::Allocator;
use tracing::info;

use ts_codegen::{lowering::lower_program, passes::run_lowering_pipeline, CodegenContext};
use ts_frontend::parse_typescript;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(ClapParser, Debug)]
#[command(
    name = "tscc",
    about = "TypeScript AOT native compiler (MLIR backend)",
    version
)]
struct Cli {
    /// Input TypeScript source file.
    input: PathBuf,

    /// Output file path (default: replaces `.ts` extension with no extension).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Emit MLIR IR and exit (don't compile to native).
    #[arg(long)]
    emit_mlir: bool,

    /// Emit LLVM IR and exit.
    #[arg(long)]
    emit_llvm: bool,

    /// Optimisation level: 0-3 (default: 2).
    #[arg(short = 'O', default_value = "2")]
    opt_level: u8,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialise logging.
    let filter = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| filter.into()),
        )
        .init();

    // ── 1. Read source ───────────────────────────────────────────────────

    let source = fs::read_to_string(&cli.input)
        .with_context(|| format!("failed to read `{}`", cli.input.display()))?;
    let file_name = cli.input.display().to_string();

    info!("parsing {file_name}");

    // ── 2. Parse TypeScript → OXC AST ────────────────────────────────────

    let alloc = Allocator::default();
    let program =
        parse_typescript(&alloc, &source, &file_name)?;

    info!("lowering to MLIR");

    // ── 3. Lower AST → MLIR ───────────────────────────────────────────────

    let cg      = CodegenContext::new();
    let mut module  = lower_program(&cg, &program, &file_name)
        .context("AST → MLIR lowering failed")?;

    if cli.emit_mlir {
        let flags = OperationPrintingFlags::new();
        println!("{}", module.as_operation().to_string_with_flags(flags)?);
        return Ok(());
    }

    // ── 4. Run MLIR passes ────────────────────────────────────────────────

    info!("running MLIR pass pipeline");
    run_lowering_pipeline(&cg.mlir, &mut module)
        .context("MLIR pass pipeline failed")?;

    // ── 5. Translate MLIR → LLVM IR ───────────────────────────────────────

    let stem = cli.input.file_stem().unwrap_or_default();
    let out_dir = cli.input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let ll_path  = out_dir.join(format!("{}.ll",  stem.to_string_lossy()));
    let obj_path = out_dir.join(format!("{}.o",   stem.to_string_lossy()));
    let bin_path = cli.output.clone().unwrap_or_else(|| {
        out_dir.join(format!("{}.exe", stem.to_string_lossy()))
    });

    emit::mlir_to_llvm_ir(&module, &ll_path)
        .context("mlir → llvm IR translation failed")?;

    if cli.emit_llvm {
        println!("{}", fs::read_to_string(&ll_path)?);
        return Ok(());
    }

    // ── 6. Compile LLVM IR → object file ─────────────────────────────────

    info!("compiling LLVM IR → object file");
    emit::llvm_ir_to_object(&ll_path, &obj_path, cli.opt_level)
        .context("LLVM IR → object compilation failed")?;

    // Build the Rust runtime (ts-runtime crate) and get the static archive.
    let runtime_obj = emit::build_runtime().context("runtime build failed")?;

    // ── 7. Link → native binary ───────────────────────────────────────────

    info!("linking → {}", bin_path.display());
    emit::link_binary(&[&obj_path, &runtime_obj], &bin_path)
        .context("linking failed")?;

    println!("✓  compiled to {}", bin_path.display());
    Ok(())
}
