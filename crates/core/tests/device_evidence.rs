use gust_core::devices::*;
use serde_json::{json, Value};
use std::path::Path;

fn source() -> Value {
    json!({"name":"fixture1","kname":"fixture1","path":"/dev/fixture1","type":"disk",
        "rm":true,"fstype":"ext4","maj:min":"8:1","disk-seq":41})
}
fn crypt() -> Value {
    json!({"name":"clear","kname":"dm-fixture","path":"/dev/mapper/clear","type":"crypt",
        "pkname":"fixture1","fstype":"ext4","maj:min":"253:7","disk-seq":81})
}
fn encrypted(child: bool) -> Value {
    let mut node = source();
    node["fstype"] = json!("crypto_LUKS");
    if child {
        node["children"] = json!([crypt()]);
    }
    node
}
fn text(rows: Vec<Value>) -> String {
    json!({"blockdevices":rows}).to_string()
}
fn observe(rows: Vec<Value>) -> Vec<DeviceObservation> {
    parse_lsblk_evidence_checked(&text(rows), Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1))
        .unwrap()
}
fn available(row: &DeviceObservation) -> &DeviceBinding {
    match row.evidence() {
        DeviceEvidence::Available(binding) => binding,
        other => panic!("expected Available, got {other:?}"),
    }
}
fn unavailable(row: &DeviceObservation, reason: DeviceEvidenceUnavailable) {
    assert_eq!(row.evidence(), &DeviceEvidence::Unavailable(reason));
}
fn same(result: DeviceRevalidation) -> DeviceObservation {
    match result {
        DeviceRevalidation::Same(row) => row,
        other => panic!("expected Same, got {other:?}"),
    }
}

#[test]
fn supported_profile_retains_typed_source_child_and_explicit_parent() {
    let rows = observe(vec![encrypted(true)]);
    let row = &rows[0];
    let binding = available(row);
    assert_eq!(binding.origin(), DeviceEvidenceOrigin::LsblkKernelSysfsV1);
    assert_eq!(
        binding.source().number(),
        DeviceNumber { major: 8, minor: 1 }
    );
    assert_eq!(binding.source().disk_sequence().get(), 41);
    let child = binding.cleartext().unwrap();
    assert_eq!(
        child.instance().number(),
        DeviceNumber {
            major: 253,
            minor: 7
        }
    );
    assert_eq!(child.instance().disk_sequence().get(), 81);
    assert_eq!(child.parent(), binding.source());
    assert_eq!(row.device().mount_target(), "/dev/mapper/clear");
}

#[test]
fn missing_origin_cannot_be_asserted_by_json_or_display_values() {
    let mut node = source();
    node["origin"] = json!("LsblkKernelSysfsV1");
    let rows = parse_lsblk_evidence_checked(&text(vec![node]), None).unwrap();
    assert_eq!(rows.len(), 1);
    unavailable(&rows[0], DeviceEvidenceUnavailable::UnsupportedOrigin);
}

#[test]
fn missing_zero_or_null_evidence_preserves_unavailable_display_rows() {
    for field in ["kname", "maj:min", "disk-seq"] {
        for absent in [true, false] {
            let mut node = source();
            if absent {
                node.as_object_mut().unwrap().remove(field);
            } else {
                node[field] = Value::Null;
            }
            let rows = observe(vec![node]);
            assert_eq!(rows.len(), 1);
            unavailable(&rows[0], DeviceEvidenceUnavailable::MissingSourceInstance);
        }
    }
    for (field, value) in [("kname", json!("")), ("disk-seq", json!(0))] {
        let mut node = source();
        node[field] = value;
        unavailable(
            &observe(vec![node])[0],
            DeviceEvidenceUnavailable::MissingSourceInstance,
        );
    }
    let mut parent = source();
    parent["name"] = json!("disk");
    parent["kname"] = json!("disk");
    parent["maj:min"] = json!("8:0");
    parent["rm"] = json!(false);
    let mut partition = source();
    partition["disk-seq"] = Value::Null;
    parent["children"] = json!([partition]);
    unavailable(
        &observe(vec![parent])[0],
        DeviceEvidenceUnavailable::MissingSourceInstance,
    );
}

