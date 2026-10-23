use gust_core::{drop, stall};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

fn flat_table(mount: &[u8], fstype: &str) -> Vec<u8> {
    let mut table = b"/dev/root / ext4 rw 0 0\npeer ".to_vec();
    table.extend_from_slice(mount);
    table.extend_from_slice(format!(" {fstype} rw 0 0\n").as_bytes());
    table
}

fn row(id: u64, parent: u64, mount: &[u8], fstype: &str) -> Vec<u8> {
    let mut row = format!("{id} {parent} 0:1 / ").into_bytes();
    row.extend_from_slice(mount);
    row.extend_from_slice(format!(" rw - {fstype} source rw\n").as_bytes());
    row
}

fn table(mount: &[u8], fstype: &str) -> Vec<u8> {
    let mut table = row(1, 0, b"/", "ext4");
    table.extend_from_slice(&row(2, 1, mount, fstype));
    table
}

#[test]
fn escaped_remote_mounts_win_over_the_local_root() {
    for fstype in ["fuse.custom-peer", "fuse.sshfs", "nfs4", "cifs"] {
        for (encoded, raw) in [
            (
                b"/mnt/name\\040peer".as_slice(),
                b"/mnt/name peer".as_slice(),
            ),
            (b"/mnt/name\\134peer", b"/mnt/name\\peer"),
            (b"/mnt/name\\011peer", b"/mnt/name\tpeer"),
            (b"/mnt/name\\012peer", b"/mnt/name\npeer"),
        ] {
            let source = Path::new(OsStr::from_bytes(raw)).join("source.txt");
            let found = stall::fstype_from_mountinfo(&source, &table(encoded, fstype)).unwrap();
            assert_eq!(found, fstype, "{encoded:?}");
            // Preserve the exact subtype for callers with an all-FUSE policy.
            assert!(found.starts_with("fuse") || stall::can_stall(&found));
            let mounts =
                drop::mount_points(std::str::from_utf8(&flat_table(encoded, fstype)).unwrap());
            assert_eq!(drop::mount_of(&mounts, &source), source.parent());
        }
    }
}

#[test]
fn literal_escape_sequences_do_not_alias_decoded_space_or_control_names() {
    for (literal, decoded, encoded_literal, encoded_decoded) in [
        (
            b"/mnt/a\\040b".as_slice(),
            b"/mnt/a b".as_slice(),
            b"/mnt/a\\134040b".as_slice(),
            b"/mnt/a\\040b".as_slice(),
        ),
        (
            b"/mnt/a\\011b",
            b"/mnt/a\tb",
            b"/mnt/a\\134011b",
            b"/mnt/a\\011b",
        ),
        (
            b"/mnt/a\\012b",
            b"/mnt/a\nb",
            b"/mnt/a\\134012b",
            b"/mnt/a\\012b",
        ),
        (
            b"/mnt/a\\134b",
            b"/mnt/a\\b",
            b"/mnt/a\\134134b",
            b"/mnt/a\\134b",
        ),
    ] {
        let mut mounts = table(encoded_literal, "fuse.custom-peer");
        mounts.extend_from_slice(&row(3, 1, encoded_decoded, "btrfs"));
        let literal = Path::new(OsStr::from_bytes(literal)).join("source.txt");
        let decoded = Path::new(OsStr::from_bytes(decoded)).join("source.txt");
        assert_eq!(
            stall::fstype_from_mountinfo(&literal, &mounts).as_deref(),
            Some("fuse.custom-peer")
        );
        assert_eq!(
            stall::fstype_from_mountinfo(&decoded, &mounts).as_deref(),
            Some("btrfs")
        );
    }
}

#[test]
fn non_utf8_and_non_separator_whitespace_remain_mount_path_bytes() {
    for raw in [
        b"/mnt/peer\xff".as_slice(),
        "/mnt/peer\u{a0}name".as_bytes(),
        b"/mnt/peer\rname",
        b"/mnt/peer\x0bname",
        b"/mnt/peer\x0cname",
    ] {
        let mounts = table(raw, "nfs4");
        let source = Path::new(OsStr::from_bytes(raw)).join("source.txt");
        assert_eq!(
            stall::fstype_from_mountinfo(&source, &mounts).as_deref(),
            Some("nfs4"),
            "{raw:?}"
        );
    }
}

#[test]
fn matching_uses_the_longest_component_prefix_and_never_opens_candidates() {
    let mut mounts = table(b"/mnt/shared", "nfs4");
    mounts.extend_from_slice(&row(3, 2, b"/mnt/shared/local", "btrfs"));
    for (path, fstype) in [
        ("/mnt/shared/source.txt", "nfs4"),
        ("/mnt/shared/local/source.txt", "btrfs"),
        ("/mnt/shared-sibling/source.txt", "ext4"),
        ("/mnt/share/source.txt", "ext4"),
        ("/mnt/shared/local-sibling/source.txt", "nfs4"),
    ] {
        assert_eq!(
            stall::fstype_from_mountinfo(Path::new(path), &mounts).as_deref(),
            Some(fstype)
        );
    }
}
