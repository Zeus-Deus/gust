use gust_core::{structured::*, syntax::*, CancelToken};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;

fn source(bytes: &[u8]) -> PreviewSource {
    PreviewSource {
        bytes: bytes.to_vec(),
        stamp: FileStamp {
            dev: 0,
            ino: 0,
            size: bytes.len() as u64,
            mtime: 0,
            mtime_ns: 0,
            ctime: 0,
            ctime_ns: 0,
        },
        truncated: false,
    }
}
fn render(bytes: &[u8], name: &str) -> CodePreview {
    preview_source(
        &source(bytes),
        OsStr::new(name),
        PreviewWindow::default(),
        &PreviewLimits::default(),
        &CancelToken::new(),
    )
}
fn text(result: &CodePreview) -> &[String] {
    match result.preview.payload.as_ref().unwrap() {
        PreviewPayload::Text { lines, .. } => lines,
        _ => panic!("text expected"),
    }
}
fn piece(line: &str, span: &SyntaxSpan) -> String {
    let encoded: Vec<_> = line.encode_utf16().collect();
    String::from_utf16(&encoded[span.start_utf16..span.start_utf16 + span.len_utf16]).unwrap()
}
fn assert_ranges(result: &CodePreview) {
    assert_eq!(result.syntax.lines.len(), text(result).len());
    for (line, spans) in text(result).iter().zip(&result.syntax.lines) {
        let mut boundary = vec![0];
        for c in line.chars() {
            boundary.push(boundary.last().unwrap() + c.len_utf16());
        }
        let mut previous = 0;
        for span in spans {
            assert!(span.len_utf16 > 0);
            assert!(span.start_utf16 >= previous);
            assert!(boundary.contains(&span.start_utf16));
            previous = span.start_utf16 + span.len_utf16;
            assert!(
                boundary.contains(&previous),
                "surrogate split/out of bounds: {span:?} {line:?}"
            );
        }
    }
}

#[test]
fn common_families_use_the_exact_serialized_contract() {
    for (name, code, language, keyword) in [
        ("a.rs", "let x = 42; // note", "rust", "let"),
        ("a.py", "return 42 # note", "python", "return"),
        ("a.js", "const x = 42; // note", "javascript", "const"),
        ("a.ts", "interface X {} // note", "typescript", "interface"),
        ("a.qml", "property int x: 42 // note", "qml", "property"),
        ("a.c", "return 42; // note", "c", "return"),
        ("a.cpp", "class X {}; // note", "cpp", "class"),
        ("a.sh", "return 42 # note", "shell", "return"),
        ("a.go", "func x() {} // note", "go", "func"),
        ("a.java", "class X {} // note", "java", "class"),
    ] {
        let result = render(code.as_bytes(), name);
        assert_eq!(result.syntax.state, SyntaxState::Complete);
        let json = serde_json::to_value(&result.syntax).unwrap();
        assert_eq!(json["language"], language);
        assert_eq!(json["state"], "complete");
        assert_eq!(json["lines"][0][0]["role"], "keyword");
        assert!(result.syntax.lines[0]
            .iter()
            .any(|s| s.role == SyntaxRole::Keyword && piece(&text(&result)[0], s) == keyword));
        assert!(result.syntax.lines[0]
            .iter()
            .any(|s| s.role == SyntaxRole::Comment));
        assert_ranges(&result);
    }
}

#[test]
fn ranges_use_sanitized_window_text_and_never_split_surrogates() {
    let input = "outside\n\t😀\u{202e} let café = \"𝄞\ttext\"; // end\n";
    let result = preview_source(
        &source(input.as_bytes()),
        OsStr::new("a.rs"),
        PreviewWindow { start: 1, count: 1 },
        &PreviewLimits::default(),
        &CancelToken::new(),
    );
    assert_eq!(text(&result), ["    😀· let café = \"𝄞    text\"; // end"]);
    assert_ranges(&result);
    let spans = &result.syntax.lines[0];
    let keyword = spans
        .iter()
        .find(|s| s.role == SyntaxRole::Keyword)
        .unwrap();
    assert_eq!(keyword.start_utf16, 8);
    assert_eq!(piece(&text(&result)[0], keyword), "let");
    assert_eq!(
        piece(
            &text(&result)[0],
            spans.iter().find(|s| s.role == SyntaxRole::String).unwrap()
        ),
        "\"𝄞    text\""
    );
    assert!(!text(&result)[0].contains('\u{202e}'));
}

