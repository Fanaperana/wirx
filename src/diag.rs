//! Diagnostics: errors and warnings with source locations, rendered for people or as JSON for tools.

use serde::Serialize;

/// Byte range in one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub file: usize,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(file: usize, start: usize, end: usize) -> Self {
        Span { file, start, end }
    }
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable machine-readable id, e.g. `pin_not_connected`.
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
    /// A concrete suggestion, ideally a line of wirx that fixes the problem.
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic { severity: Severity::Error, code, message: message.into(), span: None, hint: None }
    }
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic { severity: Severity::Warning, code, message: message.into(), span: None, hint: None }
    }
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
    pub fn at_opt(mut self, span: Option<Span>) -> Self {
        self.span = span.or(self.span);
        self
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

pub struct SourceFile {
    pub name: String,
    pub text: String,
}

/// All loaded source files, indexed by `Span::file`.
#[derive(Default)]
pub struct Sources {
    pub files: Vec<SourceFile>,
}

impl Sources {
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> usize {
        self.files.push(SourceFile { name: name.into(), text: text.into() });
        self.files.len() - 1
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, file: usize, offset: usize) -> (usize, usize) {
        let text = &self.files[file].text;
        let offset = offset.min(text.len());
        let before = &text[..offset];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
        (line, col)
    }

    pub fn render(&self, d: &Diagnostic) -> String {
        let sev = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        let mut out = format!("{sev}[{}]: {}\n", d.code, d.message);
        if let Some(span) = d.span {
            let f = &self.files[span.file];
            let (line, col) = self.line_col(span.file, span.start);
            out.push_str(&format!("  --> {}:{}:{}\n", f.name, line, col));
            if let Some(src) = f.text.lines().nth(line - 1) {
                let width = f.text[span.start..span.end.min(f.text.len())]
                    .lines()
                    .next()
                    .map(|s| s.chars().count())
                    .unwrap_or(1)
                    .max(1);
                let gutter = line.to_string().len();
                out.push_str(&format!("{:gutter$} |\n", ""));
                out.push_str(&format!("{line} | {src}\n"));
                out.push_str(&format!("{:gutter$} | {}{}\n", "", " ".repeat(col - 1), "^".repeat(width)));
            }
        }
        if let Some(h) = &d.hint {
            out.push_str(&format!("  = help: {h}\n"));
        }
        out
    }

    pub fn to_json(&self, d: &Diagnostic) -> JsonDiagnostic {
        let (file, line, col) = match d.span {
            Some(s) => {
                let (l, c) = self.line_col(s.file, s.start);
                (Some(self.files[s.file].name.clone()), Some(l), Some(c))
            }
            None => (None, None, None),
        };
        JsonDiagnostic { severity: d.severity, code: d.code, message: d.message.clone(), file, line, col, hint: d.hint.clone() }
    }
}

#[derive(Serialize)]
pub struct JsonDiagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub hint: Option<String>,
}

pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}
