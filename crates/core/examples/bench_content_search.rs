//! Time one full content search over an existing fixture directory.
use gust_core::{grep::grep_with_report, location::LocationKey, search::SearchScope, CancelToken};
use std::time::Instant;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let root = args.next().expect("usage: bench_content_search DIR QUERY");
    let query = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .expect("expected a UTF-8 query");
    let scope = SearchScope::current_dir(LocationKey::local(root));
    let start = Instant::now();
    let report = grep_with_report(&scope, &query, &CancelToken::new(), |_| {});
    let elapsed = start.elapsed();
    println!(
        "state={:?} files={} bytes={} ms={:.3}",
        report.state,
        report.hits.len(),
        report.stats.read_bytes,
        elapsed.as_secs_f64() * 1000.0
    );
}