#[test]
fn evidence_types_numbers_and_every_unofferable_row_are_validated() {
    for (field, value) in [
        ("kname", json!(4)),
        ("maj:min", json!(4)),
        ("maj:min", json!("08:1")),
        ("maj:min", json!("8:01")),
        ("maj:min", json!("8:+1")),
        ("maj:min", json!("8:1:2")),
        ("maj:min", json!("8:4294967296")),
        ("maj:min", json!(" 8:1")),
        ("maj:min", json!("")),
        ("disk-seq", json!("41")),
        ("disk-seq", json!(-1)),
        ("disk-seq", json!(1.0)),
        ("disk-seq", json!(true)),
        ("disk-seq", json!([])),
    ] {
        let mut invalid = source();
        invalid[field] = value;
        invalid["rm"] = json!(false);
        let input = text(vec![source(), invalid]);
        assert_eq!(
            parse_lsblk_evidence_checked(&input, Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1))
                .unwrap_err(),
            LsblkParseError::InvalidShape,
            "{field}"
        );
        // Compatibility: these fields were unknown to the old checked API.
        assert_eq!(parse_lsblk_checked(&input).unwrap().len(), 2);
    }
    let mut node = source();
    node["disk-seq"] = json!(u64::MAX);
    assert_eq!(
        available(&observe(vec![node])[0])
            .source()
            .disk_sequence()
            .get(),
        u64::MAX
    );
}

