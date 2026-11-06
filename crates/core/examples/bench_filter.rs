//! In-memory production filter and selection reconciliation, no I/O fixtures.
//! Run under the build lease:
//! flock -w 120 /tmp/gust-build.lock cargo run -p gust-core --release --example bench_filter -- [ROUNDS]
//! Defaults to seven measured rounds after one warmup for each size/query.
//! Names are file-000000.txt etc.; construction and selection setup are untimed.
use gust_core::entry::{Entry, EntryKind};
use gust_core::search::matching_indices;
use gust_core::selection::Selection;
use std::ffi::OsStr;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse().expect("positive round count"))
        .unwrap_or(7);
    assert!(rounds > 0);
    println!("entries,query,round,matches,index_checksum,filter_ms,prune_few_ms,prune_all_ms");
    for n in [100_000, 1_000_000] {
        let entries: Vec<_> = (0..n)
            .map(|i| Entry::new(format!("file-{i:06}.txt").into(), EntryKind::File))
            .collect();
        let mut few = Selection::new();
        for i in [0, n / 2, n - 1] {
            few.toggle(&entries[i].name);
        }
        few.toggle(OsStr::new("deleted"));
        let mut all = Selection::new();
        all.select_all_visible(&entries.iter().collect::<Vec<_>>());
        for query in [
            "",
            "file",
            "FiLe",
            "txt",
            "zzz",
            "19",
            "file-999999.txt-too-long",
        ] {
            for round in 0..=rounds {
                let mut selected_few = few.clone();
                let mut selected_all = all.clone();
                let start = Instant::now();
                let visible = matching_indices(black_box(&entries), black_box(query), false);
                let filter_time = start.elapsed();
                let start = Instant::now();
                selected_few.retain_visible(black_box(&entries), black_box(&visible));
                let few_time = start.elapsed();
                let start = Instant::now();
                selected_all.retain_visible(black_box(&entries), black_box(&visible));
                let all_time = start.elapsed();
                let checksum = visible.iter().fold(0xcbf29ce484222325_u64, |h, &i| {
                    (h ^ i as u64).wrapping_mul(0x100000001b3)
                });
                assert_eq!(selected_all.len(), visible.len());
                assert!(!selected_few.contains(OsStr::new("deleted")));
                black_box((&selected_few, &selected_all));
                if round > 0 {
                    println!(
                        "{n},{query},{round},{},{checksum:016x},{:.3},{:.3},{:.3}",
                        visible.len(),
                        filter_time.as_secs_f64() * 1000.0,
                        few_time.as_secs_f64() * 1000.0,
                        all_time.as_secs_f64() * 1000.0,
                    );
                }
            }
        }
    }
}
