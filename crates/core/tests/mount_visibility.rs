use gust_core::mount_table::{
    MountInfo, MountTableError, MAX_MOUNTINFO_BYTES, MAX_MOUNTINFO_RECORDS,
};
use gust_core::stall::fstype_from_mountinfo;
use std::path::Path;

fn row(id: u64, parent: u64, path: &str, fs: &str) -> String {
    format!("{id} {parent} 0:1 / {path} rw shared:1 - {fs} source rw\n")
}

fn table(rows: &[String], reverse: bool) -> Vec<u8> {
    let mut rows = rows.to_vec();
    if reverse {
        rows.reverse();
    }
    rows.concat().into_bytes()
}

#[test]
fn unique_overmount_leaf_wins_independently_of_row_order() {
    for fs in ["fuse.custom-peer", "nfs4", "cifs"] {
        for reverse in [false, true] {
            let bytes = table(
                &[
                    row(1, 0, "/", "ext4"),
                    row(2, 1, "/synthetic/share", "ext4"),
                    row(3, 2, "/synthetic/share", fs),
                    row(4, 2, "/synthetic/share/deep", "ext4"),
                ],
                reverse,
            );
            let mounts = MountInfo::parse(&bytes).unwrap();
            assert_eq!(
                mounts
                    .resolve(Path::new("/synthetic/share/deep/file"))
                    .unwrap()
                    .id,
                3
            );
            assert_eq!(
                fstype_from_mountinfo(Path::new("/synthetic/share/deep/file"), &bytes).as_deref(),
                Some(fs)
            );
            let ids: Vec<_> = mounts
                .visible_mounts()
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect();
            assert_eq!(ids, [1, 3]);
            assert_eq!(
                mounts
                    .resolve(Path::new("/synthetic/share-sibling/file"))
                    .unwrap()
                    .id,
                1
            );
        }
    }
}

#[test]
fn local_leaf_over_autofs_is_available_in_resolution_and_inventory() {
    for reverse in [false, true] {
        let mounts = MountInfo::parse(&table(
            &[
                row(1, 0, "/", "ext4"),
                row(2, 1, "/synthetic/home", "autofs"),
                row(3, 2, "/synthetic/home", "btrfs"),
                row(4, 3, "/synthetic/home/nested", "tmpfs"),
            ],
            reverse,
        ))
        .unwrap();
        let home = mounts.resolve(Path::new("/synthetic/home/file")).unwrap();
        assert_eq!(
            (home.id, home.parent_id, home.fs_type.as_str()),
            (3, 2, "btrfs")
        );
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/home/nested/file"))
                .unwrap()
                .id,
            4
        );
        assert_eq!(
            mounts
                .visible_mounts()
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [1, 3, 4]
        );
    }
}

#[test]
fn covering_mount_hides_old_descendants_without_an_equal_path_record() {
    for reverse in [false, true] {
        let mounts = MountInfo::parse(&table(
            &[
                row(1, 0, "/", "ext4"),
                row(2, 1, "/synthetic/share/deep", "ext4"),
                row(3, 1, "/synthetic/share", "fuse.custom-peer"),
                row(4, 2, "/synthetic/share/deep/inner", "ext4"),
                row(5, 1, "/synthetic/share-sibling", "btrfs"),
                row(6, 3, "/synthetic/share/visible", "tmpfs"),
            ],
            reverse,
        ))
        .unwrap();
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/share/deep/inner/file"))
                .unwrap()
                .id,
            3
        );
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/share/visible/file"))
                .unwrap()
                .id,
            6
        );
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/share-sibling/file"))
                .unwrap()
                .id,
            5
        );
        assert_eq!(
            mounts
                .visible_mounts()
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [1, 3, 6, 5]
        );
    }
}

#[test]
fn root_overmount_hides_old_root_children() {
    let mounts = MountInfo::parse(&table(
        &[
            row(1, 1, "/", "ext4"),
            row(2, 1, "/old", "btrfs"),
            row(3, 1, "/", "tmpfs"),
            row(4, 3, "/new", "btrfs"),
        ],
        true,
    ))
    .unwrap();
    assert_eq!(mounts.resolve(Path::new("/old/file")).unwrap().id, 3);
    assert_eq!(
        mounts
            .visible_mounts()
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        [3, 4]
    );
}

