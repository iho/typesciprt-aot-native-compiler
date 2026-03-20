//! End-to-end HTTP server tests.
//!
//! Each test compiles a TypeScript server fixture, starts the binary, waits for
//! the port to open, fires real HTTP requests via `curl`, asserts the response
//! body / status, then kills the process.
//!
//! Run with:
//!   cargo test -p tscc http_servers -- --include-ignored --test-threads=1

use std::{
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
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

/// Compile `input.ts` to `out` binary.
fn compile_server(input: &Path, out: &Path) {
    ensure_tscc_built();
    let status = Command::new(tscc_bin())
        .arg(input)
        .arg("-o").arg(out)
        .status()
        .expect("tscc failed to spawn");
    assert!(status.success(), "tscc compilation failed for {}", input.display());
}

/// Block until `localhost:port` accepts a TCP connection (up to 5 s).
fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        assert!(Instant::now() < deadline, "server on port {port} did not start in 5 s");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Run `curl` against `url` with optional extra args (e.g. `-X POST`, `-d body`).
/// Returns `(status_code: u16, body: String)`.
fn curl(url: &str, args: &[&str]) -> (u16, String) {
    let out = Command::new("curl")
        .arg("--silent")
        .arg("--write-out").arg("\n%{http_code}")
        .args(args)
        .arg(url)
        .output()
        .expect("curl failed to spawn");

    let raw = String::from_utf8_lossy(&out.stdout);
    // Last line is the status code written by --write-out
    let mut lines: Vec<&str> = raw.trim_end_matches('\n').split('\n').collect();
    let code: u16 = lines.pop().unwrap_or("0").trim().parse().unwrap_or(0);
    let body = lines.join("\n");
    (code, body)
}

/// RAII guard that kills the child process on drop.
struct ServerGuard(Child);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

// ── Hono-style server ─────────────────────────────────────────────────────────

const HONO_PORT: u16 = 18888; // high port to avoid conflicts with hono_simple default

/// Compile `hono_simple.ts` (which hardcodes port 8888) but point our test at
/// a dedicated copy that uses HONO_PORT.
fn hono_fixture() -> PathBuf {
    let src  = repo_root().join("examples/hono_simple.ts");
    let dest = repo_root().join("examples/hono_test_fixture.ts");

    // Rewrite the port in a scratch copy.
    let source = std::fs::read_to_string(&src).unwrap();
    let patched = source.replace("serve(8888,", &format!("serve({HONO_PORT},"));
    std::fs::write(&dest, patched).unwrap();
    dest
}

fn start_hono() -> (ServerGuard, PathBuf) {
    let fixture = hono_fixture();
    let out = repo_root().join("target/test-hono");
    compile_server(&fixture, &out);
    let child = Command::new(&out).spawn().expect("server binary failed to spawn");
    wait_for_port(HONO_PORT);
    (ServerGuard(child), out)
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_get_root() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello from native Hono!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_get_hello_name() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/hello/Alice");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200);
    assert!(body.contains("Hello, Alice!"), "body was: {body}");
    assert!(body.contains("/hello/Alice"), "body was: {body}");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_post_echo() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/echo");
    let (code, body) = curl(&url, &["-X", "POST", "-d", "hello world"]);
    assert_eq!(code, 200);
    assert_eq!(body.trim(), "Echo: hello world");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_not_found() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/does-not-exist");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 404);
    assert_eq!(body.trim(), "Not Found");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_url_search_params() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/url-test?foo=bar");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200);
    assert!(body.contains("\"query_foo\":\"bar\""), "body was: {body}");
    assert!(body.contains("\"/url-test\""), "body was: {body}");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn hono_request_headers() {
    let (_guard, _) = start_hono();
    let url = format!("http://127.0.0.1:{HONO_PORT}/headers");
    let (code, body) = curl(&url, &["-H", "accept: application/json"]);
    assert_eq!(code, 200);
    assert!(body.contains("application/json"), "body was: {body}");
}

// ── NestJS-style server ───────────────────────────────────────────────────────

const NEST_PORT: u16 = 13000; // avoids conflict with test_nest default (3000)
const NEST_UNMODIFIED_PORT: u16 = 13001;

fn nest_fixture() -> PathBuf {
    // test_nest.ts imports nest-native.ts which calls bootstrapNative(AppModule, 3000).
    // We need a copy that passes NEST_PORT instead.
    let src  = repo_root().join("examples/test_nest.ts");
    let dest = repo_root().join("examples/nest_test_fixture.ts");
    let source = std::fs::read_to_string(&src).unwrap();
    let patched = source.replace("bootstrapNative(AppModule, 3000)", &format!("bootstrapNative(AppModule, {NEST_PORT})"));
    std::fs::write(&dest, patched).unwrap();
    dest
}

fn start_nest() -> (ServerGuard, PathBuf) {
    let fixture = nest_fixture();
    let out = repo_root().join("target/test-nest");
    compile_server(&fixture, &out);
    let child = Command::new(&out).spawn().expect("server binary failed to spawn");
    wait_for_port(NEST_PORT);
    (ServerGuard(child), out)
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_get_root() {
    let (_guard, _) = start_nest();
    let url = format!("http://127.0.0.1:{NEST_PORT}/");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello from NestJS!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_get_world() {
    let (_guard, _) = start_nest();
    let url = format!("http://127.0.0.1:{NEST_PORT}/world");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200);
    assert_eq!(body.trim(), "Hello World!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_post_echo() {
    let (_guard, _) = start_nest();
    let url = format!("http://127.0.0.1:{NEST_PORT}/echo");
    let (code, body) = curl(&url, &["-X", "POST"]);
    assert_eq!(code, 200);
    assert_eq!(body.trim(), "echo!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_not_found() {
    let (_guard, _) = start_nest();
    let url = format!("http://127.0.0.1:{NEST_PORT}/no-such-route");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 404);
    assert_eq!(body.trim(), "Not Found");
}

// ── nestjs_unmodified.ts — standard @nestjs/common + @nestjs/core decorators ──

fn start_nest_unmodified() -> (ServerGuard, PathBuf) {
    // nestjs_unmodified.ts hardcodes port 3000; patch to NEST_UNMODIFIED_PORT.
    let src  = repo_root().join("examples/nestjs_unmodified.ts");
    let dest = repo_root().join("examples/nest_unmodified_fixture.ts");
    let source = std::fs::read_to_string(&src).unwrap();
    let patched = source.replace("app.listen(3000)", &format!("app.listen({NEST_UNMODIFIED_PORT})"));
    std::fs::write(&dest, patched).unwrap();

    let out = repo_root().join("target/test-nest-unmodified");
    compile_server(&dest, &out);
    let child = Command::new(&out).spawn().expect("server binary failed to spawn");
    wait_for_port(NEST_UNMODIFIED_PORT);
    (ServerGuard(child), out)
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_unmodified_get_root() {
    let (_guard, _) = start_nest_unmodified();
    let url = format!("http://127.0.0.1:{NEST_UNMODIFIED_PORT}/api");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello from NestJS!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_unmodified_get_hello() {
    let (_guard, _) = start_nest_unmodified();
    let url = format!("http://127.0.0.1:{NEST_UNMODIFIED_PORT}/api/hello");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello World!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_unmodified_post_echo() {
    let (_guard, _) = start_nest_unmodified();
    let url = format!("http://127.0.0.1:{NEST_UNMODIFIED_PORT}/api/echo");
    let (code, body) = curl(&url, &["-X", "POST"]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "echo!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn nest_unmodified_not_found() {
    let (_guard, _) = start_nest_unmodified();
    let url = format!("http://127.0.0.1:{NEST_UNMODIFIED_PORT}/api/no-such-route");
    let (code, _) = curl(&url, &[]);
    assert_eq!(code, 404, "expected 404, got {code}");
}

// ── node:http built-in module ─────────────────────────────────────────────────

const NODE_HTTP_PORT: u16 = 19001;

fn start_node_http() -> (ServerGuard, PathBuf) {
    let src  = repo_root().join("examples/node_http_server.ts");
    let dest = repo_root().join("examples/node_http_fixture.ts");
    let source = std::fs::read_to_string(&src).unwrap();
    let patched = source.replace(
        &format!("const PORT = {NODE_HTTP_PORT};"),
        &format!("const PORT = {NODE_HTTP_PORT};"),
    );
    std::fs::write(&dest, patched).unwrap();
    let out = repo_root().join("target/test-node-http");
    compile_server(&dest, &out);
    let child = Command::new(&out).spawn().expect("server binary failed to spawn");
    wait_for_port(NODE_HTTP_PORT);
    (ServerGuard(child), out)
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn node_http_get_root() {
    let (_guard, _) = start_node_http();
    let url = format!("http://127.0.0.1:{NODE_HTTP_PORT}/");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello from node:http!");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn node_http_get_hello() {
    let (_guard, _) = start_node_http();
    let url = format!("http://127.0.0.1:{NODE_HTTP_PORT}/hello");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Hello World");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn node_http_post_echo() {
    let (_guard, _) = start_node_http();
    let url = format!("http://127.0.0.1:{NODE_HTTP_PORT}/echo");
    let (code, body) = curl(&url, &["-X", "POST", "-d", "hello world"]);
    assert_eq!(code, 200, "expected 200, got {code}");
    assert_eq!(body.trim(), "Echo: hello world");
}

#[test]
#[ignore = "requires LLVM; run with --include-ignored"]
fn node_http_not_found() {
    let (_guard, _) = start_node_http();
    let url = format!("http://127.0.0.1:{NODE_HTTP_PORT}/no-such-route");
    let (code, body) = curl(&url, &[]);
    assert_eq!(code, 404, "expected 404, got {code}");
    assert_eq!(body.trim(), "Not Found");
}
