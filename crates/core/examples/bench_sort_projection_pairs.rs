//! Alternating pure-CPU legacy entry-sort / index-sort pairs.
use gust_core::entry::{sort_entries, Entry, EntryId, EntryKind, SortSpec};
use gust_core::projection::sort_indices;
use gust_core::CancelToken;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::time::Instant;

fn checksum<'a>(entries: impl Iterator<Item = &'a Entry>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for e in entries {
        for b in e.id.0.to_le_bytes().iter().chain(e.name.as_bytes()) {
            h = (h ^ *b as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }
    h
}

fn run(n: usize, case: &str) {
    let mut entries: Vec<_> = (0..n)
        .map(|i| {
            let number = (i * 524_287) % n;
            let mut bytes = format!(
                "{}File{number:08}.txt",
                if number.is_multiple_of(11) { "." } else { "" }
            )
            .into_bytes();
            if number.is_multiple_of(29) {
                bytes.push(0xff);
            }
            let mut e = Entry::new(
                OsString::from_vec(bytes),
                if number.is_multiple_of(7) {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            );
            e.id = EntryId(number as u64);
            e
        })
        .collect();
    match case {
        "random" => {
            let mut seed = 0x1234_5678_90ab_cdef_u64;
            for i in (1..n).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                entries.swap(i, seed as usize % (i + 1));
            }
        }
        "ordered" => sort_entries(&mut entries, SortSpec::default()),
        "permutation" => {}
        _ => unreachable!(),
    }
    let mut expected = entries.clone();
    sort_entries(&mut expected, SortSpec::default());
    let expected_checksum = checksum(expected.iter());
    for pair in 0usize..4 {
        // Legacy input cloning is setup, outside timed sorting. The index
        // worker borrows the same immutable fixture on every run.
        let mut legacy = entries.clone();
        for index_worker in if pair.is_multiple_of(2) {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            if index_worker {
                let indices =
                    sort_indices(&entries, SortSpec::default(), &CancelToken::new()).unwrap();
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.;
                assert_eq!(indices.len(), n);
                let mut seen = vec![false; n];
                for (&index, expected) in indices.iter().zip(&expected) {
                    assert!(!seen[index]);
                    seen[index] = true;
                    assert_eq!(entries[index].id, expected.id);
                    assert_eq!(entries[index].name, expected.name);
                }
                assert!(seen.into_iter().all(|v| v));
                assert_eq!(
                    checksum(indices.iter().map(|&i| &entries[i])),
                    expected_checksum
                );
                println!("pair n={n} case={case} round={pair} first={} which=index ms={elapsed_ms:.3} checksum={expected_checksum:016x} exact_membership_order=true", if pair.is_multiple_of(2) {"legacy"} else {"index"});
            } else {
                sort_entries(&mut legacy, SortSpec::default());
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.;
                for (actual, expected) in legacy.iter().zip(&expected) {
                    assert_eq!(actual.id, expected.id);
                    assert_eq!(actual.name, expected.name);
                }
                assert_eq!(checksum(legacy.iter()), expected_checksum);
                println!("pair n={n} case={case} round={pair} first={} which=legacy ms={elapsed_ms:.3} checksum={expected_checksum:016x} exact_membership_order=true", if pair.is_multiple_of(2) {"legacy"} else {"index"});
            }
        }
    }
}

fn main() {
    println!("profile_debug_assertions={} pairs=4 order=AB_BA_AB_BA timed=sort_only setup_clones_excluded=true app_latency_measured=false", cfg!(debug_assertions));
    for n in [100_000, 1_000_000] {
        for case in ["permutation", "random", "ordered"] {
            run(n, case);
        }
    }
}
