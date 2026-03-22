//! TypeScript AOT compiler runtime library.
//!
//! This library is linked into every compiled TypeScript binary.  It provides
//! the low-level support routines that the compiler emits calls to.
//!
//! The Tokio multi-thread runtime is started lazily on the first call that
//! needs it (e.g. `__ts_console_log_i32`).  Future async TS features will
//! schedule tasks onto this executor via `ts_runtime()`.

// ── Global allocator: jemalloc ────────────────────────────────────────────────
// jemalloc with throughput-optimised decay settings:
//   dirty_decay_ms:1000  — keep dirty (freed) pages for 1 s before returning to OS.
//                          Rapid reuse within one second avoids madvise DONTNEED
//                          syscalls, reducing allocation latency under high QPS.
//   muzzy_decay_ms:30000 — keep "muzzy" pages (MADV_FREE on Linux) for 30 s.
//                          The OS may reclaim them under memory pressure but we
//                          avoid the page-fault cost if they are touched again.
//   background_thread:true — dedicated jemalloc background thread for async purging.
//                          Purging no longer blocks on the allocator hot path.
#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(not(feature = "dhat-heap"))]
#[allow(non_upper_case_globals)]
#[export_name = "malloc_conf"]
pub static malloc_conf: &[u8] = b"dirty_decay_ms:1000,muzzy_decay_ms:30000,background_thread:true\0";

// ── Heap profiling (opt-in via `--features dhat-heap`) ───────────────────────
// Build:  cargo build -p ts-runtime --features dhat-heap
// Dumps:  dhat-heap.json on process exit (Ctrl+C).
// View:   npx dhat  dhat-heap.json
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(feature = "dhat-heap")]
pub static DHAT_PROFILER: std::sync::OnceLock<dhat::Profiler> = std::sync::OnceLock::new();

/// Called once from the generated `main` to activate the dhat profiler.
/// A no-op when the feature is disabled.
#[no_mangle]
pub extern "C" fn ts_dhat_init() {
    #[cfg(feature = "dhat-heap")]
    {
        DHAT_PROFILER.get_or_init(|| dhat::Profiler::new_heap());
    }
}

pub mod alloc;
pub mod console;
pub mod exceptions;
pub mod string;
pub mod value;
pub mod node;
pub mod napi;

#[cfg(feature = "napi")]
pub mod napi_bridge;

use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// Version string embedded by the compiler into generated binaries.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Global Tokio runtime ─────────────────────────────────────────────────────

static TOKIO_RT: OnceLock<Runtime> = OnceLock::new();

/// Return a reference to the shared Tokio runtime, initialising it on first call.
pub fn ts_runtime() -> &'static Runtime {
    TOKIO_RT.get_or_init(|| Runtime::new().expect("failed to start tokio runtime"))
}
