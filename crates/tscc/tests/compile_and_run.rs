//! End-to-end integration tests for the `tscc` compiler.
//!
//! Each test compiles a TypeScript fixture and asserts the exit code of the
//! resulting native binary.
//!
//! Requirements: Homebrew LLVM at `/opt/homebrew/opt/llvm` (macOS ARM64).
//! Run all tests, including those that require LLVM:
//!   cargo test -p tscc -- --include-ignored

use std::{
    path::{Path, PathBuf},
    process::Command,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap() // crates/tscc → crates
        .parent().unwrap() // crates      → repo root
        .to_path_buf()
}

fn tscc_bin() -> PathBuf {
    repo_root().join("target/debug/tscc")
}

/// Build `tscc` (debug) exactly once per test process, regardless of how many
/// tests run in parallel.
fn ensure_tscc_built() {
    use std::sync::Once;
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let status = Command::new("cargo")
            .args(["build", "-p", "tscc"])
            .current_dir(repo_root())
            .status()
            .expect("cargo build failed to spawn");
        assert!(status.success(), "cargo build -p tscc failed");
    });
}

/// Compile `input.ts` → binary at `out`, assert exit code equals `expected`.
fn compile_and_check(input: &Path, out: &Path, expected_exit: i32) {
    ensure_tscc_built();

    let compile = Command::new(tscc_bin())
        .arg(input)
        .arg("-o")
        .arg(out)
        .status()
        .expect("tscc failed to spawn");
    assert!(compile.success(), "tscc compilation failed for {}", input.display());

    let run = Command::new(out)
        .status()
        .expect("compiled binary failed to spawn");

    let code = run.code().expect("process terminated by signal");
    assert_eq!(
        code, expected_exit,
        "wrong exit code for {} (expected {}, got {})",
        input.display(), expected_exit, code
    );
}

// ── v0.1 – arithmetic & variables ────────────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn arithmetic_addition() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/arithmetic.ts"),
        &root.join("target/test-arithmetic"),
        5, // 2 + 3
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn arithmetic_subtraction() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/subtraction.ts"),
        &root.join("target/test-subtraction"),
        7, // 10 - 3
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn arithmetic_multiplication() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/multiplication.ts"),
        &root.join("target/test-multiplication"),
        20, // 4 * 5
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn arithmetic_division() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/division.ts"),
        &root.join("target/test-division"),
        5, // 20 / 4
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn arithmetic_complex_precedence() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/complex.ts"),
        &root.join("target/test-complex"),
        11, // 5 + 3 * 2
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn variables_basic() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/variables.ts"),
        &root.join("target/test-variables"),
        15, // let x=10; let y=5; x+y
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn variables_arithmetic() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/var_arithmetic.ts"),
        &root.join("target/test-var-arithmetic"),
        10, // let a=2; let b=3; let c=a+b; c*2
    );
}

// ── v0.3 – functions and loops ───────────────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn while_loop_sum() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/while_loop.ts"),
        &root.join("target/test-while-loop"),
        45, // 0+1+…+9
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn for_loop_sum() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/for_loop.ts"),
        &root.join("target/test-for-loop"),
        45, // 0+1+…+9
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn function_calls() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/functions.ts"),
        &root.join("target/test-functions"),
        21, // multiply(3, add(5, 2)) = 3*(5+2)
    );
}

// ── Beta v0.1 – string literals ──────────────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn string_literals_console_log() {
    let root = repo_root();
    ensure_tscc_built();
    let input = root.join("examples/strings.ts");
    let out   = root.join("target/test-strings");

    let compile = Command::new(tscc_bin())
        .arg(&input).arg("-o").arg(&out)
        .status().expect("tscc failed");
    assert!(compile.success(), "tscc must succeed");

    let run = Command::new(&out).output().expect("binary failed");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("hello, world!"),  "expected 'hello, world!' in output, got: {stdout}");
    assert!(stdout.contains("TypeScript AOT"), "expected 'TypeScript AOT' in output, got: {stdout}");
    assert!(stdout.contains("done"),           "expected 'done' in output, got: {stdout}");
    // exit code 0 (last expression is console.log → void → 0)
    assert_eq!(run.status.code().unwrap(), 0);
}

// ── v0.2 – comparisons, booleans, if/else ────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn comparison_less_than() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/comparison.ts"),
        &root.join("target/test-comparison"),
        1, // 5 < 10 → true → 1
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn boolean_and_not() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/boolean.ts"),
        &root.join("target/test-boolean"),
        1, // true && !false → true → 1
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn if_else_true_branch() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/if_else.ts"),
        &root.join("target/test-if-else"),
        42, // x=5 > 0 → result=42
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn console_log_integers() {
    let root = repo_root();
    // Compile the file
    ensure_tscc_built();
    let input = root.join("examples/log.ts");
    let out = root.join("target/test-log");
    let compile = Command::new(tscc_bin())
        .arg(&input).arg("-o").arg(&out)
        .status().expect("tscc failed");
    assert!(compile.success(), "tscc must succeed");

    // Run and check stdout contains the logged values
    let run = Command::new(&out).output().expect("binary failed");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(stdout.contains("42"), "expected 42 in output, got: {stdout}");
    assert!(stdout.contains("50"), "expected 50 in output, got: {stdout}");
    // Exit code = x = 42
    assert_eq!(run.status.code().unwrap(), 42);
}

// ── v0.5 – async/await with real Tokio sleep and Promise.race ────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn async_sleep_and_await() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/async_sleep.ts"),
        &root.join("target/test-async-sleep"),
        7, // delayedAdd(3,4) after 10ms sleep → 7
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn async_promise_race() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/async_select.ts"),
        &root.join("target/test-async-select"),
        1, // fast (10ms) beats slow (500ms) → 1
    );
}

// ── v0.8 – typeof, instanceof, string === ────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn typeof_and_instanceof() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/typeof_instanceof.ts"),
        &root.join("target/test-typeof-instanceof"),
        6, // t_num+t_str+t_bool+t_wrong + tri_is_tri+tri_is_shape+shp_is_shape+shp_is_tri
    );
}

// ── v0.7 – class inheritance, super, static, getters/setters, private fields ──

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn classes_full_features() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/classes_full.ts"),
        &root.join("target/test-classes-full"),
        38, // age(3)+code(5)+desc(3)+gotten(5)+after(10)+doubled(12)
    );
}

// ── v0.6 – try / catch / finally ─────────────────────────────────────────────

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn try_catch_basic() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/try_catch.ts"),
        &root.join("target/test-try-catch"),
        42, // throw 42 → catch(e) { result = e } → exit 42
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn try_finally_basic() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/try_finally.ts"),
        &root.join("target/test-try-finally"),
        15, // result=10 in try, +5 in finally → 15
    );
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn try_catch_finally() {
    let root = repo_root();
    compile_and_check(
        &root.join("examples/try_catch_finally.ts"),
        &root.join("target/test-try-catch-finally"),
        7, // throw 7 → catch(e){result=e} → finally{result+0} → 7
    );
}
