//! Spans, sources and rendered diagnostics.
//!
//! Every later phase reports problems through this type, so it is worth getting the presentation
//! right once: an analyst who has to count characters to find the offending line will not read the
//! message at all.

use std::fmt;

/// A byte range in a source file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// The smallest span containing both.
    #[must_use]
    pub fn merge(self, other: Span) -> Span {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    #[must_use]
    pub fn empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A file plus the line offsets needed to turn a byte offset into a line and column.
///
/// `Debug` deliberately leaves the body out: a model source is often hundreds of lines and a
/// debugger dump should not drown in it.
pub struct Source {
    pub path: String,
    text: String,
    line_starts: Vec<u32>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Source {{ path: {:?}, lines: {}, bytes: {} }}",
            self.path,
            self.line_starts.len(),
            self.text.len()
        )
    }
}

impl Source {
    #[must_use]
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        let path = path.into();
        let text = text.into();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self {
            path,
            text,
            line_starts,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Zero-based line index and zero-based column in *characters*, which is what a reader expects.
    #[must_use]
    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let line = self
            .line_starts
            .binary_search(&offset)
            .unwrap_or_else(|i| i.saturating_sub(1));
        let start = self.line_starts[line] as usize;
        let col = self.text[start..offset.min(self.text.len() as u32) as usize]
            .chars()
            .count();
        (line, col)
    }

    /// The full text of the line containing `offset`.
    #[must_use]
    pub fn line_text(&self, line: usize) -> &str {
        let Some(&start) = self.line_starts.get(line) else {
            return "";
        };
        let start = start as usize;
        let end = self.text[start..]
            .find('\n')
            .map_or(self.text.len(), |i| start + i);
        self.text[start..end].trim_end_matches('\r')
    }

    #[must_use]
    pub fn span_text(&self, span: Span) -> &str {
        let s = (span.start as usize).min(self.text.len());
        let e = (span.end as usize).min(self.text.len());
        &self.text[s..e.max(s)]
    }
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,
    Warning,
    Error,
}

impl Severity {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// One pointed-at place in the source.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A problem reported by any phase of the front end.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    /// Optional advice, printed as a help line.
    pub help: Option<String>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            labels: vec![Label {
                span,
                message: String::new(),
            }],
            help: None,
        }
    }

    #[must_use]
    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            labels: vec![Label {
                span,
                message: String::new(),
            }],
            help: None,
        }
    }

    #[must_use]
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// True when this diagnostic must stop compilation.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Render with a source excerpt, the way rustc does it.
    #[must_use]
    pub fn render(&self, src: &Source) -> String {
        let mut out = format!("{}: {}\n", self.severity.label(), self.message);
        for label in &self.labels {
            let (line, col) = src.line_col(label.span.start);
            let number = line + 1;
            let text = src.line_text(line).replace('\t', "  ");
            let width = number.to_string().len();
            let gutter = " ".repeat(width + 1);
            let _ = std::fmt::write(
                &mut out,
                format_args!("{} --> {}:{}:{}\n", gutter, src.path, number, col + 1),
            );
            if text.is_empty() {
                continue;
            }
            let _ = std::fmt::write(&mut out, format_args!("{number} | {text}\n{gutter}| "));
            let lead: String = text.chars().take(col).collect();
            let lead = lead.replace('\t', "  ");
            let caret_len = (label.span.end as usize)
                .saturating_sub(label.span.start as usize)
                .max(1);
            let available = text.chars().count().saturating_sub(col);
            let bars = "^".repeat(caret_len.min(available).max(1));
            let _ = std::fmt::write(
                &mut out,
                format_args!("{}{bars}", " ".repeat(lead.chars().count())),
            );
            if !label.message.is_empty() {
                let _ = std::fmt::write(&mut out, format_args!(" {}", label.message));
            }
            out.push('\n');
        }
        if let Some(help) = &self.help {
            let _ = std::fmt::write(&mut out, format_args!("  = help: {help}\n"));
        }
        out
    }
}

/// Anything that went wrong badly enough to stop a phase.
#[derive(Clone, Debug)]
pub struct Errors(pub Vec<Diagnostic>);

impl Errors {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn render_all(&self, src: &Source) -> String {
        self.0
            .iter()
            .map(|d| d.render(src))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in &self.0 {
            writeln!(f, "{}: {}", d.severity.label(), d.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_are_offsets_after_each_newline() {
        let src = Source::new("t.ap", "abc\ndefg\nhi");
        assert_eq!(src.line_col(0), (0, 0));
        assert_eq!(src.line_col(4), (1, 0));
        assert_eq!(src.line_col(8), (1, 4));
        assert_eq!(src.line_col(10), (2, 1));
    }

    #[test]
    fn span_of_a_word_can_be_read_back() {
        let src = Source::new("t.ap", "let v = a + b\n");
        assert_eq!(src.span_text(Span::new(4, 5)), "v");
        assert_eq!(src.span_text(Span::new(12, 13)), "b");
    }

    #[test]
    fn rendering_points_at_the_right_column() {
        let src = Source::new("p.ap", "    let v = a + b\n");
        let d = Diagnostic::error("cannot add m and s", Span::new(10, 15))
            .with_label(Span::new(12, 13), "this is metres");
        let text = d.render(&src);
        assert!(text.contains("error: cannot add m and s"), "{text}");
        assert!(text.contains("--> p.ap:1:11"), "{text}");
        assert!(text.contains("    let v = a + b"), "{text}");
        assert!(text.contains("^^^"), "{text}");
        assert!(text.contains("this is metres"), "{text}");
    }

    #[test]
    fn merge_gives_the_smallest_covering_span() {
        assert_eq!(Span::new(5, 9).merge(Span::new(1, 3)), Span::new(1, 9));
    }

    #[test]
    fn tabs_do_not_move_the_caret_off_the_character() {
        let src = Source::new("t.ap", "\tx = 1\n");
        let d = Diagnostic::error("bad", Span::new(1, 2));
        let rendered = d.render(&src);
        // Column is counted in characters, so the caret sits under `x`.
        assert!(rendered.contains(":1:2"), "{rendered}");
    }
}
