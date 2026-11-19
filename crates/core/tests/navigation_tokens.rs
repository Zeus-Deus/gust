use gust_core::location::{crumbs, escape_for_display, ChildNameToken, LocationKey, PathToken};
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

fn crumb_text(path: &str, home: &str, place: Option<(&str, &str)>) -> Vec<(String, String)> {
    crumbs(
        Path::new(path),
        Path::new(home),
        place.map(|(root, label)| (root.into(), label.to_string())),
    )
    .into_iter()
    .map(|(p, l)| (p.to_string_lossy().into_owned(), l))
    .collect()
}

fn pair(path: &str, label: &str) -> (String, String) {
    (path.to_string(), label.to_string())
}

#[test]
fn the_path_bar_starts_at_home_inside_the_home_folder() {
    assert_eq!(
        crumb_text("/home/sam/Documents/Taxes 2026", "/home/sam", None),
        vec![
            pair("/home/sam", "Home"),
            pair("/home/sam/Documents", "Documents"),
            pair("/home/sam/Documents/Taxes 2026", "Taxes 2026"),
        ]
    );
    assert_eq!(
        crumb_text("/home/sam", "/home/sam", None),
        vec![pair("/home/sam", "Home")]
    );
}

#[test]
fn outside_home_the_path_bar_still_starts_at_the_root() {
    assert_eq!(
        crumb_text("/home", "/home/sam", None),
        vec![pair("/", "/"), pair("/home", "home")]
    );
    // A sibling whose name merely begins like the home folder is not inside it.
    assert_eq!(
        crumb_text("/home/samuel", "/home/sam", None),
        vec![
            pair("/", "/"),
            pair("/home", "home"),
            pair("/home/samuel", "samuel")
        ]
    );
    assert_eq!(crumb_text("/", "/home/sam", None), vec![pair("/", "/")]);
}

#[test]
fn an_unset_or_root_home_never_renames_the_root() {
    for home in ["", "/", "relative/home"] {
        assert_eq!(
            crumb_text("/etc", home, None),
            vec![pair("/", "/"), pair("/etc", "etc")],
            "home {home:?}"
        );
    }
}

#[test]
fn a_mounted_place_names_the_first_crumb_even_under_home() {
    assert_eq!(
        crumb_text(
            "/home/sam/mnt/nas/Shared",
            "/home/sam",
            Some(("/home/sam/mnt/nas", "nas"))
        ),
        vec![
            pair("/home/sam/mnt/nas", "nas"),
            pair("/home/sam/mnt/nas/Shared", "Shared")
        ]
    );
    // A place that does not contain the path is ignored.
    assert_eq!(
        crumb_text(
            "/home/sam/Music",
            "/home/sam",
            Some(("/run/media/sam/USB", "USB"))
        ),
        vec![pair("/home/sam", "Home"), pair("/home/sam/Music", "Music")]
    );
}

#[test]
fn crumb_labels_make_hostile_names_visible() {
    let got = crumb_text("/home/sam/a\u{202e}b", "/home/sam", None);
    assert_eq!(got[1].1, "a<U+202E>b");
}
