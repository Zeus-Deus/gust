//! Bounded plain-data preview payloads. No markup, decoder, subprocess or UI I/O.
use crate::lister::CancelToken;
use serde::Serialize;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const HARD_BYTES: usize = 4 * 1024 * 1024;
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewFormat {
    Text,
    Json,
    Csv,
    Tsv,
    Toml,
}
#[derive(Debug, Clone, Copy)]
pub struct PreviewWindow {
    pub start: usize,
    pub count: usize,
}
impl Default for PreviewWindow {
    fn default() -> Self {
        Self {
            start: 0,
            count: 200,
        }
    }
}
#[derive(Debug, Clone)]
pub struct PreviewLimits {
    pub max_bytes: usize,
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_cell_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_diff_work: usize,
}
impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024,
            max_rows: 1000,
            max_columns: 64,
            max_cell_bytes: 4096,
            max_nodes: 2000,
            max_depth: 32,
            max_diff_work: 1_000_000,
        }
    }
}
impl PreviewLimits {
    fn bounded(&self) -> Self {
        Self {
            max_bytes: self.max_bytes.min(HARD_BYTES),
            max_rows: self.max_rows.min(10_000),
            max_columns: self.max_columns.min(256),
            max_cell_bytes: self.max_cell_bytes.min(64 * 1024),
            max_nodes: self.max_nodes.min(10_000),
            max_depth: self.max_depth.min(64),
            max_diff_work: self.max_diff_work.min(2_000_000),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewLimit {
    Bytes,
    Rows,
    Columns,
    CellBytes,
    Nodes,
    Depth,
    DiffWork,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum PreviewState {
    Complete,
    Truncated(Vec<PreviewLimit>),
    Invalid(String),
    Binary,
    UnsupportedEncoding,
    Cancelled,
}
#[derive(Debug, Clone, Serialize)]
pub struct PreviewNode {
    pub path: String,
    pub display_path: String,
    pub kind: String,
    pub value: String,
    pub depth: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Equal,
    Removed,
    Added,
}
#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub text: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreviewPayload {
    Text {
        lines: Vec<String>,
        first_line: usize,
        total_lines: Option<usize>,
    },
    Table {
        rows: Vec<Vec<String>>,
        first_row: usize,
        total_rows: Option<usize>,
    },
    Tree {
        nodes: Vec<PreviewNode>,
    },
    Diff {
        lines: Vec<DiffLine>,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub state: PreviewState,
    pub payload: Option<PreviewPayload>,
    pub bytes: usize,
}
impl Preview {
    fn empty(state: PreviewState, bytes: usize) -> Self {
        Self {
            state,
            payload: None,
            bytes,
        }
    }
}
fn add_limit(limits: &mut Vec<PreviewLimit>, limit: PreviewLimit) {
    if !limits.contains(&limit) {
        limits.push(limit);
    }
}
fn state(limits: Vec<PreviewLimit>) -> PreviewState {
    if limits.is_empty() {
        PreviewState::Complete
    } else {
        PreviewState::Truncated(limits)
    }
}
fn clip_text(text: &str, max: usize, limited: &mut Vec<PreviewLimit>) -> String {
    clip_display(text, max, limited, |out, c| {
        if c == '\n' {
            out.push(c);
        } else {
            crate::peek::push_sanitised_char(out, c);
        }
    })
}
fn clip_display(
    text: &str,
    max: usize,
    limited: &mut Vec<PreviewLimit>,
    push_char: impl Fn(&mut String, char),
) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    // Render one character at a time so expansion never allocates a full,
    // over-budget display string. Both emitters share the existing UI policy.
    let mut fragment = String::with_capacity(10);
    for c in text.chars() {
        fragment.clear();
        push_char(&mut fragment, c);
        let mut end = fragment.len().min(max - out.len());
        while !fragment.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&fragment[..end]);
        if end < fragment.len() {
            add_limit(limited, PreviewLimit::CellBytes);
            break;
        }
    }
    out
}
fn utf8(bytes: &[u8], truncated: bool) -> Result<&str, PreviewState> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if bytes.starts_with(b"\xff\xfe")
        || bytes.starts_with(b"\xfe\xff")
        || bytes.starts_with(b"\0\0\xfe\xff")
    {
        return Err(PreviewState::UnsupportedEncoding);
    }
    if bytes.contains(&0) {
        return Err(PreviewState::Binary);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s),
        Err(e) if truncated && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()])
                .map_err(|_| PreviewState::Invalid("Invalid UTF-8".into()))
        }
        Err(_) => Err(PreviewState::Invalid("Invalid UTF-8".into())),
    }
}

