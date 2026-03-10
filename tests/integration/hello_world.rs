//! Integration test: compile `examples/hello.ts` and run the binary.
//!
//! Run with:
//!   cargo test --test hello_world

use std::{path::PathBuf, process::Command};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()  // tests/integration  → tests
        .parent().unwrap()  // tests              → repo root
        .to_path_buf()
}

#[test]
#[ignore = "requires LLVM tools on PATH; run with --include-ignored"]
fn compile_and_run_hello_world() {
    let root      = repo_root();
    let input     = root.join("examples/hello.ts");
    let output    = root.join("target/test-hello");

    // Build tscc first.
    let build = Command::new("cargo")
        .args(["build", "-p", "tscc"])
        .current_dir(&root)
        .status()
        .expect("cargo build failed");
    assert!(build.success(), "cargo build must succeed");

    let tscc = root.join("target/debug/tscc");

    // Compile the TypeScript source.
    let compile = Command::new(&tscc)
        .args([input.to_str().unwrap(), "-o", output.to_str().unwrap()])
        .status()
        .expect("tscc failed to spawn");
    assert!(compile.success(), "tscc must exit successfully");

    // Run the compiled binary and check stdout.
    let run = Command::new(&output)
        .output()
        .expect("failed to run compiled binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        stdout.contains("Hello from TypeScript AOT!"),
        "unexpected output: {stdout}"
    );
}
