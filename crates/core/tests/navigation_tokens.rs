use gust_core::location::{escape_for_display, ChildNameToken, LocationKey, PathToken};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

#[test]
fn colliding_display_labels_keep_distinct_activation_tokens() {
    let parent = PathToken::from_path(Path::new("/fixture")).unwrap();
    let pairs = [
        (
            OsString::from("two\nlines"),
            OsString::from("two<U+000A>lines"),
        ),
        (OsString::from("a\u{202e}b"), OsString::from("a<U+202E>b")),
        (
            OsString::from_vec(b"bad\xff".to_vec()),
            OsString::from("bad\u{fffd}"),
        ),
    ];
    for (a, b) in pairs {
        assert_eq!(escape_for_display(&a), escape_for_display(&b));
        let a_token = ChildNameToken::from_name(&a).unwrap();
        let b_token = ChildNameToken::from_name(&b).unwrap();
        assert_ne!(a_token.to_token(), b_token.to_token());
        for token in [a_token, b_token] {
            let parsed = ChildNameToken::parse(&token.to_token()).unwrap();
            assert_eq!(parsed.name(), token.name());
            let path = parsed.resolve(&parent);
            assert_eq!(path.as_path(), parent.as_path().join(token.name()));
            assert_eq!(PathToken::parse(&path.to_token()).unwrap(), path);
            assert_eq!(
                LocationKey::from_token(&path.to_token()).unwrap(),
                *path.location()
            );
        }
    }
}

#[test]
fn navigation_refuses_malformed_or_non_child_tokens() {
    for name in ["", ".", "..", "a/b", "/a", "a\0b"] {
        assert!(ChildNameToken::from_name(OsStr::new(name)).is_none());
    }
    for raw in [
        "",
        "name:0",
        "name:zz",
        "name:2f",
        "name:00",
        "name:2e2e",
        "local:61",
    ] {
        assert!(ChildNameToken::parse(raw).is_none(), "{raw}");
    }
    for raw in [
        "local:",
        "local:61",
        "local:2f00",
        "sftp:2f61",
        "local:2fzz",
    ] {
        assert!(PathToken::parse(raw).is_none(), "{raw}");
    }
    assert!(PathToken::from_path(Path::new("relative")).is_none());
}
