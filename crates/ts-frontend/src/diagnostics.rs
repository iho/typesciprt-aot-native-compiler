//! Simple diagnostic printer (will be replaced by a proper LSP-compatible
//! diagnostics system later).

use oxc_span::Span;

pub struct Diagnostic {
    pub message: String,
    pub span:    Option<Span>,
    pub level:   Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.level {
            Level::Error   => "error",
            Level::Warning => "warning",
            Level::Note    => "note",
        };
        write!(f, "{prefix}: {}", self.message)?;
        if let Some(span) = self.span {
            write!(f, " [{}..{}]", span.start, span.end)?;
        }
        Ok(())
    }
}