#[test]
fn paging_carries_nested_comments_and_raw_string_state_from_the_prefix() {
    let input=b"/* outer\n/* nested */ still\n*/ let text = r##\"start\n// raw not comment\n\"##; let n = 3;\n";
    let result = preview_source(
        &source(input),
        OsStr::new("code.rs"),
        PreviewWindow { start: 1, count: 4 },
        &PreviewLimits::default(),
        &CancelToken::new(),
    );
    assert_ranges(&result);
    assert_eq!(result.syntax.lines[0][0].role, SyntaxRole::Comment);
    assert_eq!(
        piece(
            &text(&result)[1],
            result.syntax.lines[1]
                .iter()
                .find(|s| s.role == SyntaxRole::Keyword)
                .unwrap()
        ),
        "let"
    );
    assert_eq!(result.syntax.lines[2].len(), 1);
    assert_eq!(result.syntax.lines[2][0].role, SyntaxRole::String);
    assert!(result.syntax.lines[3]
        .iter()
        .any(|s| s.role == SyntaxRole::Number));
}

#[test]
fn python_triple_quotes_and_js_templates_do_not_turn_contents_into_comments() {
    for (name, input) in [
        (
            "x.py",
            "text = \"\"\"start\n# still string\n\"\"\"\nreturn 3",
        ),
        ("x.js", "const text = `start\n// still string\n`;\nreturn 3"),
    ] {
        let result = preview_source(
            &source(input.as_bytes()),
            OsStr::new(name),
            PreviewWindow { start: 1, count: 3 },
            &PreviewLimits::default(),
            &CancelToken::new(),
        );
        assert_ranges(&result);
        assert_eq!(result.syntax.lines[0][0].role, SyntaxRole::String);
        assert!(result.syntax.lines[2]
            .iter()
            .any(|s| s.role == SyntaxRole::Keyword));
    }
}

#[test]
fn display_byte_clipping_retains_plain_text_and_safe_span_boundaries() {
    let input = "\tlet s = \"😀\ttext\";";
    for max in 0..36 {
        let limits = PreviewLimits {
            max_cell_bytes: max,
            ..Default::default()
        };
        let source = source(input.as_bytes());
        let plain = source.parse(
            PreviewFormat::Text,
            PreviewWindow::default(),
            &limits,
            &CancelToken::new(),
        );
        let code = preview_source(
            &source,
            OsStr::new("x.rs"),
            PreviewWindow::default(),
            &limits,
            &CancelToken::new(),
        );
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::to_value(&code.preview).unwrap()
        );
        assert!(text(&code)[0].len() <= max);
        assert_ranges(&code);
    }
}

