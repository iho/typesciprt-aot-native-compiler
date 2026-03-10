//! Integration test: compile `examples/fib.ts` and `examples/factorial.ts` and run the binaries.
//!
//! Run with:
//!   cargo test --test recursive_functions

use std::{path::PathBuf, process::Command};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()  // tests/integration  → tests
        .parent().unwrap()  // tests              → repo root
        .to_path_buf()
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_fibonacci() {
    let root      = repo_root();
    let input     = root.join("examples/fib.ts");
    let output    = root.join("target/test-fib");

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
    
    // Output of fib(6) is 8 limit is 255.
    assert_eq!(run.code(), Some(8), "fib(6) returned incorrect exit code.");
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_factorial() {
    let root      = repo_root();
    let input     = root.join("examples/factorial.ts");
    let output    = root.join("target/test-factorial");

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
    
    // Output of fact(5) is 120, limit is 255.
    assert_eq!(run.code(), Some(120), "fact(5) returned incorrect exit code.");
}
