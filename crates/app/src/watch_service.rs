//! Worker ownership for local watches: neither setup nor joining occurs in a
//! publication callback. The queue holds one handle per pane; intents coalesce.
use gust_core::watch::{Change, DirWatch};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
type Changed = Arc<dyn Fn(u64, Change) + Send + Sync>;
type Ready = Arc<dyn Fn(u64, bool) + Send + Sync>;
struct Intent {
    path: Option<PathBuf>,
    generation: u64,
}
struct State {
    intent: Option<Intent>,
    watch: Option<DirWatch>,
    queued: bool,
    running: bool,
    closed: bool,
    revision: u64,
    changed: Option<Changed>,
    ready: Option<Ready>,
}
#[cfg(test)]
#[derive(Default)]
struct Hooks {
    setup: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    drops: Mutex<Vec<String>>,
}
struct Inner {
    state: Mutex<State>,
    #[cfg(test)]
    hooks: Hooks,
}
pub struct Handle {
    inner: Arc<Inner>,
}
struct Executor {
    queue: Mutex<VecDeque<Arc<Inner>>>,
    ready: Condvar,
}
fn executor() -> &'static Arc<Executor> {
    static EXEC: OnceLock<Arc<Executor>> = OnceLock::new();
    EXEC.get_or_init(|| {
        let exec = Arc::new(Executor {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
        });
        let worker = exec.clone();
        std::thread::Builder::new()
            .name("gust-watch-owner".into())
            .spawn(move || loop {
                let inner = {
                    let mut queue = worker.queue.lock().unwrap();
                    while queue.is_empty() {
                        queue = worker.ready.wait(queue).unwrap();
                    }
                    queue.pop_front().unwrap()
                };
                inner.run();
            })
            .expect("watch executor creation");
        exec
    })
}
impl Handle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                #[cfg(test)]
                hooks: Hooks::default(),
                state: Mutex::new(State {
                    intent: None,
                    watch: None,
                    queued: false,
                    running: false,
                    closed: false,
                    revision: 0,
                    changed: None,
                    ready: None,
                }),
            }),
        }
    }
    pub fn subscribe(&self, changed: Changed, ready: Ready) {
        let mut state = self.inner.state.lock().unwrap();
        state.changed = Some(changed);
        state.ready = Some(ready);
    }
    pub fn replace(&self, path: Option<PathBuf>, generation: u64) {
        let mut state = self.inner.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.revision = state.revision.wrapping_add(1);
        state.intent = Some(Intent { path, generation });
        drop(state);
        self.inner.schedule();
    }
    fn close(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.closed = true;
        state.revision = state.revision.wrapping_add(1);
        state.intent = Some(Intent {
            path: None,
            generation: 0,
        });
        drop(state);
        self.inner.schedule();
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}
impl Inner {
    fn schedule(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        if !state.queued && !state.running && state.intent.is_some() {
            state.queued = true;
            let exec = executor();
            exec.queue.lock().unwrap().push_back(self.clone());
            exec.ready.notify_one();
        }
    }
    fn run(self: &Arc<Self>) {
        let (intent, revision, old, changed, ready, closed) = {
            let mut state = self.state.lock().unwrap();
            state.queued = false;
            let Some(intent) = state.intent.take() else {
                return;
            };
            state.running = true;
            (
                intent,
                state.revision,
                state.watch.take(),
                state.changed.clone(),
                state.ready.clone(),
                state.closed,
            )
        };
        #[cfg(test)]
        let had_old = old.is_some();
        drop(old);
        #[cfg(test)]
        if had_old {
            self.hooks.drops.lock().unwrap().push(
                std::thread::current()
                    .name()
                    .unwrap_or("unnamed")
                    .to_string(),
            );
        }
        #[cfg(test)]
        if let Some(hook) = self.hooks.setup.lock().unwrap().clone() {
            hook();
        }
        // DirWatch::drop joins its polling thread here.
        let watch = if !closed {
            intent
                .path
                .filter(|path| !gust_core::stall::needs_probe(path))
                .and_then(|path| {
                    let generation = intent.generation;
                    DirWatch::start(&path, move |change| {
                        if let Some(callback) = &changed {
                            callback(generation, change);
                        }
                    })
                    .ok()
                })
        } else {
            None
        };
        let active = watch.is_some();
        let mut state = self.state.lock().unwrap();
        state.running = false;
        if state.revision == revision && !state.closed {
            state.watch = watch;
            drop(state);
            if let Some(ready) = ready {
                ready(intent.generation, active);
            }
        } else {
            drop(state);
            #[cfg(test)]
            let stale = watch.is_some();
            drop(watch);
            #[cfg(test)]
            if stale {
                self.hooks.drops.lock().unwrap().push(
                    std::thread::current()
                        .name()
                        .unwrap_or("unnamed")
                        .to_string(),
                );
            }
        }
        self.schedule();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    #[test]
    fn stale_setup_and_final_watch_join_are_worker_owned() {
        let path =
            std::env::temp_dir().join(format!("gust-watch-owner-test-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let handle = Handle::new();
        let inner = handle.inner.clone();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicBool::new(false));
        let worker_gate = gate.clone();
        let worker_entered = entered.clone();
        let first = AtomicBool::new(true);
        *inner.hooks.setup.lock().unwrap() = Some(Arc::new(move || {
            if first.swap(false, Ordering::AcqRel) {
                worker_entered.store(true, Ordering::Release);
                let mut open = worker_gate.0.lock().unwrap();
                while !*open {
                    open = worker_gate.1.wait(open).unwrap();
                }
            }
        }));
        let ready = Arc::new(Mutex::new(Vec::new()));
        let seen = ready.clone();
        handle.subscribe(
            Arc::new(|_, _| {}),
            Arc::new(move |generation, active| seen.lock().unwrap().push((generation, active))),
        );
        handle.replace(Some(path.clone()), 1);
        let until = Instant::now() + Duration::from_secs(15);
        while !entered.load(Ordering::Acquire) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        for generation in 2..100 {
            handle.replace(Some(path.clone()), generation);
        }
        assert!(inner.state.lock().unwrap().intent.is_some());
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        while !ready
            .lock()
            .unwrap()
            .iter()
            .any(|&(generation, active)| generation == 99 && active)
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!ready
            .lock()
            .unwrap()
            .iter()
            .any(|&(generation, _)| generation == 1));
        drop(handle);
        loop {
            let done = {
                let state = inner.state.lock().unwrap();
                state.closed && !state.running && !state.queued && state.watch.is_none()
            };
            if done {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        let drops = inner.hooks.drops.lock().unwrap();
        assert!(drops.len() >= 2);
        assert!(drops.iter().all(|name| name == "gust-watch-owner"));
        drop(drops);
        std::fs::remove_dir_all(path).unwrap();
    }
}