#[test]
fn unresolved_sibling_stacks_fence_only_their_subtree_but_refuse_inventory() {
    for reverse in [false, true] {
        let mounts = MountInfo::parse(&table(
            &[
                row(1, 0, "/", "ext4"),
                row(2, 1, "/synthetic/share", "ext4"),
                row(3, 1, "/synthetic/share", "fuse.custom-peer"),
                row(4, 2, "/synthetic/share/deep", "btrfs"),
                row(5, 1, "/synthetic/local", "btrfs"),
            ],
            reverse,
        ))
        .unwrap();
        for path in ["/synthetic/share", "/synthetic/share/deep/file"] {
            assert_eq!(
                mounts.resolve(Path::new(path)),
                Err(MountTableError::Ambiguous)
            );
        }
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/local/file"))
                .unwrap()
                .id,
            5
        );
        assert_eq!(
            mounts
                .resolve(Path::new("/synthetic/share-sibling/file"))
                .unwrap()
                .id,
            1
        );
        assert_eq!(mounts.visible_mounts(), Err(MountTableError::Ambiguous));
    }
}

#[test]
fn branching_overmount_chain_is_ambiguous_and_covered_ambiguity_is_hidden() {
    let rows = [
        row(1, 0, "/", "ext4"),
        row(2, 1, "/share", "ext4"),
        row(3, 2, "/share", "btrfs"),
        row(4, 2, "/share", "cifs"),
    ];
    let mounts = MountInfo::parse(&table(&rows, false)).unwrap();
    assert_eq!(
        mounts.resolve(Path::new("/share/file")),
        Err(MountTableError::Ambiguous)
    );
    let mut covered = rows.to_vec();
    covered.push(row(5, 1, "/", "tmpfs"));
    let mounts = MountInfo::parse(&table(&covered, true)).unwrap();
    assert_eq!(
        mounts
            .visible_mounts()
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        [5]
    );
}

#[test]
fn malformed_or_incomplete_topology_fails_closed() {
    let fixtures = [
        vec![row(1, 0, "/", "ext4"), row(1, 1, "/a", "tmpfs")],
        vec![
            row(1, 0, "/", "ext4"),
            row(2, 3, "/a", "tmpfs"),
            row(3, 2, "/a", "tmpfs"),
        ],
        vec![
            row(1, 0, "/", "ext4"),
            row(2, 1, "/a", "tmpfs"),
            row(3, 2, "/b", "tmpfs"),
        ],
        vec![row(1, 0, "/", "ext4"), row(2, 99, "/a", "tmpfs")],
        vec![row(1, 0, "/", "ext4"), row(2, 2, "/a", "tmpfs")],
        vec![row(1, 0, "/", "ext4"), row(2, 1, "relative", "tmpfs")],
        vec![row(1, 0, "/", "ext4"), row(2, 1, "/a/../b", "tmpfs")],
        vec![row(1, 0, "/", "ext4"), row(2, 1, "/a\0b", "tmpfs")],
    ];
    for rows in fixtures {
        assert!(matches!(
            MountInfo::parse(&table(&rows, false)),
            Err(MountTableError::Invalid)
        ));
    }
    for bytes in [
        b"1 0 0:1 / / rw ext4 source rw\n".as_slice(),
        b"1 0 0:1 / / rw - ext4 source\n",
        b"\n",
    ] {
        assert!(matches!(
            MountInfo::parse(bytes),
            Err(MountTableError::Invalid)
        ));
    }
    assert!(matches!(
        MountInfo::parse(b""),
        Err(MountTableError::NoMount)
    ));
    assert!(matches!(
        MountInfo::parse(&table(
            &[row(1, 0, "/", "ext4"), row(2, 99, "/", "tmpfs")],
            false
        )),
        Err(MountTableError::Ambiguous)
    ));
    let mounts = MountInfo::parse(row(1, 0, "/", "ext4").as_bytes()).unwrap();
    for path in ["relative", "/a/../b", "/a\0b"] {
        assert_eq!(
            mounts.resolve(Path::new(path)),
            Err(MountTableError::Invalid)
        );
    }
}

#[test]
fn bounds_report_limits_instead_of_partial_inventory() {
    assert!(matches!(
        MountInfo::parse(&vec![b' '; MAX_MOUNTINFO_BYTES + 1]),
        Err(MountTableError::Limit)
    ));
    let mut bytes = row(1, 0, "/", "ext4").into_bytes();
    // A deep chain also proves that validation and projection do not recurse.
    for id in 2..=MAX_MOUNTINFO_RECORDS as u64 {
        bytes.extend_from_slice(row(id, id - 1, "/", "tmpfs").as_bytes());
    }
    let mounts = MountInfo::parse(&bytes).unwrap();
    assert_eq!(
        mounts.resolve(Path::new("/file")).unwrap().id,
        MAX_MOUNTINFO_RECORDS as u64
    );
    bytes.extend_from_slice(row(MAX_MOUNTINFO_RECORDS as u64 + 1, 1, "/extra", "tmpfs").as_bytes());
    assert!(matches!(
        MountInfo::parse(&bytes),
        Err(MountTableError::Limit)
    ));
    assert_eq!(
        mounts.resolve(Path::new(&format!("/{}", "x".repeat(MAX_MOUNTINFO_BYTES)))),
        Err(MountTableError::Limit)
    );
}
