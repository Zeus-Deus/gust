//! Repeatable release benchmark using a private cache and caller-owned fixtures.
//!
//! `cargo run --release -p gust-thumbs --example performance -- ROOT SOURCE MIME SIZE MODE`
//! SIZE is normal, large or xx-large; MODE is hits, cold or path. ROOT must
//! be a dedicated benchmark directory: its thumbnail for SOURCE is replaced.

use gust_thumbs::{cache, render, ThumbSize};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 6, "ROOT SOURCE MIME SIZE MODE");
    let root = PathBuf::from(&args[1]);
    std::fs::create_dir_all(&root).unwrap();
    // Only this benchmark process uses this cache; never touch the user's.
    std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
    let source = PathBuf::from(&args[2]);
    let mime = args[3].to_str().unwrap();
    let size = match args[4].to_str().unwrap() {
        "normal" => ThumbSize::Normal,
        "large" => ThumbSize::Large,
        "xx-large" => ThumbSize::XxLarge,
        _ => panic!("unknown size"),
    };
    let mode = args[5].to_str().unwrap();
    let target = render::thumbnail(&source, mime, size).unwrap();
    let iterations = match mode {
        "hits" => 10_000,
        "path" => 100_000,
        "cold" => 3,
        _ => panic!("unknown mode"),
    };
    for sample in 1..=9 {
        let mut elapsed = std::time::Duration::ZERO;
        for _ in 0..iterations {
            if mode == "cold" {
                std::fs::remove_file(&target).unwrap();
            }
            let start = Instant::now();
            match mode {
                "hits" => assert_eq!(
                    black_box(render::cached(&source, size)).as_ref(),
                    Some(&target)
                ),
                "cold" => assert_eq!(
                    black_box(render::thumbnail(&source, mime, size)).unwrap(),
                    target
                ),
                "path" => {
                    black_box(cache::thumbnail_path(&source, size).unwrap());
                }
                _ => unreachable!(),
            }
            elapsed += start.elapsed();
        }
        println!(
            "{mode},{},{sample},{iterations},{:.3}",
            size.pixels(),
            elapsed.as_nanos() as f64 / iterations as f64
        );
    }
}
