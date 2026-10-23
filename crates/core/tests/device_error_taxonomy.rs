use gust_core::devices::*;

fn inventory(field: &str, raw: &str) -> String {
    format!(
        r#"{{"blockdevices":[{{"name":"fixture","path":"/dev/fixture","type":"disk","rm":true,"{field}":{raw}}}]}}"#
    )
}
fn evidence(input: &str) -> Result<Vec<DeviceObservation>, LsblkParseError> {
    parse_lsblk_evidence_checked(input, Some(DeviceEvidenceOrigin::LsblkKernelSysfsV1))
}

#[test]
fn recognized_numeric_representation_overflow_is_invalid_shape() {
    let huge_integer = "1".to_owned() + &"0".repeat(400);
    for field in ["disk-seq", "kname", "maj:min"] {
        for raw in ["1e400", "-1e400", huge_integer.as_str()] {
            let error = evidence(&inventory(field, raw)).unwrap_err();
            assert_eq!(error, LsblkParseError::InvalidShape, "{field}: {raw}");
            assert_eq!(error.to_string(), "invalid lsblk device structure");
        }
    }
}

#[test]
fn ordinary_wrong_types_large_integer_and_finite_exponents_are_invalid_shape() {
    for raw in [
        "-1",
        "1.0",
        "1e0",
        "1e308",
        "0e400",
        "18446744073709551616",
        r#""41""#,
        "true",
        "[]",
        "{}",
    ] {
        assert_eq!(
            evidence(&inventory("disk-seq", raw)).unwrap_err(),
            LsblkParseError::InvalidShape,
            "{raw}"
        );
    }
    // A wrong known type may be reported before a later grammar fault; this
    // decoder does not run a separate whole-input syntax pass after refusal.
    assert_eq!(
        evidence(&inventory("disk-seq", "[")).unwrap_err(),
        LsblkParseError::InvalidShape
    );
    for field in ["kname", "maj:min"] {
        assert_eq!(
            evidence(&inventory(field, "41")).unwrap_err(),
            LsblkParseError::InvalidShape
        );
    }
}

#[test]
fn malformed_numeric_and_json_grammar_remain_invalid_json() {
    for raw in ["1e+", "1e", "1.e2", "01", "-", r#""unterminated"#] {
        assert_eq!(
            evidence(&inventory("disk-seq", raw)).unwrap_err(),
            LsblkParseError::InvalidJson,
            "{raw}"
        );
    }
    for text in ["", "{", r#"{"blockdevices":[]} trailing"#] {
        assert_eq!(evidence(text).unwrap_err(), LsblkParseError::InvalidJson);
    }
}

#[test]
fn unknown_out_of_range_numbers_still_skip_without_numeric_representation() {
    for raw in ["1e400", "-1e400", r#"{"nested":[1e400,-1e400]}"#] {
        assert_eq!(evidence(&inventory("unknown", raw)).unwrap().len(), 1);
        assert_eq!(
            parse_lsblk_checked(&inventory("unknown", raw))
                .unwrap()
                .len(),
            1
        );
        // These are unknown only to the compatible legacy projection.
        assert_eq!(
            parse_lsblk_checked(&inventory("kname", raw)).unwrap().len(),
            1
        );
    }
}
