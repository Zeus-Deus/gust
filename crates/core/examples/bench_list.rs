// Measures the two real phases: names (what the user sees first) and
// metadata (what makes the columns true).
fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let key = gust_core::location::LocationKey::local(std::path::Path::new(&dir));
    let cancel = gust_core::lister::CancelToken::new();

    let t = std::time::Instant::now();
    let mut snap = gust_core::lister::list_names(&key, 0, &cancel, |_| {});
    let names_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t2 = std::time::Instant::now();
    gust_core::lister::fill_metadata(&mut snap, &cancel);
    let meta_ms = t2.elapsed().as_secs_f64() * 1000.0;

    println!(
        "{} {:.1} {:.1}",
        snap.entries.len(),
        names_ms,
        names_ms + meta_ms
    );
}