/// Pure parsing of an explicitly supplied byte buffer. Call on a worker.
pub fn parse(
    bytes: &[u8],
    format: PreviewFormat,
    window: PreviewWindow,
    limits: &PreviewLimits,
    cancel: &CancelToken,
) -> Preview {
    parse_inner(bytes, format, window, limits, cancel, false)
}
fn parse_inner(
    bytes: &[u8],
    format: PreviewFormat,
    window: PreviewWindow,
    limits: &PreviewLimits,
    cancel: &CancelToken,
    source_truncated: bool,
) -> Preview {
    if cancel.is_cancelled() {
        return Preview::empty(PreviewState::Cancelled, 0);
    }
    let limits = limits.bounded();
    let truncated = source_truncated || bytes.len() > limits.max_bytes;
    let bytes = &bytes[..bytes.len().min(limits.max_bytes)];
    let mut limited = Vec::new();
    if truncated {
        limited.push(PreviewLimit::Bytes);
    }
    let text = match utf8(bytes, truncated) {
        Ok(t) => t,
        Err(s) => return Preview::empty(s, bytes.len()),
    };
    let window = PreviewWindow {
        start: window.start,
        count: window.count.min(limits.max_rows),
    };
    let payload = match format {
        PreviewFormat::Text => {
            let mut lines = Vec::new();
            let mut total = 0;
            for (i, line) in text.lines().enumerate() {
                if cancel.is_cancelled() {
                    return Preview::empty(PreviewState::Cancelled, bytes.len());
                }
                if i >= window.start && i - window.start < window.count {
                    lines.push(clip_text(line, limits.max_cell_bytes, &mut limited));
                } else if i >= window.start {
                    add_limit(&mut limited, PreviewLimit::Rows);
                }
                total += 1;
            }
            PreviewPayload::Text {
                lines,
                first_line: window.start,
                total_lines: (!truncated).then_some(total),
            }
        }
        PreviewFormat::Csv | PreviewFormat::Tsv => {
            let delimiter = if matches!(format, PreviewFormat::Csv) {
                b','
            } else {
                b'\t'
            };
            match table(
                text,
                delimiter,
                window,
                &limits,
                &mut limited,
                cancel,
                truncated,
            ) {
                Ok(p) => p,
                Err(s) => return Preview::empty(s, bytes.len()),
            }
        }
        PreviewFormat::Json | PreviewFormat::Toml => {
            let parsed: Result<serde_json::Value, String> = if matches!(format, PreviewFormat::Json)
            {
                match serde_json::from_str(text) {
                    Ok(value) => Ok(value),
                    Err(e) if truncated && e.is_eof() => {
                        return Preview::empty(state(limited), bytes.len())
                    }
                    Err(e) if e.to_string().starts_with("recursion limit exceeded") => {
                        add_limit(&mut limited, PreviewLimit::Depth);
                        return Preview::empty(state(limited), bytes.len());
                    }
                    Err(e) => Err(e.to_string()),
                }
            } else {
                match text.parse::<toml::Value>() {
                    Ok(value) => serde_json::to_value(value).map_err(|e| e.to_string()),
                    Err(e) if e.message().contains("recursion limit exceeded") => {
                        add_limit(&mut limited, PreviewLimit::Depth);
                        return Preview::empty(state(limited), bytes.len());
                    }
                    Err(_) if truncated => return Preview::empty(state(limited), bytes.len()),
                    Err(e) => Err(e.to_string()),
                }
            };
            let value = match parsed {
                Ok(v) => v,
                Err(e) => {
                    return Preview::empty(
                        PreviewState::Invalid(clip_text(&e, limits.max_cell_bytes, &mut limited)),
                        bytes.len(),
                    )
                }
            };
            let mut nodes = Vec::new();
            flatten(&value, "", 0, &limits, &mut limited, &mut nodes, cancel);
            PreviewPayload::Tree { nodes }
        }
    };
    if cancel.is_cancelled() {
        return Preview::empty(PreviewState::Cancelled, bytes.len());
    }
    Preview {
        state: state(limited),
        payload: Some(payload),
        bytes: bytes.len(),
    }
}

