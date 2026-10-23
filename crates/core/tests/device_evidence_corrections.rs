use gust_core::devices::*;
use serde_json::{json, Value};

fn source(kind: &str) -> Value {
    json!({"name":"fixture","kname":"fixture","path":"/dev/fixture","type":kind,
        "rm":true,"fstype":"crypto_LUKS","maj:min":"8:1","disk-seq":41})
}
fn observe(row: Value) -> Vec<DeviceObservation> {
    parse_lsblk_evidence_checked(
        &json!({"blockdevices":[row]}).to_string(),
        Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1),
    )
    .unwrap()
}
fn with_child(mut row: Value) -> Value {
    row["children"] = json!([{"name":"clear","kname":"dm-fixture","path":"/dev/mapper/clear",
        "pkname":"fixture","type":"crypt","fstype":"ext4","maj:min":"253:7","disk-seq":81}]);
    row
}

#[test]
fn positive_inherited_partition_sequence_cannot_revalidate_recreated_partition() {
    // Synthetic captures model deletion/recreation while the containing gendisk
    // survives. The provider can repeat all of these fields for a new partition.
    let old = observe(source("part")).remove(0);
    let mut recreated = source("part");
    recreated["label"] = json!("recreated");
    let current = observe(recreated);
    assert_eq!(
        revalidate_device(&old, &current),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    assert_eq!(
        current[0].evidence(),
        &DeviceEvidence::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    assert_eq!(current[0].device().label.as_deref(), Some("recreated"));
}

#[test]
fn positive_partition_sequence_cannot_bind_new_cleartext_after_unlock() {
    let old = observe(source("part")).remove(0);
    let current = observe(with_child(source("part")));
    assert_eq!(
        bind_cleartext_after_unlock(&old, &current, "/dev/mapper/clear"),
        DeviceRevalidation::Unavailable(DeviceEvidenceUnavailable::MissingSourceInstance)
    );
    assert_eq!(
        current[0].device().cleartext.as_deref(),
        Some("/dev/mapper/clear")
    );
}

#[test]
fn unchanged_whole_instance_and_changed_whole_disk_controls_remain_distinct() {
    let old = observe(source("disk")).remove(0);
    let mut unchanged = source("disk");
    unchanged["label"] = json!("fresh label");
    match revalidate_device(&old, &observe(unchanged)) {
        DeviceRevalidation::Same(fresh) => {
            assert_eq!(fresh.device().label.as_deref(), Some("fresh label"))
        }
        other => panic!("unchanged whole instance refused: {other:?}"),
    }
    let mut replaced = source("disk");
    replaced["disk-seq"] = json!(42);
    assert_eq!(
        revalidate_device(&old, &observe(replaced)),
        DeviceRevalidation::Changed
    );
}

#[test]
fn legacy_unknown_metadata_is_grammar_skipped_without_numeric_buffering() {
    for extra in [
        r#""kname":1e400"#,
        r#""unknown":1e400"#,
        r#""unknown":{"nested":[1e400,{"x":-1e400}]}"#,
        r#""unknown":1e400,"unknown":false"#,
    ] {
        let input =
            format!(r#"{{"blockdevices":[{{"name":"n","path":"p","type":"disk",{extra}}}]}}"#);
        let rows = parse_lsblk_checked(&input).expect("unknown values must not be decoded as f64");
        assert_eq!(rows.len(), 1);
        assert_eq!(parse_lsblk(&input).len(), 1);
    }
}

#[test]
fn evidence_skips_unknown_values_but_refuses_invalid_known_metadata() {
    let input = r#"{"blockdevices":[{"name":"n","path":"p","type":"disk","rm":true,"kname":"n","maj:min":"8:0","disk-seq":41,"unknown":{"nested":[1e400]}}]}"#;
    let rows = parse_lsblk_evidence_checked(input, Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1))
        .unwrap();
    assert!(matches!(rows[0].evidence(), DeviceEvidence::Available(_)));
    for extra in [
        r#""kname":1e400"#,
        r#""disk-seq":1e400"#,
        r#""unknown":1e+"#,
    ] {
        let input =
            format!(r#"{{"blockdevices":[{{"name":"n","path":"p","type":"disk",{extra}}}]}}"#);
        assert!(parse_lsblk_evidence_checked(
            &input,
            Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1)
        )
        .is_err());
    }
}
