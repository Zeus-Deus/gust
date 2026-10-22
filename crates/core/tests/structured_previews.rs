use gust_core::{structured::*, CancelToken};
fn p(bytes: &[u8], format: PreviewFormat) -> Preview {
    parse(
        bytes,
        format,
        PreviewWindow::default(),
        &PreviewLimits::default(),
        &CancelToken::new(),
    )
}
fn rows(preview: Preview) -> Vec<Vec<String>> {
    match preview.payload.unwrap() {
        PreviewPayload::Table { rows, .. } => rows,
        _ => panic!("table expected"),
    }
}
fn has_limit(preview: &Preview, limit: PreviewLimit) -> bool {
    matches!(&preview.state,PreviewState::Truncated(limits) if limits.contains(&limit))
}
#[test]
fn csv_and_tsv_use_logical_records_with_bom_quotes_and_crlf() {
    let preview = p(
        b"\xef\xbb\xbfhead,value\r\n\"a,b\",\"line1\nline2\"\r\n\"a\"\"b\",last,\r\n",
        PreviewFormat::Csv,
    );
    assert_eq!(preview.state, PreviewState::Complete);
    assert_eq!(
        rows(preview),
        vec![
            vec!["head", "value"],
            vec!["a,b", "line1\nline2"],
            vec!["a\"b", "last", ""]
        ]
    );
    assert_eq!(
        rows(p(b"\"a\tb\"\t\"c\"\"d\"\n", PreviewFormat::Tsv)),
        vec![vec!["a    b", "c\"d"]]
    );
    for (input, count) in [
        (b"".as_slice(), 0),
        (b"\n", 1),
        (b"a\n", 1),
        (b",", 1),
        (b"\"\"", 1),
    ] {
        assert_eq!(rows(p(input, PreviewFormat::Csv)).len(), count);
    }
}
#[test]
fn record_windows_count_skipped_and_multiline_rows_correctly() {
    let limits = PreviewLimits::default();
    let input = b"id,name\n1,\"one\ntwo\"\n2,last\n3,tail\n";
    let result = parse(
        input,
        PreviewFormat::Csv,
        PreviewWindow { start: 1, count: 2 },
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&result, PreviewLimit::Rows));
    match result.payload.unwrap() {
        PreviewPayload::Table {
            rows,
            first_row,
            total_rows,
        } => {
            assert_eq!(first_row, 1);
            assert_eq!(total_rows, Some(4));
            assert_eq!(rows, vec![vec!["1", "one\ntwo"], vec!["2", "last"]]);
        }
        _ => panic!("table expected"),
    }
    let beyond = parse(
        input,
        PreviewFormat::Csv,
        PreviewWindow {
            start: usize::MAX,
            count: 20,
        },
        &limits,
        &CancelToken::new(),
    );
    assert!(rows(beyond).is_empty());
}
#[test]
fn malformed_data_binary_invalid_utf8_and_unsupported_boms_are_distinct() {
    for input in [b"\"unclosed".as_slice(), b"a\"b,c", b"\"a\"oops,b"] {
        assert!(matches!(
            p(input, PreviewFormat::Csv).state,
            PreviewState::Invalid(_)
        ));
    }
    assert_eq!(p(b"a\0b", PreviewFormat::Text).state, PreviewState::Binary);
    assert!(matches!(
        p(b"a\xff", PreviewFormat::Text).state,
        PreviewState::Invalid(_)
    ));
    assert_eq!(
        p(b"\xff\xfea\0", PreviewFormat::Text).state,
        PreviewState::UnsupportedEncoding
    );
    assert!(matches!(
        p(b"{bad}", PreviewFormat::Json).state,
        PreviewState::Invalid(_)
    ));
    assert!(matches!(
        p(b"a = [", PreviewFormat::Toml).state,
        PreviewState::Invalid(_)
    ));
}
#[test]
fn byte_bounds_do_not_publish_partial_records_or_claim_validated_documents() {
    let limits = PreviewLimits {
        max_bytes: 7,
        ..Default::default()
    };
    let preview = parse(
        b"a,b\n\"long cell\",x\n",
        PreviewFormat::Csv,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&preview, PreviewLimit::Bytes));
    match preview.payload.unwrap() {
        PreviewPayload::Table {
            rows, total_rows, ..
        } => {
            assert_eq!(rows, vec![vec!["a", "b"]]);
            assert_eq!(total_rows, None);
        }
        _ => panic!("table expected"),
    }
    let preview = parse(
        b"{\"long\":12345}",
        PreviewFormat::Json,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&preview, PreviewLimit::Bytes));
    assert!(preview.payload.is_none());
    let limits = PreviewLimits {
        max_bytes: 2,
        ..Default::default()
    };
    let preview = parse(
        "aé".as_bytes(),
        PreviewFormat::Text,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&preview, PreviewLimit::Bytes));
    match preview.payload.unwrap() {
        PreviewPayload::Text {
            lines, total_lines, ..
        } => {
            assert_eq!(lines, ["a"]);
            assert_eq!(total_lines, None);
        }
        _ => panic!(),
    }
}
#[test]
fn rows_columns_cells_nodes_and_depth_have_named_limits() {
    let limits = PreviewLimits {
        max_rows: 1,
        max_columns: 1,
        max_cell_bytes: 2,
        ..Default::default()
    };
    let result = parse(
        b"hello,x\nbye,y\n",
        PreviewFormat::Csv,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    for limit in [
        PreviewLimit::Rows,
        PreviewLimit::Columns,
        PreviewLimit::CellBytes,
    ] {
        assert!(has_limit(&result, limit));
    }
    assert_eq!(rows(result), vec![vec!["he"]]);
    let limits = PreviewLimits {
        max_nodes: 2,
        ..Default::default()
    };
    let result = parse(
        b"[1,2,3]",
        PreviewFormat::Json,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&result, PreviewLimit::Nodes));
    let limits = PreviewLimits {
        max_depth: 0,
        ..Default::default()
    };
    let result = parse(
        b"{\"nested\":{\"a\":1}}",
        PreviewFormat::Json,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&result, PreviewLimit::Depth));
}
#[test]
fn json_pointers_are_unambiguous_and_values_stay_plain_safe_text() {
    let result = p(
        br#"{"a/b":1,"a~b":"<script>","line\n":"x\u202ey","line<U+000A>":true}"#,
        PreviewFormat::Json,
    );
    assert_eq!(result.state, PreviewState::Complete);
    match result.payload.unwrap() {
        PreviewPayload::Tree { nodes } => {
            assert!(nodes.iter().any(|n| n.path == "/a~1b"));
            assert!(nodes
                .iter()
                .any(|n| n.path == "/a~0b" && n.value == "<script>"));
            let newline = nodes.iter().find(|n| n.path == "/line\n").unwrap();
            let literal = nodes.iter().find(|n| n.path == "/line<U+000A>").unwrap();
            assert_eq!(newline.display_path, literal.display_path);
            assert_ne!(newline.path, literal.path);
            assert!(!newline.value.contains('\u{202e}'));
        }
        _ => panic!(),
    }
    assert_eq!(
        p(b"\xef\xbb\xbf[1,true,null]", PreviewFormat::Json).state,
        PreviewState::Complete
    );
    assert_eq!(
        p(
            b"\xef\xbb\xbfkey = 'value'\ncount = 3\n",
            PreviewFormat::Toml
        )
        .state,
        PreviewState::Complete
    );
}
#[test]
fn text_windows_and_exact_bounds_are_truthful() {
    let limits = PreviewLimits {
        max_bytes: 4,
        ..Default::default()
    };
    assert_eq!(
        parse(
            b"one\n",
            PreviewFormat::Text,
            PreviewWindow::default(),
            &limits,
            &CancelToken::new()
        )
        .state,
        PreviewState::Complete
    );
    let result = parse(
        b"a\r\nb\nc\n",
        PreviewFormat::Text,
        PreviewWindow { start: 1, count: 1 },
        &PreviewLimits::default(),
        &CancelToken::new(),
    );
    match result.payload.unwrap() {
        PreviewPayload::Text {
            lines,
            first_line,
            total_lines,
        } => {
            assert_eq!(lines, ["b"]);
            assert_eq!(first_line, 1);
            assert_eq!(total_lines, Some(3));
        }
        _ => panic!(),
    }
}
#[test]
fn diff_aligns_insertions_and_reports_coarse_fallback_and_bounds() {
    let result = compare(
        b"a\nb\n",
        b"a\nnew\nb\n",
        &PreviewLimits::default(),
        &CancelToken::new(),
    );
    assert_eq!(result.state, PreviewState::Complete);
    match result.payload.unwrap() {
        PreviewPayload::Diff { lines } => {
            assert_eq!(
                lines.iter().map(|l| l.kind).collect::<Vec<_>>(),
                [DiffKind::Equal, DiffKind::Added, DiffKind::Equal]
            );
            assert_eq!((lines[2].left, lines[2].right), (Some(1), Some(2)));
        }
        _ => panic!(),
    }
    let limits = PreviewLimits {
        max_diff_work: 0,
        max_rows: 2,
        ..Default::default()
    };
    let result = compare(b"a\nb\n", b"c\nd\n", &limits, &CancelToken::new());
    assert!(has_limit(&result, PreviewLimit::DiffWork));
    assert!(has_limit(&result, PreviewLimit::Rows));
    assert!(
        matches!(compare(b"a\n",b"a",&PreviewLimits::default(),&CancelToken::new()).payload,Some(PreviewPayload::Diff {lines}) if lines.iter().any(|l|l.kind!=DiffKind::Equal))
    );
}
#[test]
fn cancellation_drops_payloads_and_does_not_open_a_path() {
    let cancel = CancelToken::new();
    cancel.cancel();
    for format in [
        PreviewFormat::Text,
        PreviewFormat::Json,
        PreviewFormat::Csv,
        PreviewFormat::Tsv,
        PreviewFormat::Toml,
    ] {
        let result = parse(
            b"hello",
            format,
            PreviewWindow::default(),
            &PreviewLimits::default(),
            &cancel,
        );
        assert_eq!(result.state, PreviewState::Cancelled);
        assert!(result.payload.is_none());
    }
    assert_eq!(
        read_source(std::path::Path::new("/nonexistent"), 20, &cancel).unwrap_err(),
        PreviewReadError::Cancelled
    );
}
#[test]
fn file_reads_are_stamped_exact_bounded_and_refuse_links_and_special_files() {
    let dir = std::env::temp_dir().join(format!("gust-structured-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("data.txt");
    std::fs::write(&path, b"one\n").unwrap();
    let source = read_source(&path, 4, &CancelToken::new()).unwrap();
    assert!(!source.truncated);
    assert_eq!(source.stamp.size, 4);
    let shorter = read_source(&path, 3, &CancelToken::new()).unwrap();
    assert!(shorter.truncated);
    let preview = shorter.parse(
        PreviewFormat::Text,
        PreviewWindow::default(),
        &PreviewLimits::default(),
        &CancelToken::new(),
    );
    assert!(has_limit(&preview, PreviewLimit::Bytes));
    std::os::unix::fs::symlink(&path, dir.join("link")).unwrap();
    assert!(read_source(&dir.join("link"), 4, &CancelToken::new()).is_err());
    rustix::fs::mknodat(
        rustix::fs::CWD,
        dir.join("pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR,
        0,
    )
    .unwrap();
    assert_eq!(
        read_source(&dir.join("pipe"), 4, &CancelToken::new()).unwrap_err(),
        PreviewReadError::NotRegular
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn json_numbers_preserve_precision_and_long_paths_are_explicitly_limited() {
    let result = p(
        br#"{"value":1234567890123456789012345678901234567890}"#,
        PreviewFormat::Json,
    );
    match result.payload.unwrap() {
        PreviewPayload::Tree { nodes } => {
            assert!(nodes
                .iter()
                .any(|n| n.value == "1234567890123456789012345678901234567890"));
        }
        _ => panic!(),
    }
    let limits = PreviewLimits {
        max_cell_bytes: 3,
        ..Default::default()
    };
    let result = parse(
        br#"{"long-key":{"nested":1}}"#,
        PreviewFormat::Json,
        PreviewWindow::default(),
        &limits,
        &CancelToken::new(),
    );
    assert!(has_limit(&result, PreviewLimit::CellBytes));
    match result.payload.unwrap() {
        PreviewPayload::Tree { nodes } => assert_eq!(nodes.len(), 1),
        _ => panic!(),
    }
    for c in ['\u{061c}', '\u{200e}', '\u{200f}'] {
        let result = p(format!("a{c}b").as_bytes(), PreviewFormat::Text);
        match result.payload.unwrap() {
            PreviewPayload::Text { lines, .. } => assert!(!lines[0].contains(c)),
            _ => panic!(),
        }
    }
}

#[test]
fn legacy_text_peek_uses_exact_limits_and_never_reads_a_fifo() {
    let dir = std::env::temp_dir().join(format!("gust-peek-bounds-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("data.txt");
    std::fs::write(&file, vec![b'x'; gust_core::peek::MAX_PEEK_BYTES as usize]).unwrap();
    assert!(!gust_core::peek::peek_text(&file).unwrap().1);
    std::fs::write(
        &file,
        vec![b'x'; gust_core::peek::MAX_PEEK_BYTES as usize + 1],
    )
    .unwrap();
    assert!(gust_core::peek::peek_text(&file).unwrap().1);
    let fifo = dir.join("fifo.txt");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR,
        0,
    )
    .unwrap();
    assert!(gust_core::peek::peek_text(&fifo).is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn parser_recursion_guards_are_reported_as_depth_limits() {
    let json = format!("{}0{}", "[".repeat(200), "]".repeat(200));
    assert!(has_limit(
        &p(json.as_bytes(), PreviewFormat::Json),
        PreviewLimit::Depth
    ));
    let toml = format!("value = {}0{}", "[".repeat(200), "]".repeat(200));
    assert!(has_limit(
        &p(toml.as_bytes(), PreviewFormat::Toml),
        PreviewLimit::Depth
    ));
}
