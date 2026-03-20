//! TypeScript AOT compiler runtime library.
//!
//! This library is linked into every compiled TypeScript binary.  It provides
//! the low-level support routines that the compiler emits calls to.
//!
//! The Tokio multi-thread runtime is started lazily on the first call that
//! needs it (e.g. `__ts_console_log_i32`).  Future async TS features will
//! schedule tasks onto this executor via `ts_runtime()`.

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
