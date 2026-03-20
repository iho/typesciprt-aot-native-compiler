//! TypeScript frontend: source → OXC AST → diagnostic reporting.
//!
//! This crate wraps the OXC parser and exposes a clean API for the rest of
//! the compiler.  The OXC allocator is arena-based, so the parsed AST has
//! the same lifetime as the `ParsedModule` you get back.

pub mod diagnostics;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::{Parser, ParserReturn};
use oxc_span::SourceType;
use thiserror::Error;

// ── Public error type ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum FrontendError {
    #[error("parse errors in `{file}`:\n{messages}")]
    ParseErrors { file: String, messages: String },
}

// ── ParsedModule ─────────────────────────────────────────────────────────────

/// Owns the arena allocator and the parsed AST.
///
/// The `Program<'src>` borrows from both the allocator and the source string,
/// so we keep all three together.
pub struct ParsedModule<'src> {
    /// The OXC arena allocator that owns all AST nodes.
    pub alloc: Allocator,
    /// The parsed AST root (borrows from `alloc` and the source).
    pub program: Program<'src>,
    /// Source file path (for diagnostics).
    pub file_name: String,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Parse `source` with an explicit `SourceType` and return a `Program`.
///
/// Returns an error if OXC reports any parse errors.
pub fn parse_source<'src>(
    alloc: &'src Allocator,
    source: &'src str,
    file_name: &str,
    source_type: SourceType,
) -> Result<Program<'src>, FrontendError> {
    let ParserReturn {
        program,
        errors,
        panicked,
        ..
    } = Parser::new(alloc, source, source_type).parse();

    if panicked || !errors.is_empty() {
        let messages = errors
            .iter()
            .map(|e| format!("  {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(FrontendError::ParseErrors {
            file: file_name.to_owned(),
            messages,
        });
    }

    Ok(program)
}

/// Parse `source` as a TypeScript file and return a `Program`.
///
/// Returns an error if OXC reports any parse errors.
pub fn parse_typescript<'src>(
    alloc: &'src Allocator,
    source: &'src str,
    file_name: &str,
) -> Result<Program<'src>, FrontendError> {
    parse_source(alloc, source, file_name, SourceType::ts())
}
