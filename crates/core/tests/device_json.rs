use gust_core::devices::*;
use serde_json::{json, Value};
use std::path::Path;

fn row() -> Value {
    json!({"name":"fixture","path":"/dev/fixture","type":"part"})
}
fn inventory(row: Value) -> String {
    json!({"blockdevices":[row]}).to_string()
}

#[test]
fn braces_in_labels_and_mountpoints_preserve_mounted_state() {
    for label in ["BACKUP{A}", "Backup}2026", "{\"name\":\"other\"}"] {
        let mut node = row();
        node["rm"] = json!(true);
        node["fstype"] = json!("ext4");
        node["label"] = json!(label);
        node["mountpoint"] = json!("/synthetic/BACKUP{A}");
        let devices = offerable_devices(&parse_lsblk_checked(&inventory(node)).unwrap());
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].label.as_deref(), Some(label));
        assert_eq!(
            devices[0].mount_point.as_deref(),
            Some(Path::new("/synthetic/BACKUP{A}"))
        );
        assert!(devices[0].is_mounted());
    }
}

#[test]
fn strings_decode_json_escapes_once_without_path_normalization() {
    let input = r#"{"blockdevices":[{"na\u006de":"fixture","path":"/dev/fixture","type":"part","label":"\\u0041","mountpoint":"/synthetic/a\nb\tc\\share\"/\u00e9/\uD83D\uDE00"}]}"#;
    let rows = parse_lsblk_checked(input).unwrap();
    assert_eq!(rows[0].name, "fixture");
    assert_eq!(rows[0].label.as_deref(), Some("\\u0041"));
    assert_eq!(
        rows[0].mount_point.as_deref(),
        Some("/synthetic/a\nb\tc\\share\"/é/😀")
    );
}

