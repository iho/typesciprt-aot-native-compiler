//! Integration test: compile `examples/loop_control.ts` and `examples/ternary.ts` and run the binaries.
//!
//! Run with:
//!   cargo test --test control_flow

use std::{path::PathBuf, process::Command};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()  // crates/tscc        → crates
        .parent().unwrap()  // crates             → repo root
        .to_path_buf()
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_loop_control() {
    let root      = repo_root();
    let input     = root.join("examples/loop_control.ts");
    let output    = root.join("target/test-loop-control");

    let build = Command::new("cargo")
        .args(["build", "-p", "tscc"])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    assert!(build.success(), "cargo build must succeed");

    let tscc = root.join("target/debug/tscc");

    let compile = Command::new(&tscc)
        .args([input.to_str().unwrap(), "-o", output.to_str().unwrap()])
        .status()
        .expect("tscc failed to spawn");
    assert!(compile.success(), "tscc must exit successfully");

    let run = Command::new(&output)
        .status()
        .expect("failed to run compiled binary");
    
    // Output of loop_control() returns 23 limit is 255.
    assert_eq!(run.code(), Some(23), "loop_control() returned incorrect exit code.");
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_ternary() {
    let root      = repo_root();
    let input     = root.join("examples/ternary.ts");
    let output    = root.join("target/test-ternary");

    let build = Command::new("cargo")
        .args(["build", "-p", "tscc"])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    assert!(build.success(), "cargo build must succeed");

    let tscc = root.join("target/debug/tscc");

    let compile = Command::new(&tscc)
        .args([input.to_str().unwrap(), "-o", output.to_str().unwrap()])
        .status()
        .expect("tscc failed to spawn");
    assert!(compile.success(), "tscc must exit successfully");

    let run = Command::new(&output)
        .status()
        .expect("failed to run compiled binary");
    
    // Output of is_even() calls returns 2 limit is 255.
    assert_eq!(run.code(), Some(2), "is_even() returned incorrect exit code.");
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_short_circuit() {
    let root      = repo_root();
    let input     = root.join("examples/short_circuit.ts");
    let output    = root.join("target/test-short-circuit");

    let build = Command::new("cargo")
        .args(["build", "-p", "tscc"])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    assert!(build.success(), "cargo build must succeed");

    let tscc = root.join("target/debug/tscc");

    let compile = Command::new(&tscc)
        .args([input.to_str().unwrap(), "-o", output.to_str().unwrap()])
        .status()
        .expect("tscc failed to spawn");
    assert!(compile.success(), "tscc must exit successfully");

    let run = Command::new(&output)
        .status()
        .expect("failed to run compiled binary");
    
    assert_eq!(run.code(), Some(0), "short_circuit() returned incorrect exit code.");
}
