//! Admission and cache-first delivery for one bounded thumbnail pass.

use gust_core::lister::CancelToken;
use std::ffi::OsString;
use std::path::Path;

pub fn cached_rows(
    dir: &Path,
    names: impl IntoIterator<Item = OsString>,
    cancel: &CancelToken,
) -> Vec<(OsString, String)> {
    let mut hits = Vec::new();
    for name in names {
        if cancel.is_cancelled() {
            break;
        }
        if let Some(thumb) =
            gust_thumbs::render::cached_thumbnail(&dir.join(&name), gust_thumbs::ThumbSize::Normal)
        {
            hits.push((name, thumb.url()));
        }
    }
    hits
}

pub fn run(
    dir: &Path,
    want: Vec<(OsString, &'static str)>,
    cancel: &CancelToken,
    publish: impl FnMut(Vec<(OsString, String)>),
) {
    run_with(
        want,
        cancel,
        |(name, _)| {
            gust_thumbs::render::cached_thumbnail(&dir.join(name), gust_thumbs::ThumbSize::Normal)
                .map(|t| t.url())
        },
        |(name, mime)| {
            gust_thumbs::render::thumbnail_cancellable(
                &dir.join(name),
                mime,
                gust_thumbs::ThumbSize::Normal,
                cancel,
            )
            .ok()
            .map(|t| t.url())
        },
        publish,
    );
}

fn run_with(
    want: Vec<(OsString, &'static str)>,
    cancel: &CancelToken,
    cached: impl Fn(&(OsString, &'static str)) -> Option<String>,
    render: impl Fn(&(OsString, &'static str)) -> Option<String> + Sync,
    mut publish: impl FnMut(Vec<(OsString, String)>),
) {
    let mut missing = Vec::new();
    let mut ready = Vec::new();
    for item in want {
        if cancel.is_cancelled() {
            return;
        }
        if let Some(url) = cached(&item) {
            ready.push((item.0, url));
        } else {
            // A changed/deleted source must not keep its previous pixels.
            ready.push((item.0.clone(), String::new()));
            missing.push(item);
        }
        if ready.len() == 64 {
            publish(std::mem::take(&mut ready));
        }
    }
    if !ready.is_empty() {
        publish(ready);
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut workers = 0;
        for _ in 0..2.min(missing.len()) {
            let tx = tx.clone();
            let missing = &missing;
            let next = &next;
            let render = &render;
            let started = std::thread::Builder::new()
                .name("gust-thumb-render".into())
                .spawn_scoped(scope, move || {
                    while !cancel.is_cancelled() {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = missing.get(i) else {
                            break;
                        };
                        if let Some(url) = render(item) {
                            if cancel.is_cancelled() || tx.send((item.0.clone(), url)).is_err() {
                                break;
                            }
                        }
                    }
                });
            if started.is_ok() {
                workers += 1;
            }
        }
        drop(tx);
        if workers == 0 {
            for item in &missing {
                if cancel.is_cancelled() {
                    break;
                }
                if let Some(url) = render(item) {
                    publish(vec![(item.0.clone(), url)]);
                }
            }
        }
        for result in rx {
            if !cancel.is_cancelled() {
                publish(vec![result]);
            }
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Unchanged,
    Pending,
    Start,
}

#[derive(Default)]
pub struct Work {
    active: bool,
    pending: bool,
    window: Option<(u64, usize, usize)>,
}

impl Work {
    pub fn request(&mut self, window: (u64, usize, usize)) -> Admission {
        if self.window == Some(window) {
            return Admission::Unchanged;
        }
        self.window = Some(window);
        if self.active {
            self.pending = true;
            Admission::Pending
        } else {
            self.active = true;
            Admission::Start
        }
    }

    /// Rows may have changed even when their generation and bounds did not.
    pub fn invalidate(&mut self) {
        self.window = None;
    }

    /// Cancel future work without forgetting a helper that has not exited.
    pub fn stop(&mut self) {
        self.invalidate();
        self.pending = false;
    }

    /// Called only after the worker exits; the next request reads current rows.
    pub fn finish(&mut self) -> bool {
        self.active = false;
        let pending = std::mem::take(&mut self.pending);
        if pending {
            self.invalidate();
        }
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_publish_before_generation_and_misses_have_two_workers() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let hit_published = AtomicBool::new(false);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let want = (0..12)
            .map(|i| (OsString::from(i.to_string()), "image/jpeg"))
            .collect();
        let mut delivered = Vec::new();
        run_with(
            want,
            &CancelToken::new(),
            |(name, _)| (name == "11").then(|| "cached".into()),
            |_| {
                assert!(hit_published.load(Ordering::SeqCst));
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5));
                active.fetch_sub(1, Ordering::SeqCst);
                Some("rendered".into())
            },
            |batch| {
                if batch.iter().any(|(_, url)| url == "cached") {
                    hit_published.store(true, Ordering::SeqCst);
                }
                delivered.extend(batch.into_iter().filter(|(_, url)| !url.is_empty()));
            },
        );
        assert_eq!(delivered.len(), 12);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancelled_pass_never_generates_or_publishes() {
        let cancel = CancelToken::new();
        cancel.cancel();
        run_with(
            vec![(OsString::from("image"), "image/jpeg")],
            &cancel,
            |_| panic!("cache lookup after cancellation"),
            |_| panic!("render after cancellation"),
            |_| panic!("publish after cancellation"),
        );
    }

    #[test]
    fn failed_revalidation_clears_previous_pixels() {
        let mut urls = Vec::new();
        run_with(
            vec![(OsString::from("changed"), "image/jpeg")],
            &CancelToken::new(),
            |_| None,
            |_| None,
            |batch| urls.extend(batch),
        );
        assert_eq!(urls, vec![(OsString::from("changed"), String::new())]);
    }

    #[test]
    fn rapid_scroll_waits_for_the_cancelled_helper_and_keeps_only_the_latest_request() {
        let mut work = Work::default();
        assert_eq!(work.request((1, 0, 20)), Admission::Start);
        for row in 1..=100 {
            assert_eq!(work.request((1, row, row + 20)), Admission::Pending);
        }
        assert_eq!(work.request((1, 100, 120)), Admission::Unchanged);
        assert!(work.finish());
        assert_eq!(work.request((1, 100, 120)), Admission::Start);
        assert!(!work.finish());
        assert_eq!(work.request((1, 100, 120)), Admission::Unchanged);
    }

    #[test]
    fn new_order_or_membership_reopens_the_same_bounds() {
        let mut work = Work::default();
        assert_eq!(work.request((1, 0, 20)), Admission::Start);
        assert!(!work.finish());
        work.invalidate();
        assert_eq!(work.request((1, 0, 20)), Admission::Start);
        work.invalidate();
        assert_eq!(work.request((1, 0, 20)), Admission::Pending);
    }

    #[test]
    fn leaving_grid_discards_pending_work_but_keeps_the_active_worker_counted() {
        let mut work = Work::default();
        assert_eq!(work.request((1, 0, 20)), Admission::Start);
        assert_eq!(work.request((1, 20, 40)), Admission::Pending);
        work.stop();
        assert!(!work.finish());
        assert_eq!(work.request((1, 20, 40)), Admission::Start);
        work.stop();
        assert_eq!(work.request((2, 0, 20)), Admission::Pending);
        assert!(work.finish());
        assert_eq!(work.request((2, 0, 20)), Admission::Start);
    }
}