#[test]
fn invalid_envelopes_rows_and_children_refuse_the_whole_inventory() {
    for envelope in [
        json!(null),
        json!([]),
        json!({}),
        json!({"blockdevices":null}),
    ] {
        assert_eq!(
            parse_lsblk_checked(&envelope.to_string()).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
    for invalid in [
        json!(null),
        json!(false),
        json!("row"),
        json!({}),
        json!([]),
    ] {
        let text = json!({"blockdevices":[row(),invalid]}).to_string();
        assert_eq!(
            parse_lsblk_checked(&text).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
    for children in [
        json!(null),
        json!(true),
        json!("children"),
        json!({}),
        json!([null]),
        json!([{}]),
    ] {
        let mut node = row();
        node["children"] = children;
        assert_eq!(
            parse_lsblk_checked(&inventory(node)).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
    assert!(parse_lsblk_checked(r#"{"blockdevices":[]}"#)
        .unwrap()
        .is_empty());
}

#[test]
fn required_fields_and_present_optional_types_are_validated() {
    for field in ["name", "path", "type"] {
        for value in [json!(null), json!(""), json!(false), json!(3)] {
            let mut node = row();
            node[field] = value;
            assert_eq!(
                parse_lsblk_checked(&inventory(node)).unwrap_err(),
                LsblkParseError::InvalidShape,
                "{field}"
            );
        }
        let mut node = row();
        node.as_object_mut().unwrap().remove(field);
        assert_eq!(
            parse_lsblk_checked(&inventory(node)).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
    for (field, wrong) in [
        ("size", json!(null)),
        ("size", json!(4)),
        ("rm", json!(null)),
        ("rm", json!("true")),
        ("hotplug", json!(1)),
        ("fstype", json!(false)),
        ("label", json!({})),
        ("mountpoint", json!([])),
        ("pkname", json!(42)),
    ] {
        let mut node = row();
        node[field] = wrong;
        assert_eq!(
            parse_lsblk_checked(&inventory(node)).unwrap_err(),
            LsblkParseError::InvalidShape,
            "{field}"
        );
    }
}

#[test]
fn optional_defaults_and_unknown_fields_keep_legacy_absence_semantics() {
    let mut node = row();
    node["label"] = json!("");
    node["fstype"] = json!(null);
    node["mountpoint"] = json!("");
    node["extra"] = json!({"nested":["{label}",true,null]});
    let rows = parse_lsblk_checked(&inventory(node)).unwrap();
    assert!(!rows[0].removable && !rows[0].hotplug);
    assert_eq!(rows[0].size, "");
    assert!(rows[0].label.is_none() && rows[0].fstype.is_none());
    assert!(rows[0].mount_point.is_none() && rows[0].parent.is_none());
}

#[test]
fn nested_rows_keep_preorder_explicit_parents_and_cleartext_join() {
    let input = json!({"blockdevices":[
        {"name":"outer","path":"/dev/outer","type":"part","rm":true,
         "fstype":"crypto_LUKS","children":[
            {"name":"dm-fixture","path":"/dev/mapper/fixture","type":"crypt",
             "pkname":"outer","fstype":"ext4","mountpoint":"/synthetic/unlocked"}
         ]},
        {"name":"loop-fixture","path":"/dev/loop-fixture","type":"loop","rm":true}
    ]});
    let rows = parse_lsblk_checked(&input.to_string()).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["outer", "dm-fixture", "loop-fixture"]
    );
    let devices = offerable_devices(&rows);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].cleartext.as_deref(), Some("/dev/mapper/fixture"));
    assert_eq!(
        devices[0].mount_point.as_deref(),
        Some(Path::new("/synthetic/unlocked"))
    );
}

#[test]
fn duplicate_recognized_fields_are_refused_after_key_decoding() {
    for input in [
        r#"{"blockdevices":[],"blockdevices":[]}"#,
        r#"{"blockdevices":[{"name":"a","na\u006de":"b","path":"/dev/a","type":"disk"}]}"#,
        r#"{"blockdevices":[{"name":"a","path":"/dev/a","type":"disk","mountpoint":null,"mountpoint":"/synthetic"}]}"#,
    ] {
        assert_eq!(
            parse_lsblk_checked(input).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
}

#[test]
fn malformed_json_is_not_an_empty_success_or_an_input_bearing_error() {
    for input in [
        "",
        "{",
        r#"{"blockdevices":[]} trailing"#,
        r#"{"blockdevices":[{"name":"\uD800"}]}"#,
    ] {
        let error = parse_lsblk_checked(input).unwrap_err();
        assert_eq!(error, LsblkParseError::InvalidJson);
        assert_eq!(error.to_string(), "invalid lsblk JSON");
    }
    let error = parse_lsblk_checked(r#"{"blockdevices":["synthetic-private-value"]}"#).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("synthetic-private-value"));
}

#[test]
fn byte_and_row_ceilings_are_exact_errors_without_partial_results() {
    let mut exact = r#"{"blockdevices":[]}"#.to_string();
    exact.extend(std::iter::repeat_n(' ', MAX_LSBLK_BYTES - exact.len()));
    assert!(parse_lsblk_checked(&exact).unwrap().is_empty());
    exact.push(' ');
    assert_eq!(
        parse_lsblk_checked(&exact).unwrap_err(),
        LsblkParseError::Limit
    );
    let one = r#"{"name":"n","path":"p","type":"disk"}"#;
    let mut rows = format!(
        "{{\"blockdevices\":[{}{}]}}",
        (one.to_owned() + ",").repeat(MAX_LSBLK_ROWS - 1),
        one
    );
    assert!(rows.len() < MAX_LSBLK_BYTES);
    assert_eq!(parse_lsblk_checked(&rows).unwrap().len(), MAX_LSBLK_ROWS);
    rows.truncate(rows.len() - 2);
    rows.push(',');
    rows.push_str(one);
    rows.push_str("]}");
    assert_eq!(
        parse_lsblk_checked(&rows).unwrap_err(),
        LsblkParseError::Limit
    );
}

fn nested(levels: usize) -> String {
    let parent = r#"{"name":"n","path":"p","type":"disk","children":["#;
    format!(
        "{{\"blockdevices\":[{}{}{}]}}",
        parent.repeat(levels - 1),
        row(),
        "]}".repeat(levels - 1)
    )
}
#[test]
fn row_depth_and_parser_recursion_guards_are_explicit_limits() {
    assert_eq!(
        parse_lsblk_checked(&nested(MAX_LSBLK_DEPTH)).unwrap().len(),
        MAX_LSBLK_DEPTH
    );
    for depth in [MAX_LSBLK_DEPTH + 1, 80] {
        assert_eq!(
            parse_lsblk_checked(&nested(depth)).unwrap_err(),
            LsblkParseError::Limit
        );
    }
}

#[test]
fn compatibility_adapter_uses_the_same_correct_decoder_but_loses_errors() {
    let raw = parse_lsblk(
        r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","label":"BACKUP{A}","mountpoint":"/synthetic/a\nb"}]}"#,
    );
    assert_eq!(raw[0].label.as_deref(), Some("BACKUP{A}"));
    assert_eq!(raw[0].mount_point.as_deref(), Some("/synthetic/a\nb"));
    assert!(parse_lsblk(r#"{"blockdevices":[null]}"#).is_empty());
}
