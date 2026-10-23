//! Bounded lexical hints into the existing sanitized plain-text preview.
use crate::structured::{
    Preview, PreviewFormat, PreviewLimits, PreviewPayload, PreviewSource, PreviewState,
    PreviewWindow,
};
use crate::CancelToken;
use serde::Serialize;
use std::ffi::OsStr;
use std::path::Path;

mod lexer;

pub const MAX_SYNTAX_SPANS: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Python,
    #[serde(rename = "javascript")]
    JavaScript,
    #[serde(rename = "typescript")]
    TypeScript,
    Qml,
    C,
    Cpp,
    Shell,
    Go,
    Java,
}

/// Filename routing only. The name is never opened or treated as trusted code.
pub fn language_for_name(name: &OsStr) -> Option<Language> {
    let extension = Path::new(name).extension()?.to_str()?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "rs" => Language::Rust,
        "py" | "pyw" => Language::Python,
        "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
        "ts" | "tsx" => Language::TypeScript,
        "qml" => Language::Qml,
        "c" | "h" => Language::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
        "sh" | "bash" | "zsh" => Language::Shell,
        "go" => Language::Go,
        "java" => Language::Java,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxRole {
    Keyword,
    String,
    Comment,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxState {
    Complete,
    Unsupported,
    Unavailable,
    Limited,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyntaxSpan {
    pub start_utf16: usize,
    pub len_utf16: usize,
    pub role: SyntaxRole,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyntaxAnnotations {
    pub language: Option<Language>,
    pub state: SyntaxState,
    pub lines: Vec<Vec<SyntaxSpan>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodePreview {
    pub preview: Preview,
    pub syntax: SyntaxAnnotations,
}

/// Worker-side, pure and cancellable. Ranges refer to this very result's text.
pub fn preview_source(
    source: &PreviewSource,
    name: &OsStr,
    window: PreviewWindow,
    limits: &PreviewLimits,
    cancel: &CancelToken,
) -> CodePreview {
    let preview = source.parse(PreviewFormat::Text, window, limits, cancel);
    let language = language_for_name(name);
    let mut result = CodePreview {
        preview,
        syntax: SyntaxAnnotations {
            language,
            state: SyntaxState::Unavailable,
            lines: Vec::new(),
        },
    };
    if cancel.is_cancelled() || matches!(result.preview.state, PreviewState::Cancelled) {
        return cancelled(result);
    }
    let Some(PreviewPayload::Text {
        lines, first_line, ..
    }) = result.preview.payload.as_ref()
    else {
        return result;
    };
    let Some(language) = language else {
        result.syntax.state = SyntaxState::Unsupported;
        return result;
    };
    result.syntax.state = SyntaxState::Complete;
    result.syntax.lines.resize_with(lines.len(), Vec::new);
    if lines.is_empty() {
        return result;
    }
    let first = *first_line;
    let end_line = first.saturating_add(lines.len());
    let bytes = &source.bytes[..result.preview.bytes];
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    // Text parsing already validated this prefix; a byte-limited trailing scalar
    // may be incomplete, and was also omitted by the plain preview.
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or(""),
    };
    let mut lexer = lexer::Lexer::new(language);
    let mut count = 0;
    let mut fragment = String::with_capacity(10);
    for (index, line) in text.lines().enumerate() {
        if index >= end_line {
            break;
        }
        if cancel.is_cancelled() {
            return cancelled(result);
        }
        let visible = index.checked_sub(first).filter(|i| *i < lines.len());
        let mut column = 0;
        let display_end = visible
            .map(|i| lines[i].encode_utf16().count())
            .unwrap_or(0);
        let status = lexer.line(line, cancel, |token, role| {
            let Some(row) = visible else {
                return true;
            };
            let start = column;
            'render: for c in token.chars() {
                if column >= display_end {
                    break;
                }
                fragment.clear();
                crate::peek::push_sanitised_char(&mut fragment, c);
                for rendered in fragment.chars() {
                    let next = column + rendered.len_utf16();
                    if next > display_end {
                        break 'render;
                    }
                    column = next;
                }
            }
            let Some(role) = role.filter(|_| column > start) else {
                return true;
            };
            let spans = &mut result.syntax.lines[row];
            if let Some(last) = spans
                .last_mut()
                .filter(|s| s.role == role && s.start_utf16 + s.len_utf16 == start)
            {
                last.len_utf16 = column - last.start_utf16;
            } else {
                if count >= MAX_SYNTAX_SPANS {
                    return false;
                }
                spans.push(SyntaxSpan {
                    start_utf16: start,
                    len_utf16: column - start,
                    role,
                });
                count += 1;
            }
            true
        });
        match status {
            lexer::Scan::Complete => {}
            lexer::Scan::Cancelled => return cancelled(result),
            lexer::Scan::Limited => {
                result.syntax.state = SyntaxState::Limited;
                break;
            }
        }
    }
    if cancel.is_cancelled() {
        return cancelled(result);
    }
    result
}

fn cancelled(mut result: CodePreview) -> CodePreview {
    result.preview.state = PreviewState::Cancelled;
    result.preview.payload = None;
    result.syntax.state = SyntaxState::Cancelled;
    result.syntax.lines.clear();
    result
}