fn flatten(
    value: &serde_json::Value,
    path: &str,
    depth: usize,
    limits: &PreviewLimits,
    limited: &mut Vec<PreviewLimit>,
    nodes: &mut Vec<PreviewNode>,
    cancel: &CancelToken,
) {
    if cancel.is_cancelled() {
        return;
    }
    if nodes.len() >= limits.max_nodes {
        add_limit(limited, PreviewLimit::Nodes);
        return;
    }
    if depth > limits.max_depth {
        add_limit(limited, PreviewLimit::Depth);
        return;
    }
    use serde_json::Value;
    let (kind, display) = match value {
        Value::Object(o) => ("object", format!("{} fields", o.len())),
        Value::Array(a) => ("array", format!("{} items", a.len())),
        Value::String(s) => ("string", s.clone()),
        Value::Null => ("null", "null".into()),
        Value::Bool(b) => ("boolean", b.to_string()),
        Value::Number(n) => ("number", n.to_string()),
    };
    nodes.push(PreviewNode {
        path: path.to_owned(),
        display_path: clip_display(
            path,
            limits.max_cell_bytes,
            limited,
            crate::location::push_escaped_display_char,
        ),
        kind: kind.into(),
        value: clip_text(&display, limits.max_cell_bytes, limited),
        depth,
    });
    match value {
        Value::Object(o) => {
            for (key, value) in o {
                if cancel.is_cancelled() {
                    break;
                }
                if nodes.len() >= limits.max_nodes {
                    add_limit(limited, PreviewLimit::Nodes);
                    break;
                }
                let key = key.replace('~', "~0").replace('/', "~1");
                if path.len().saturating_add(key.len()).saturating_add(1) > limits.max_cell_bytes {
                    add_limit(limited, PreviewLimit::CellBytes);
                    continue;
                }
                flatten(
                    value,
                    &format!("{path}/{key}"),
                    depth + 1,
                    limits,
                    limited,
                    nodes,
                    cancel,
                );
            }
        }
        Value::Array(a) => {
            for (i, value) in a.iter().enumerate() {
                if cancel.is_cancelled() {
                    break;
                }
                if nodes.len() >= limits.max_nodes {
                    add_limit(limited, PreviewLimit::Nodes);
                    break;
                }
                if path
                    .len()
                    .saturating_add(i.to_string().len())
                    .saturating_add(1)
                    > limits.max_cell_bytes
                {
                    add_limit(limited, PreviewLimit::CellBytes);
                    continue;
                }
                flatten(
                    value,
                    &format!("{path}/{i}"),
                    depth + 1,
                    limits,
                    limited,
                    nodes,
                    cancel,
                );
            }
        }
        _ => {}
    }
}

