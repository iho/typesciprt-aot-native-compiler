//! Wrapper around the MLIR `Context` that registers all required dialects.

use melior::{dialect::DialectRegistry, Context};

/// Owns the MLIR `Context` and is kept alive for the duration of codegen.
pub struct CodegenContext {
    pub mlir: Context,
}

impl CodegenContext {
    /// Create a new context with all dialects the TS compiler needs.
    pub fn new() -> Self {
        let ctx = Context::new();

        // Load all available dialects.
        let registry = DialectRegistry::new();
        melior::utility::register_all_dialects(&registry);
        ctx.append_dialect_registry(&registry);
        ctx.load_all_available_dialects();

        // Allow unregistered operations during development so that we can
        // incrementally add support for new nodes without crashing.
        ctx.set_allow_unregistered_dialects(true);

        Self { mlir: ctx }
    }
}

impl Default for CodegenContext {
    fn default() -> Self {
        Self::new()
    }
}
