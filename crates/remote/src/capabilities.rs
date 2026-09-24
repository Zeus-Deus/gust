//! Backend capability probing.
//!
//! Capabilities come from the connected account (rclone `operations/fsinfo`),
//! never from a provider's marketing name. Anything the backend does not
//! report stays `Unknown` (plan §9).

use gust_core::location::{Cap, Capabilities};

/// Map rclone's `Features` object onto Gust capabilities.
///
/// Absent keys are `Unknown`, not `No` — the distinction matters because the
/// UI attempts Unknown operations and reports honestly, while it disables
/// definite `No`.
pub fn probe_from_fsinfo(fsinfo: &serde_json::Value) -> Capabilities {
    let mut c = Capabilities::unknown();
    let f = &fsinfo["Features"];

    let tri = |v: &serde_json::Value| -> Cap {
        match v.as_bool() {
            Some(true) => Cap::Yes,
            Some(false) => Cap::No,
            None => Cap::Unknown,
        }
    };

    c.read = Cap::Yes; // if we can query fsinfo we can list
    c.write = match f["PutStream"].as_bool().or(f["Put"].as_bool()) {
        Some(v) => {
            if v {
                Cap::Yes
            } else {
                Cap::No
            }
        }
        None => Cap::Unknown,
    };
    c.atomic_rename = tri(&f["Move"]);
    c.server_side_copy = tri(&f["Copy"]);
    c.trash = match f["CleanUp"].as_bool() {
        // CleanUp is emptying the trash, which implies a trash concept.
        Some(true) => Cap::Unknown, // still not proof of restore support
        Some(false) => Cap::No,
        None => Cap::Unknown,
    };
    c.case_sensitive = match f["CaseInsensitive"].as_bool() {
        Some(true) => Cap::No,
        Some(false) => Cap::Yes,
        None => Cap::Unknown,
    };
    // Object stores do not have Unix permissions; absence of the key means we
    // simply do not know.
    c.unix_permissions = Cap::Unknown;
    c.hardlinks = Cap::No;
    c.symlinks = tri(&f["Links"]);
    c
}

/// Should the UI offer this action?
pub fn action_enabled(cap: Cap) -> bool {
    !cap.is_no()
}

/// Should the UI warn before attempting it?
pub fn action_uncertain(cap: Cap) -> bool {
    cap.is_unknown()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_features_stay_unknown_not_no() {
        let c = probe_from_fsinfo(&json!({"Features": {}}));
        assert!(c.atomic_rename.is_unknown());
        assert!(c.trash.is_unknown());
        assert!(!c.atomic_rename.is_no(), "Unknown must not collapse to No");
    }

    #[test]
    fn explicit_false_becomes_no() {
        let c = probe_from_fsinfo(&json!({"Features": {"Move": false, "Copy": false}}));
        assert!(c.atomic_rename.is_no());
        assert!(c.server_side_copy.is_no());
        assert!(!action_enabled(c.atomic_rename));
    }

    #[test]
    fn explicit_true_becomes_yes() {
        let c = probe_from_fsinfo(&json!({"Features": {"Move": true, "Copy": true, "Put": true}}));
        assert!(c.atomic_rename.is_yes());
        assert!(c.write.is_yes());
        assert!(action_enabled(c.atomic_rename));
        assert!(!action_uncertain(c.atomic_rename));
    }

    #[test]
    fn case_insensitive_backend_reports_case_sensitivity_false() {
        let c = probe_from_fsinfo(&json!({"Features": {"CaseInsensitive": true}}));
        assert!(c.case_sensitive.is_no());
    }

    #[test]
    fn cleanup_support_does_not_promise_restorable_trash() {
        let c = probe_from_fsinfo(&json!({"Features": {"CleanUp": true}}));
        assert!(
            c.trash.is_unknown(),
            "emptying a trash is not the same as restoring from it"
        );
    }

    #[test]
    fn unknown_actions_are_offered_but_flagged() {
        assert!(action_enabled(Cap::Unknown));
        assert!(action_uncertain(Cap::Unknown));
        assert!(!action_uncertain(Cap::Yes));
    }
}