fn table(
    text: &str,
    delimiter: u8,
    window: PreviewWindow,
    limits: &PreviewLimits,
    limited: &mut Vec<PreviewLimit>,
    cancel: &CancelToken,
    truncated: bool,
) -> Result<PreviewPayload, PreviewState> {
    // State: 0 field start, 1 unquoted, 2 quoted, 3 after closing quote.
    let mut mode = 0u8;
    let mut field = Vec::new();
    let mut row = Vec::new();
    let mut rows = Vec::new();
    let mut row_number = 0usize;
    let mut column = 0usize;
    let mut field_overflow = false;
    let mut ended_record = true;
    let data = text.as_bytes();
    let mut i = 0;
    while i < data.len() {
        if cancel.is_cancelled() {
            return Err(PreviewState::Cancelled);
        }
        let b = data[i];
        if b == b'"' {
            ended_record = false;
            match mode {
                0 => mode = 2,
                2 if data.get(i + 1) == Some(&b'"') => {
                    push_byte(&mut field, b, limits, &mut field_overflow);
                    i += 1;
                }
                2 => mode = 3,
                _ => {
                    return Err(PreviewState::Invalid(format!(
                        "Unexpected quote in record {}",
                        row_number + 1
                    )))
                }
            }
        } else if mode != 2 && (b == delimiter || b == b'\n' || b == b'\r') {
            finish_field(
                &mut field,
                &mut row,
                column,
                field_overflow,
                limits,
                limited,
            );
            column += 1;
            field_overflow = false;
            mode = 0;
            if b != delimiter {
                finish_row(&mut row, &mut rows, row_number, window, limited);
                row_number += 1;
                column = 0;
                ended_record = true;
                if b == b'\r' && data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            } else {
                ended_record = false;
            }
        } else {
            ended_record = false;
            if mode == 3 {
                return Err(PreviewState::Invalid(format!(
                    "Unexpected text after quote in record {}",
                    row_number + 1
                )));
            }
            if mode == 0 {
                mode = 1;
            }
            push_byte(&mut field, b, limits, &mut field_overflow);
        }
        i += 1;
    }
    if mode == 2 {
        if !truncated {
            return Err(PreviewState::Invalid(format!(
                "Unclosed quote in record {}",
                row_number + 1
            )));
        }
        // Never publish a partial logical record as a complete CSV row.
    } else if !ended_record && !truncated {
        finish_field(
            &mut field,
            &mut row,
            column,
            field_overflow,
            limits,
            limited,
        );
        finish_row(&mut row, &mut rows, row_number, window, limited);
        row_number += 1;
    }
    Ok(PreviewPayload::Table {
        rows,
        first_row: window.start,
        total_rows: (!truncated).then_some(row_number),
    })
}
fn push_byte(field: &mut Vec<u8>, byte: u8, limits: &PreviewLimits, overflow: &mut bool) {
    if field.len() < limits.max_cell_bytes {
        field.push(byte);
    } else {
        *overflow = true;
    }
}
fn finish_field(
    field: &mut Vec<u8>,
    row: &mut Vec<String>,
    column: usize,
    overflow: bool,
    limits: &PreviewLimits,
    limited: &mut Vec<PreviewLimit>,
) {
    if overflow {
        add_limit(limited, PreviewLimit::CellBytes);
    }
    if column < limits.max_columns {
        let mut end = field.len();
        while end > 0 && std::str::from_utf8(&field[..end]).is_err() {
            end -= 1;
        }
        let value = std::str::from_utf8(&field[..end]).unwrap_or_default();
        row.push(clip_text(value, limits.max_cell_bytes, limited));
    } else {
        add_limit(limited, PreviewLimit::Columns);
    }
    field.clear();
}
fn finish_row(
    row: &mut Vec<String>,
    rows: &mut Vec<Vec<String>>,
    number: usize,
    window: PreviewWindow,
    limited: &mut Vec<PreviewLimit>,
) {
    if number >= window.start && number - window.start < window.count {
        rows.push(std::mem::take(row));
    } else {
        row.clear();
        if number >= window.start {
            add_limit(limited, PreviewLimit::Rows);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileStamp {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime: i64,
    pub mtime_ns: i64,
    pub ctime: i64,
    pub ctime_ns: i64,
}
impl FileStamp {
    pub fn from_metadata(m: &std::fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewReadError {
    Io(std::io::ErrorKind),
    NotRegular,
    Cancelled,
    Changed,
}
impl From<std::io::Error> for PreviewReadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.kind())
    }
}
#[derive(Debug, Clone)]
pub struct PreviewSource {
    pub bytes: Vec<u8>,
    pub stamp: FileStamp,
    pub truncated: bool,
}
impl PreviewSource {
    pub fn parse(
        &self,
        format: PreviewFormat,
        window: PreviewWindow,
        limits: &PreviewLimits,
        cancel: &CancelToken,
    ) -> Preview {
        parse_inner(&self.bytes, format, window, limits, cancel, self.truncated)
    }
}
/// Worker-only. Reject links/special files, retain a stamp, and detect exact limits.
pub fn read_source(
    path: &Path,
    max_bytes: usize,
    cancel: &CancelToken,
) -> Result<PreviewSource, PreviewReadError> {
    if cancel.is_cancelled() {
        return Err(PreviewReadError::Cancelled);
    }
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(PreviewReadError::NotRegular);
    }
    let stamp = FileStamp::from_metadata(&meta);
    let max = max_bytes.min(HARD_BYTES);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    while bytes.len() <= max {
        if cancel.is_cancelled() {
            return Err(PreviewReadError::Cancelled);
        }
        let want = (max + 1 - bytes.len()).min(buf.len());
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    if cancel.is_cancelled() {
        return Err(PreviewReadError::Cancelled);
    }
    if FileStamp::from_metadata(&file.metadata()?) != stamp
        || FileStamp::from_metadata(&std::fs::symlink_metadata(path)?) != stamp
    {
        return Err(PreviewReadError::Changed);
    }
    let truncated = bytes.len() > max;
    bytes.truncate(max);
    Ok(PreviewSource {
        bytes,
        stamp,
        truncated,
    })
}

/// Bounded line diff. A work-limit fallback emits an honest coarse edit script.
pub fn compare(left: &[u8], right: &[u8], limits: &PreviewLimits, cancel: &CancelToken) -> Preview {
    if cancel.is_cancelled() {
        return Preview::empty(PreviewState::Cancelled, 0);
    }
    let limits = limits.bounded();
    let mut limited = Vec::new();
    let left_truncated = left.len() > limits.max_bytes;
    let right_truncated = right.len() > limits.max_bytes;
    let truncated = left_truncated || right_truncated;
    if truncated {
        limited.push(PreviewLimit::Bytes);
    }
    let left = &left[..left.len().min(limits.max_bytes)];
    let right = &right[..right.len().min(limits.max_bytes)];
    let a = match utf8(left, left_truncated) {
        Ok(s) => s,
        Err(s) => return Preview::empty(s, 0),
    };
    let b = match utf8(right, right_truncated) {
        Ok(s) => s,
        Err(s) => return Preview::empty(s, 0),
    };
    let a: Vec<_> = a.split_inclusive('\n').take(limits.max_rows + 1).collect();
    let b: Vec<_> = b.split_inclusive('\n').take(limits.max_rows + 1).collect();
    if a.len() > limits.max_rows || b.len() > limits.max_rows {
        add_limit(&mut limited, PreviewLimit::Rows);
    }
    let a = &a[..a.len().min(limits.max_rows)];
    let b = &b[..b.len().min(limits.max_rows)];
    let work = (a.len() + 1).saturating_mul(b.len() + 1);
    let mut lines = Vec::new();
    let mut emit = |kind: DiffKind, i: Option<usize>, j: Option<usize>, text: &str| {
        if lines.len() < limits.max_rows {
            lines.push(DiffLine {
                kind,
                left: i,
                right: j,
                text: clip_text(
                    text.trim_end_matches('\n'),
                    limits.max_cell_bytes,
                    &mut limited,
                ),
            });
        } else {
            add_limit(&mut limited, PreviewLimit::Rows);
        }
    };
    if work <= limits.max_diff_work {
        let width = b.len() + 1;
        let mut lcs = vec![0u32; work];
        for i in (0..a.len()).rev() {
            if cancel.is_cancelled() {
                return Preview::empty(PreviewState::Cancelled, left.len() + right.len());
            }
            for j in (0..b.len()).rev() {
                lcs[i * width + j] = if a[i] == b[j] {
                    1 + lcs[(i + 1) * width + j + 1]
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if cancel.is_cancelled() {
                return Preview::empty(PreviewState::Cancelled, left.len() + right.len());
            }
            if i < a.len() && j < b.len() && a[i] == b[j] {
                emit(DiffKind::Equal, Some(i), Some(j), a[i]);
                i += 1;
                j += 1;
            } else if i < a.len()
                && (j == b.len() || lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])
            {
                emit(DiffKind::Removed, Some(i), None, a[i]);
                i += 1;
            } else {
                emit(DiffKind::Added, None, Some(j), b[j]);
                j += 1;
            }
        }
    } else {
        for (i, line) in a.iter().enumerate() {
            emit(DiffKind::Removed, Some(i), None, line);
        }
        for (j, line) in b.iter().enumerate() {
            emit(DiffKind::Added, None, Some(j), line);
        }
        add_limit(&mut limited, PreviewLimit::DiffWork);
    }
    if cancel.is_cancelled() {
        return Preview::empty(PreviewState::Cancelled, left.len() + right.len());
    }
    Preview {
        state: state(limited),
        payload: Some(PreviewPayload::Diff { lines }),
        bytes: left.len() + right.len(),
    }
}