#[test]
fn decoded_duplicate_evidence_keys_and_positional_nodes_are_refused() {
    for input in [
        r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","kname":null,"kn\u0061me":"n"}]}"#,
        r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","maj:min":null,"maj\u003amin":"8:1"}]}"#,
        r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","disk-seq":null,"disk-seq":1}]}"#,
        "[[]]",
        r#"{"blockdevices":[["n","p","disk","4G",true,false,"ext4",null,null,null,[]]]}"#,
        r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","children":[null]}]}"#,
    ] {
        assert_eq!(
            parse_lsblk_evidence_checked(input, Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1))
                .unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
}

#[test]
fn duplicated_kernel_numbers_names_or_locators_fence_only_affected_bindings() {
    for field in ["maj:min", "kname", "path"] {
        let mut duplicate = source();
        duplicate["kname"] = json!("other");
        duplicate["name"] = json!("other");
        duplicate["maj:min"] = json!("8:2");
        duplicate["path"] = json!("/dev/other");
        duplicate[field] = source()[field].clone();
        let mut safe = source();
        safe["name"] = json!("safe");
        safe["kname"] = json!("safe");
        safe["path"] = json!("/dev/safe");
        safe["maj:min"] = json!("8:3");
        for rows in [
            vec![source(), duplicate.clone(), safe.clone()],
            vec![duplicate, source(), safe],
        ] {
            let rows = observe(rows);
            unavailable(&rows[0], DeviceEvidenceUnavailable::AmbiguousInstance);
            unavailable(&rows[1], DeviceEvidenceUnavailable::AmbiguousInstance);
            available(&rows[2]);
        }
    }
}

#[test]
fn cleartext_requires_one_real_tree_edge_and_matching_unique_pkname() {
    let mut flat_child = crypt();
    flat_child["hotplug"] = json!(false);
    let mut wrong_parent = encrypted(true);
    wrong_parent["children"][0]["pkname"] = json!("other");
    let mut missing_parent = encrypted(true);
    missing_parent["children"][0]
        .as_object_mut()
        .unwrap()
        .remove("pkname");
    let mut two = encrypted(true);
    let mut second = crypt();
    second["maj:min"] = json!("253:8");
    second["kname"] = json!("dm-other");
    second["path"] = json!("/dev/mapper/other");
    two["children"].as_array_mut().unwrap().push(second);
    for rows in [
        vec![encrypted(false), flat_child],
        vec![wrong_parent],
        vec![missing_parent],
        vec![two],
    ] {
        unavailable(
            &observe(rows)[0],
            DeviceEvidenceUnavailable::AmbiguousRelationship,
        );
    }
}

#[test]
fn cleartext_missing_or_duplicate_kernel_evidence_never_becomes_available() {
    for field in ["kname", "maj:min", "disk-seq"] {
        let mut node = encrypted(true);
        node["children"][0][field] = Value::Null;
        unavailable(
            &observe(vec![node])[0],
            DeviceEvidenceUnavailable::MissingCleartextInstance,
        );
    }
    let mut node = encrypted(true);
    node["children"][0]["disk-seq"] = json!(0);
    unavailable(
        &observe(vec![node])[0],
        DeviceEvidenceUnavailable::MissingCleartextInstance,
    );
    let mut duplicate = crypt();
    duplicate["pkname"] = Value::Null;
    unavailable(
        &observe(vec![encrypted(true), duplicate])[0],
        DeviceEvidenceUnavailable::AmbiguousInstance,
    );
}

#[test]
fn path_reuse_or_child_instance_changes_refuse_retained_selection() {
    let old = observe(vec![encrypted(true)]).remove(0);
    for which in ["source", "child"] {
        let mut node = encrypted(true);
        if which == "source" {
            node["disk-seq"] = json!(42);
        } else {
            node["children"][0]["disk-seq"] = json!(82);
        }
        assert_eq!(
            revalidate_device(&old, &observe(vec![node])),
            DeviceRevalidation::Changed
        );
    }
    assert_eq!(
        revalidate_device(&old, &observe(vec![encrypted(false)])),
        DeviceRevalidation::Changed
    );
    let mut node = encrypted(true);
    node["disk-seq"] = Value::Null;
    assert_eq!(
        revalidate_device(&old, &observe(vec![node])),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    assert_eq!(
        revalidate_device(&old, &[]),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
}

#[test]
fn same_returns_fresh_locators_and_preserves_exact_mount_strings() {
    let old = observe(vec![encrypted(true)]).remove(0);
    let mut node = encrypted(true);
    node["path"] = json!("/dev/new-source");
    node["label"] = json!("BACKUP{A}");
    node["children"][0]["path"] = json!("/dev/mapper/new-clear");
    node["children"][0]["mountpoint"] = json!("/synthetic/a\nb\tc\\share\"/é/😀");
    let fresh = same(revalidate_device(&old, &observe(vec![node])));
    assert_eq!(fresh.device().path, "/dev/new-source");
    assert_eq!(fresh.device().mount_target(), "/dev/mapper/new-clear");
    assert_eq!(
        fresh.device().mount_point.as_deref(),
        Some(Path::new("/synthetic/a\nb\tc\\share\"/é/😀"))
    );
    assert_eq!(available(&old), available(&fresh));
}

#[test]
fn ordinary_revalidation_never_adopts_a_new_cleartext_binding() {
    let old = observe(vec![encrypted(false)]).remove(0);
    assert_eq!(
        revalidate_device(&old, &observe(vec![encrypted(true)])),
        DeviceRevalidation::Changed
    );
    let plain = observe(vec![source()]).remove(0);
    assert_eq!(
        revalidate_device(&plain, &observe(vec![encrypted(false)])),
        DeviceRevalidation::Changed
    );
}

#[test]
fn successful_own_unlock_transition_requires_retained_source_and_exact_child_locator() {
    let old = observe(vec![encrypted(false)]).remove(0);
    let current = observe(vec![encrypted(true)]);
    let fresh = same(bind_cleartext_after_unlock(
        &old,
        &current,
        "/dev/mapper/clear",
    ));
    assert!(available(&fresh).cleartext().is_some());
    same(revalidate_device(&fresh, &current));
    for locator in [
        "",
        "/dev/dm-fixture",
        "/dev/mapper/other",
        "/dev/mapper/clear/",
    ] {
        assert_eq!(
            bind_cleartext_after_unlock(&old, &current, locator),
            DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::UnverifiedLocator)
        );
    }
    let mut replaced = encrypted(true);
    replaced["disk-seq"] = json!(42);
    assert_eq!(
        bind_cleartext_after_unlock(&old, &observe(vec![replaced]), "/dev/mapper/clear"),
        DeviceRevalidation::Changed
    );
    assert_eq!(
        bind_cleartext_after_unlock(&fresh, &current, "/dev/mapper/clear"),
        DeviceRevalidation::Changed
    );
    assert_eq!(
        bind_cleartext_after_unlock(&old, &observe(vec![encrypted(false)]), "/dev/mapper/clear"),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingCleartextInstance)
    );
    let plain = observe(vec![source()]).remove(0);
    assert_eq!(
        bind_cleartext_after_unlock(&plain, &current, "/dev/mapper/clear"),
        DeviceRevalidation::Changed
    );
}

#[test]
fn unavailable_capture_or_child_cannot_be_upgraded_by_comparison_or_unlock() {
    let supported = observe(vec![encrypted(false)]).remove(0);
    let unsupported = parse_lsblk_evidence_checked(&text(vec![encrypted(true)]), None).unwrap();
    assert_eq!(
        revalidate_device(&supported, &unsupported),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::UnsupportedOrigin)
    );
    assert_eq!(
        bind_cleartext_after_unlock(&supported, &unsupported, "/dev/mapper/clear"),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::UnsupportedOrigin)
    );
    assert_eq!(
        revalidate_device(&unsupported[0], &observe(vec![encrypted(true)])),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::UnsupportedOrigin)
    );
}

