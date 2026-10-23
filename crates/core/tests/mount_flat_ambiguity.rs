use gust_core::stall::fstype_from_mounts;
use std::path::Path;

#[test]
fn flat_stacks_and_their_apparently_local_descendants_are_unknown() {
    for fs in ["fuse.custom-peer", "nfs4", "cifs", "ext4"] {
        for reverse in [false, true] {
            for descendant in [false, true] {
                let local = "local /synthetic/share ext4 rw 0 0\n";
                let remote = format!("peer /synthetic/share {fs} rw 0 0\n");
                let mut table = "/dev/root / ext4 rw 0 0\n".to_owned();
                for row in if reverse {
                    [&remote, local]
                } else {
                    [local, &remote]
                } {
                    table.push_str(row);
                }
                if descendant {
                    table.push_str("local /synthetic/share/deep ext4 rw 0 0\n");
                }
                assert_eq!(
                    fstype_from_mounts(Path::new("/synthetic/share/deep/file"), table.as_bytes()),
                    None,
                    "fs={fs}, reverse={reverse}, descendant={descendant}"
                );
                assert_eq!(
                    fstype_from_mounts(
                        Path::new("/synthetic/share-sibling/file"),
                        table.as_bytes()
                    )
                    .as_deref(),
                    Some("ext4")
                );
            }
        }
    }
}

#[test]
fn flat_conflicting_ancestors_cannot_prove_a_local_child_visible() {
    let table = b"root / ext4 rw 0 0\npeer /synthetic/share fuse.custom-peer rw 0 0\nlocal /synthetic/share/deep ext4 rw 0 0\n";
    assert_eq!(
        fstype_from_mounts(Path::new("/synthetic/share/deep/file"), table),
        None
    );
    assert_eq!(
        fstype_from_mounts(Path::new("/synthetic/other/file"), table).as_deref(),
        Some("ext4")
    );
}