#[test]
fn span_cap_is_explicit_without_omitting_text_or_unstyled_rows() {
    let input = ("1 ".repeat(200) + "\n").repeat(100);
    let result = render(input.as_bytes(), "a.py");
    assert_eq!(result.syntax.state, SyntaxState::Limited);
    assert_eq!(result.preview.state, PreviewState::Complete);
    assert_eq!(text(&result).len(), 100);
    assert_eq!(
        result.syntax.lines.iter().map(Vec::len).sum::<usize>(),
        MAX_SYNTAX_SPANS
    );
    assert!(result.syntax.lines.last().unwrap().is_empty());
    assert_ranges(&result);
    let limits = PreviewLimits {
        max_cell_bytes: 64 * 1024,
        ..Default::default()
    };
    let exact = preview_source(
        &source("1 ".repeat(MAX_SYNTAX_SPANS).as_bytes()),
        OsStr::new("a.py"),
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert_eq!(exact.syntax.state, SyntaxState::Complete);
    assert_eq!(exact.syntax.lines[0].len(), MAX_SYNTAX_SPANS);
}

#[test]
fn plain_fallback_preserves_markup_and_language_routing_stays_bytes() {
    let input = b"<script>not executable</script>";
    let result = render(input, "unknown.txt");
    assert_eq!(result.syntax.state, SyntaxState::Unsupported);
    assert_eq!(result.syntax.language, None);
    assert!(result.syntax.lines.is_empty());
    assert_eq!(text(&result), ["<script>not executable</script>"]);
    assert_eq!(
        language_for_name(&OsString::from_vec(b"raw\xff.RS".to_vec())),
        Some(Language::Rust)
    );
    assert_eq!(
        language_for_name(&OsString::from_vec(b"file.r\xff".to_vec())),
        None
    );
}

#[test]
fn invalid_binary_and_other_encodings_publish_no_annotations() {
    for (bytes, state) in [
        (b"a\0b".as_slice(), PreviewState::Binary),
        (b"\xff\xfea\0", PreviewState::UnsupportedEncoding),
    ] {
        let result = render(bytes, "a.rs");
        assert_eq!(result.preview.state, state);
        assert_eq!(result.syntax.state, SyntaxState::Unavailable);
        assert!(result.syntax.lines.is_empty());
    }
    let result = render(b"bad\xff", "a.py");
    assert!(matches!(result.preview.state, PreviewState::Invalid(_)));
    assert!(result.preview.payload.is_none());
    assert!(result.syntax.lines.is_empty());
}

#[test]
fn bom_and_partial_unicode_at_a_byte_bound_share_existing_preview_rules() {
    let limits = PreviewLimits {
        max_bytes: 11,
        ..Default::default()
    };
    let result = preview_source(
        &source("\u{feff}let x = 😀".as_bytes()),
        OsStr::new("a.rs"),
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(matches!(result.preview.state, PreviewState::Truncated(_)));
    assert_ranges(&result);
    assert_eq!(piece(&text(&result)[0], &result.syntax.lines[0][0]), "let");
    assert!(!text(&result)[0].contains('\u{feff}'));
}

#[test]
fn blank_unstyled_and_beyond_end_windows_retain_exact_alignment() {
    let result = render(b"\nplain identifier\n\n", "a.py");
    assert_eq!(result.syntax.lines.len(), 3);
    assert!(result.syntax.lines.iter().all(Vec::is_empty));
    for start in [0, 999, usize::MAX] {
        let result = preview_source(
            &source(b"let x = 1"),
            OsStr::new("a.rs"),
            PreviewWindow { start, count: 0 },
            &PreviewLimits::default(),
            &CancelToken::new(),
        );
        assert_eq!(result.syntax.state, SyntaxState::Complete);
        assert_ranges(&result);
        assert!(result.syntax.lines.is_empty());
    }
}

#[test]
fn cancellation_clears_both_the_preview_and_all_roles() {
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = preview_source(
        &source(b"let x = 42;"),
        OsStr::new("a.rs"),
        PreviewWindow::default(),
        &PreviewLimits::default(),
        &cancel,
    );
    assert_eq!(result.preview.state, PreviewState::Cancelled);
    assert!(result.preview.payload.is_none());
    assert_eq!(result.syntax.state, SyntaxState::Cancelled);
    assert!(result.syntax.lines.is_empty());
}

const CPP_SEPARATED_NUMBERS: [&str; 14] = [
    "1'000",
    "0xCA'FE",
    "0b1010'0101",
    "1.23'45",
    "1e1'0",
    "0x1.A'Bp1'0",
    "1.e1'0",
    "1.E1'0f",
    "1.e+1'0F",
    "1.E-1'0L",
    "0x1.p1'0",
    "0XCA.P1'0L",
    "0xA.p+1'0f",
    "0XCA.P-1'0F",
];

#[test]
fn cpp_digit_separators_preserve_following_keywords_and_character_literals() {
    for literal in CPP_SEPARATED_NUMBERS {
        let input = format!("auto n = {literal};\nreturn n;\nauto c = 'x';\n");
        let result = render(input.as_bytes(), "x.cpp");
        assert_ranges(&result);
        assert!(
            result.syntax.lines[1]
                .iter()
                .any(|s| s.role == SyntaxRole::Keyword && piece(&text(&result)[1], s) == "return"),
            "following keyword after {literal}"
        );
        assert!(
            result.syntax.lines[0]
                .iter()
                .any(|s| s.role == SyntaxRole::Number && piece(&text(&result)[0], s) == literal),
            "one numeric token for {literal}"
        );
        assert!(result.syntax.lines[2]
            .iter()
            .any(|s| s.role == SyntaxRole::String && piece(&text(&result)[2], s) == "'x'"));
    }
}

#[test]
fn cpp_digit_separators_do_not_poison_a_window_after_the_number() {
    for literal in CPP_SEPARATED_NUMBERS {
        let input = format!("auto n = {literal};\nreturn n;\nauto c = 'x';\n");
        let result = preview_source(
            &source(input.as_bytes()),
            OsStr::new("x.cpp"),
            PreviewWindow { start: 1, count: 2 },
            &PreviewLimits::default(),
            &CancelToken::new(),
        );
        assert_ranges(&result);
        assert_eq!(result.syntax.state, SyntaxState::Complete);
        assert!(
            result.syntax.lines[0]
                .iter()
                .any(|s| s.role == SyntaxRole::Keyword && piece(&text(&result)[0], s) == "return"),
            "paged keyword after {literal}"
        );
        assert_eq!(text(&result), ["return n;", "auto c = 'x';"]);
        assert!(result.syntax.lines[1]
            .iter()
            .any(|s| s.role == SyntaxRole::String && piece(&text(&result)[1], s) == "'x'"));
    }
}
