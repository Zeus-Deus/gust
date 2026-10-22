use super::{Language, SyntaxRole};
use crate::CancelToken;

#[derive(Clone, Copy)]
enum Mode {
    Plain,
    Block(usize),
    Quoted { byte: u8, triple: bool },
    RawRust(usize),
}
pub(super) enum Scan {
    Complete,
    Limited,
    Cancelled,
}
pub(super) struct Lexer {
    language: Language,
    mode: Mode,
}

impl Lexer {
    pub fn new(language: Language) -> Self {
        Self {
            language,
            mode: Mode::Plain,
        }
    }

    pub fn line(
        &mut self,
        line: &str,
        cancel: &CancelToken,
        mut emit: impl FnMut(&str, Option<SyntaxRole>) -> bool,
    ) -> Scan {
        let bytes = line.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            if cancel.is_cancelled() {
                return Scan::Cancelled;
            }
            let start = at;
            let role = match self.mode {
                Mode::Plain => {
                    let rest = &line[at..];
                    let c = rest.chars().next().unwrap();
                    if self.hash_comment(bytes, at) || (self.c_family() && rest.starts_with("//")) {
                        // No need to inspect a line-comment body.
                        at = bytes.len();
                        Some(SyntaxRole::Comment)
                    } else if self.c_family() && rest.starts_with("/*") {
                        self.mode = Mode::Block(1);
                        at += 2;
                        self.block(bytes, &mut at, cancel);
                        Some(SyntaxRole::Comment)
                    } else if let Some((opening, hashes)) = self.raw_rust(rest) {
                        at += opening;
                        self.mode = Mode::RawRust(hashes);
                        self.quoted(bytes, &mut at, cancel);
                        Some(SyntaxRole::String)
                    } else if matches!(c, '"' | '\'')
                        && (c != '\'' || self.language != Language::Rust || rust_char(rest))
                        || c == '`'
                            && matches!(
                                self.language,
                                Language::JavaScript
                                    | Language::TypeScript
                                    | Language::Qml
                                    | Language::Go
                                    | Language::Shell
                            )
                    {
                        let triple = self.language == Language::Python
                            && bytes.get(at..at + 3).is_some_and(|b| b == [c as u8; 3]);
                        at += if triple { 3 } else { 1 };
                        self.mode = Mode::Quoted {
                            byte: c as u8,
                            triple,
                        };
                        self.quoted(bytes, &mut at, cancel);
                        Some(SyntaxRole::String)
                    } else if c.is_ascii_digit() {
                        let cpp_hex = self.language == Language::Cpp
                            && (rest.starts_with("0x") || rest.starts_with("0X"));
                        at += 1;
                        while at < bytes.len() && !cancel.is_cancelled() {
                            let b = bytes[at];
                            if b.is_ascii_alphanumeric()
                                || b == b'_'
                                // C++ separators belong to the number, not quote state.
                                || self.language == Language::Cpp
                                    && b == b'\''
                                    && bytes[at - 1].is_ascii_hexdigit()
                                    && bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit)
                                || b == b'.'
                                    && bytes.get(at + 1).is_some_and(|b| {
                                        b.is_ascii_digit() || cpp_hex && b.is_ascii_hexdigit()
                                    })
                                || matches!(b, b'+' | b'-')
                                    && matches!(bytes[at - 1], b'e' | b'E' | b'p' | b'P')
                            {
                                at += 1;
                            } else {
                                break;
                            }
                        }
                        Some(SyntaxRole::Number)
                    } else if c == '_' || c.is_alphabetic() {
                        at += c.len_utf8();
                        while at < bytes.len() && !cancel.is_cancelled() {
                            let next = line[at..].chars().next().unwrap();
                            if next == '_' || next.is_alphanumeric() {
                                at += next.len_utf8();
                            } else {
                                break;
                            }
                        }
                        self.keyword(&line[start..at])
                            .then_some(SyntaxRole::Keyword)
                    } else {
                        at += c.len_utf8();
                        None
                    }
                }
                Mode::Block(_) => {
                    self.block(bytes, &mut at, cancel);
                    Some(SyntaxRole::Comment)
                }
                Mode::Quoted { .. } | Mode::RawRust(_) => {
                    self.quoted(bytes, &mut at, cancel);
                    Some(SyntaxRole::String)
                }
            };
            if cancel.is_cancelled() {
                return Scan::Cancelled;
            }
            if !emit(&line[start..at], role) {
                return Scan::Limited;
            }
        }
        Scan::Complete
    }

    fn c_family(&self) -> bool {
        !matches!(self.language, Language::Python | Language::Shell)
    }
    fn hash_comment(&self, bytes: &[u8], at: usize) -> bool {
        bytes[at] == b'#'
            && (self.language == Language::Python
                || self.language == Language::Shell
                    && (at == 0
                        || bytes[at - 1].is_ascii_whitespace()
                        || b";|&()".contains(&bytes[at - 1])))
    }
    fn raw_rust(&self, text: &str) -> Option<(usize, usize)> {
        if self.language != Language::Rust {
            return None;
        }
        let prefix = if text.starts_with("br") {
            2
        } else if text.starts_with('r') {
            1
        } else {
            return None;
        };
        let hashes = text.as_bytes()[prefix..]
            .iter()
            .take_while(|b| **b == b'#')
            .count();
        (text.as_bytes().get(prefix + hashes) == Some(&b'"'))
            .then_some((prefix + hashes + 1, hashes))
    }
    fn block(&mut self, bytes: &[u8], at: &mut usize, cancel: &CancelToken) {
        let Mode::Block(mut depth) = self.mode else {
            return;
        };
        while *at < bytes.len() {
            if cancel.is_cancelled() {
                break;
            }
            if bytes[*at..].starts_with(b"*/") {
                *at += 2;
                depth -= 1;
                if depth == 0 {
                    self.mode = Mode::Plain;
                    return;
                }
            } else if self.language == Language::Rust && bytes[*at..].starts_with(b"/*") {
                *at += 2;
                depth += 1;
            } else {
                *at += 1;
            }
        }
        self.mode = Mode::Block(depth);
    }
    fn quoted(&mut self, bytes: &[u8], at: &mut usize, cancel: &CancelToken) {
        while *at < bytes.len() {
            if cancel.is_cancelled() {
                return;
            }
            let close = match self.mode {
                Mode::RawRust(hashes) => {
                    bytes[*at] == b'"'
                        && bytes
                            .get(*at + 1..*at + 1 + hashes)
                            .is_some_and(|b| b.iter().all(|b| *b == b'#'))
                }
                Mode::Quoted { byte, triple } => {
                    if triple {
                        bytes.get(*at..*at + 3).is_some_and(|b| b == [byte; 3])
                    } else {
                        bytes[*at] == byte
                    }
                }
                _ => return,
            };
            if close {
                *at += match self.mode {
                    Mode::RawRust(hashes) => 1 + hashes,
                    Mode::Quoted { triple: true, .. } => 3,
                    _ => 1,
                };
                self.mode = Mode::Plain;
                return;
            }
            let escaped = bytes[*at] == b'\\'
                && !matches!(self.mode, Mode::RawRust(_))
                && !(self.language == Language::Go
                    && matches!(self.mode, Mode::Quoted { byte: b'`', .. }))
                && !(self.language == Language::Shell
                    && matches!(self.mode, Mode::Quoted { byte: b'\'', .. }));
            *at += if escaped && *at + 1 < bytes.len() {
                2
            } else {
                1
            };
        }
    }
    fn keyword(&self, word: &str) -> bool {
        let words = match self.language {
            Language::Rust => "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while",
            Language::Python => "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield",
            Language::JavaScript | Language::TypeScript => "abstract as async await break case catch class const continue debugger declare default delete do else enum export extends false finally for from function get if implements import in instanceof interface let module namespace new null of private protected public readonly return set static super switch this throw true try type typeof undefined var void while with yield",
            Language::Qml => "as async await break case catch class const continue default delete do else false finally for function if import in instanceof let new null on pragma property readonly required return signal super switch this throw true try typeof undefined var void while with yield",
            Language::C | Language::Cpp => "alignas alignof auto bool break case catch char class const constexpr continue default delete do double else enum explicit extern false float for friend if inline int long namespace new noexcept nullptr operator private protected public register reinterpret_cast return short signed sizeof static static_cast struct switch template this throw true try typedef typename union unsigned using virtual void volatile while",
            Language::Shell => "case do done elif else esac export fi for function if in local readonly return select then time until while",
            Language::Go => "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
            Language::Java => "abstract assert boolean break byte case catch char class const continue default do double else enum extends false final finally float for if implements import instanceof int interface long native new null package private protected public return short static strictfp super switch synchronized this throw throws transient true try void volatile while",
        };
        words
            .split_ascii_whitespace()
            .any(|keyword| keyword == word)
    }
}

fn rust_char(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next();
    if chars.next() == Some('\\') {
        // Character escapes are short; lifetimes have no closing quote.
        text.get(2..)
            .is_some_and(|tail| tail.chars().take(12).any(|c| c == '\''))
    } else {
        chars.next() == Some('\'')
    }
}
