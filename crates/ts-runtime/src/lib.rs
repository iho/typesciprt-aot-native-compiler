//! TypeScript AOT compiler runtime library.
//!
//! This library is linked into every compiled TypeScript binary.  It provides
//! the low-level support routines that the compiler emits calls to.

#![no_std]
// Allow std for now during early development.
// Remove when we switch to a freestanding / no_std runtime.
extern crate std;

pub mod alloc;
pub mod string;
pub mod value;

/// Version string embedded by the compiler into generated binaries.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");
