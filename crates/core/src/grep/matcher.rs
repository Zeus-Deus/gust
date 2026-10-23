use super::MAX_FILE_BYTES;
use crate::CancelToken;
use serde::Serialize;

pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_SNIPPET_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentQueryError {
    Empty,
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContentSnippet {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentMatchError {
    Cancelled,
    InvalidQuery(ContentQueryError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatches {
    pub lines: u32,
    pub first_line: u32,
    pub snippet: Option<ContentSnippet>,
    pub bytes: usize,
    pub truncated: bool,
    pub ascii_fallback: bool,
}

/// Pure bounded matching. The tuple compatibility API cannot disclose these limits.
pub fn match_buffer(
    data: &[u8],
    needle: &str,
    cancel: &CancelToken,
) -> Result<ContentMatches, ContentMatchError> {
    if cancel.is_cancelled() {
        return Err(ContentMatchError::Cancelled);
    }
    let mut matcher = Matcher::new(needle).map_err(ContentMatchError::InvalidQuery)?;
    matcher.buffer(data, false, cancel)
}

fn prefix<T: Eq>(pattern: &[T]) -> Vec<usize> {
    let mut table = vec![0; pattern.len()];
    let mut matched = 0;
    for i in 1..pattern.len() {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = table[matched - 1];
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
        }
        table[i] = matched;
    }
    table
}

pub(super) struct Matcher {
    chars: Vec<char>,
    char_prefix: Vec<usize>,
    starts: Vec<usize>,
    bytes: Vec<u8>,
    byte_prefix: Vec<usize>,
}
impl Matcher {
    pub fn new(needle: &str) -> Result<Self, ContentQueryError> {
        if needle.is_empty() {
            return Err(ContentQueryError::Empty);
        }
        if needle.len() > MAX_QUERY_BYTES {
            return Err(ContentQueryError::TooLong);
        }
        let chars: Vec<_> = needle.chars().flat_map(char::to_lowercase).collect();
        let bytes = needle.as_bytes().to_ascii_lowercase();
        Ok(Self {
            char_prefix: prefix(&chars),
            starts: vec![0; chars.len()],
            chars,
            byte_prefix: prefix(&bytes),
            bytes,
        })
    }
    pub fn buffer(
        &mut self,
        data: &[u8],
        source_truncated: bool,
        cancel: &CancelToken,
    ) -> Result<ContentMatches, ContentMatchError> {
        let truncated = source_truncated || data.len() > MAX_FILE_BYTES as usize;
        let data = &data[..data.len().min(MAX_FILE_BYTES as usize)];
        let valid = match std::str::from_utf8(data) {
            Ok(s) => Some(s),
            Err(e) if truncated && e.error_len().is_none() => {
                std::str::from_utf8(&data[..e.valid_up_to()]).ok()
            }
            Err(_) => None,
        };
        let data = valid.map(str::as_bytes).unwrap_or(data);
        let mut result = ContentMatches {
            lines: 0,
            first_line: 0,
            snippet: None,
            bytes: data.len(),
            truncated,
            ascii_fallback: valid.is_none(),
        };
        let mut offset = 0;
        for (index, line) in data.split(|b| *b == b'\n').enumerate() {
            if cancel.is_cancelled() {
                return Err(ContentMatchError::Cancelled);
            }
            let found = if valid.is_some() {
                self.unicode(std::str::from_utf8(line).unwrap(), cancel)?
            } else {
                self.ascii(line, cancel)?
            };
            if let Some(start) = found {
                result.lines += 1;
                if result.first_line == 0 {
                    result.first_line = (index + 1) as u32;
                    let mut snippet = snippet(line, start);
                    snippet.truncated |= truncated && offset + line.len() == data.len();
                    result.snippet = Some(snippet);
                }
            }
            offset += line.len() + 1;
        }
        if cancel.is_cancelled() {
            return Err(ContentMatchError::Cancelled);
        }
        Ok(result)
    }
    fn unicode(
        &mut self,
        line: &str,
        cancel: &CancelToken,
    ) -> Result<Option<usize>, ContentMatchError> {
        let mut matched = 0;
        let mut seen = 0;
        for (offset, c) in line.char_indices() {
            if cancel.is_cancelled() {
                return Err(ContentMatchError::Cancelled);
            }
            for folded in c.to_lowercase() {
                self.starts[seen % self.chars.len()] = offset;
                seen += 1;
                while matched > 0 && folded != self.chars[matched] {
                    matched = self.char_prefix[matched - 1];
                }
                if folded == self.chars[matched] {
                    matched += 1;
                }
                if matched == self.chars.len() {
                    return Ok(Some(
                        self.starts[(seen - self.chars.len()) % self.chars.len()],
                    ));
                }
            }
        }
        Ok(None)
    }
    fn ascii(&self, line: &[u8], cancel: &CancelToken) -> Result<Option<usize>, ContentMatchError> {
        let mut matched = 0;
        for (index, byte) in line.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(ContentMatchError::Cancelled);
            }
            let byte = byte.to_ascii_lowercase();
            while matched > 0 && byte != self.bytes[matched] {
                matched = self.byte_prefix[matched - 1];
            }
            if byte == self.bytes[matched] {
                matched += 1;
            }
            if matched == self.bytes.len() {
                return Ok(Some(index + 1 - self.bytes.len()));
            }
        }
        Ok(None)
    }
}

fn snippet(line: &[u8], matched: usize) -> ContentSnippet {
    // At most 32 source bytes of context leave room after worst-case tab expansion.
    let mut start = matched.saturating_sub(32);
    if let Ok(text) = std::str::from_utf8(line) {
        while !text.is_char_boundary(start) {
            start += 1;
        }
    }
    let mut out = ContentSnippet {
        text: String::new(),
        truncated: start > 0,
    };
    let mut fragment = String::with_capacity(10);
    let mut push = |c| {
        fragment.clear();
        crate::peek::push_sanitised_char(&mut fragment, c);
        let mut end = fragment.len().min(MAX_SNIPPET_BYTES - out.text.len());
        while !fragment.is_char_boundary(end) {
            end -= 1;
        }
        out.text.push_str(&fragment[..end]);
        if end < fragment.len() {
            out.truncated = true;
            false
        } else {
            true
        }
    };
    for chunk in line[start..].utf8_chunks() {
        for c in chunk.valid().chars() {
            if !push(c) {
                return out;
            }
        }
        if !chunk.invalid().is_empty() && !push('·') {
            return out;
        }
    }
    out
}