#[test]
fn evidence_parser_retains_shared_byte_depth_and_row_limits() {
    let origin = Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1);
    let mut input = text(vec![source()]);
    input.extend(std::iter::repeat_n(' ', MAX_LSBLK_BYTES - input.len()));
    assert_eq!(
        parse_lsblk_evidence_checked(&input, origin).unwrap().len(),
        1
    );
    input.push(' ');
    assert_eq!(
        parse_lsblk_evidence_checked(&input, origin).unwrap_err(),
        LsblkParseError::Limit
    );
    let mut node = source();
    for _ in 0..MAX_LSBLK_DEPTH {
        let mut parent = source();
        parent["children"] = json!([node]);
        node = parent;
    }
    assert_eq!(
        parse_lsblk_evidence_checked(&text(vec![node]), origin).unwrap_err(),
        LsblkParseError::Limit
    );
    let row = r#"{"name":"n","path":"p","type":"disk"}"#;
    let input = format!(
        "{{\"blockdevices\":[{}{}]}}",
        (row.to_owned() + ",").repeat(MAX_LSBLK_ROWS),
        row
    );
    assert!(input.len() < MAX_LSBLK_BYTES);
    assert_eq!(
        parse_lsblk_evidence_checked(&input, origin).unwrap_err(),
        LsblkParseError::Limit
    );
}

#[test]
fn nested_partition_remains_unavailable_after_inventory_reordering() {
    let mut disk = source();
    disk["name"] = json!("disk");
    disk["kname"] = json!("disk");
    disk["path"] = json!("/dev/disk");
    disk["type"] = json!("disk");
    disk["maj:min"] = json!("8:0");
    disk["rm"] = json!(false);
    let mut partition = encrypted(true);
    partition["type"] = json!("part");
    partition["pkname"] = json!("disk");
    disk["children"] = json!([partition]);
    let old = observe(vec![disk.clone()]).remove(0);
    let mut unrelated = source();
    unrelated["name"] = json!("other");
    unrelated["kname"] = json!("other");
    unrelated["path"] = json!("/dev/other");
    unrelated["maj:min"] = json!("8:2");
    let current = observe(vec![unrelated, disk]);
    assert_eq!(
        revalidate_device(&old, &current),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    unavailable(
        &current[1],
        DeviceEvidenceUnavailable::MissingSourceInstance,
    );
    available(&current[0]);
    assert_eq!(current[1].device().mount_target(), "/dev/mapper/clear");
}

#[test]
fn number_changes_and_duplicate_fresh_selections_never_use_old_path() {
    let old = observe(vec![encrypted(true)]).remove(0);
    let mut new_child = encrypted(true);
    new_child["children"][0]["maj:min"] = json!("253:8");
    assert_eq!(
        revalidate_device(&old, &observe(vec![new_child])),
        DeviceRevalidation::Changed
    );
    let mut new_source = encrypted(true);
    new_source["maj:min"] = json!("8:2");
    assert_eq!(
        revalidate_device(&old, &observe(vec![new_source])),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    assert_eq!(
        revalidate_device(&old, &observe(vec![encrypted(true), encrypted(true)])),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::AmbiguousInstance)
    );
}
