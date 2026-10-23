//! Repeatable listing phases; no fixture creation or desktop interaction.
//! Run: cargo run -p gust-core --release --example bench_listing -- DIR [ROUNDS]
use gust_core::entry::{sort_entries, Completeness, SortSpec};
use gust_core::lister::{
    fill_metadata, fill_metadata_for, first_window, list_names, CancelToken, FIRST_WINDOW,
};
use gust_core::location::LocationKey;
use std::hint::black_box;
use std::os::unix::ffi::OsStrExt;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let key = LocationKey::local(args.next().expect("directory"));
    let rounds: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(7);
    let cancel = CancelToken::new();
    let source = list_names(&key, 0, &cancel, |_| {});
    assert_eq!(source.complete, Completeness::Complete);
    let shown: Vec<_> = (0..source.entries.len()).collect();
    let window = first_window(&shown, None, FIRST_WINDOW);
    let checksum = source.entries.iter().fold(0u64, |h, e| {
        e.name
            .as_bytes()
            .iter()
            .fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
    });
    println!(
        "# entries={} order_checksum={checksum:016x}",
        source.entries.len()
    );
    println!("round,names_ms,window_ms,metadata_ms,sort_ms,display_ms");
    for round in 0..rounds {
        let start = Instant::now();
        let mut snap = list_names(&key, round as u64, &cancel, |_| {});
        let names = start.elapsed();
        assert_eq!(snap.complete, Completeness::Complete);

        let start = Instant::now();
        assert!(fill_metadata_for(&mut snap, &window, &cancel));
        let window_time = start.elapsed();

        let start = Instant::now();
        fill_metadata(&mut snap, &cancel);
        let metadata = start.elapsed();
        assert!(snap.entries.iter().all(|e| e.meta.is_some()));

        let mut entries = source.entries.clone();
        let mut seed = 0x60d47e1d_u64;
        for i in (1..entries.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            entries.swap(i, seed as usize % (i + 1));
        }
        let start = Instant::now();
        sort_entries(&mut entries, SortSpec::default());
        let sort = start.elapsed();
        black_box(&entries);

        let start = Instant::now();
        for entry in &entries {
            black_box(entry.display_name());
        }
        let display = start.elapsed();
        println!(
            "{round},{:.3},{:.3},{:.3},{:.3},{:.3}",
            names.as_secs_f64() * 1000.0,
            window_time.as_secs_f64() * 1000.0,
            metadata.as_secs_f64() * 1000.0,
            sort.as_secs_f64() * 1000.0,
            display.as_secs_f64() * 1000.0,
        );
    }
}
