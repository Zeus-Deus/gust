//! Clipboard authority and cancellation, independent of Qt and filesystem I/O.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Desktop reads must never resurrect a remembered Cut when no files arrive.
/// Internal-only operation is available only outside a Wayland session.
pub fn choose<T>(remembered: Option<T>, desktop: Option<T>, has_desktop: bool) -> Option<T> {
    if has_desktop {
        desktop
    } else {
        desktop.or(remembered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_gates() -> (ReadGate, ReadGate) {
        let generation = Arc::new(AtomicU64::new(0));
        (
            ReadGate::with_generation(generation.clone()),
            ReadGate::with_generation(generation),
        )
    }

    #[test]
    fn newer_non_file_data_cannot_revive_a_remembered_cut() {
        let remembered = Some(("original.txt", "cut"));
        assert_eq!(choose(remembered, None, true), None);
    }

    #[test]
    fn a_desktop_copy_overrides_a_same_path_cut() {
        assert_eq!(
            choose(Some(("a", "cut")), Some(("a", "copy")), true),
            Some(("a", "copy"))
        );
    }

    #[test]
    fn an_ordinary_desktop_cut_still_moves() {
        assert_eq!(
            choose(Some(("a", "cut")), Some(("a", "cut")), true),
            Some(("a", "cut"))
        );
    }

    #[test]
    fn another_windows_files_win() {
        assert_eq!(choose(Some("a"), Some("b"), true), Some("b"));
    }

    #[test]
    fn no_wayland_session_preserves_the_explicit_internal_fallback() {
        assert_eq!(choose(Some(("a", "cut")), None, false), Some(("a", "cut")));
    }

    #[test]
    fn a_cancelled_read_cannot_admit_a_delayed_move() {
        let (mut gate, _) = isolated_gates();
        let old = gate.start();
        gate.cancel();
        assert!(!gate.finish(old));
    }

    #[test]
    fn an_old_reply_cannot_consume_a_newer_read() {
        let (mut gate, _) = isolated_gates();
        let old = gate.start();
        gate.cancel();
        let new = gate.start();
        assert!(!gate.finish(old));
        assert!(gate.busy());
        assert!(gate.finish(new));
        assert!(!gate.busy());
        assert!(!gate.finish(new));
    }

    #[test]
    fn another_models_write_retires_the_pending_read_without_sticking_busy() {
        let (mut a, mut b) = isolated_gates();
        let old = a.start();
        b.clipboard_changed();
        assert!(!a.busy());
        assert!(!a.finish(old));
        assert!(a.pending.is_none());
        let fresh = a.start();
        assert!(a.finish(fresh));
    }

    #[test]
    fn an_epoch_mismatched_reply_cannot_retire_a_fresh_read() {
        let (mut a, mut b) = isolated_gates();
        let old = a.start();
        b.clipboard_changed();
        let fresh = a.start();
        assert!(!a.finish(old));
        assert!(a.busy());
        assert!(a.finish(fresh));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReadTicket {
    id: u64,
    generation: u64,
}

pub struct ReadGate {
    next: u64,
    pending: Option<ReadTicket>,
    generation: Arc<AtomicU64>,
}

impl Default for ReadGate {
    fn default() -> Self {
        // This versions only writes in this process, not external writers.
        static GENERATION: OnceLock<Arc<AtomicU64>> = OnceLock::new();
        Self::with_generation(
            GENERATION
                .get_or_init(|| Arc::new(AtomicU64::new(0)))
                .clone(),
        )
    }
}

impl ReadGate {
    pub fn with_generation(generation: Arc<AtomicU64>) -> Self {
        Self {
            next: 0,
            pending: None,
            generation,
        }
    }

    pub fn start(&mut self) -> ReadTicket {
        self.next += 1;
        let ticket = ReadTicket {
            id: self.next,
            generation: self.generation.load(Ordering::SeqCst),
        };
        self.pending = Some(ticket);
        ticket
    }

    pub fn busy(&self) -> bool {
        self.pending
            .is_some_and(|p| p.generation == self.generation.load(Ordering::SeqCst))
    }

    pub fn clipboard_changed(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel();
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn finish(&mut self, ticket: ReadTicket) -> bool {
        if self.pending == Some(ticket) {
            // Retire even an epoch-mismatched reply, without consuming a
            // newer read or emitting an obsolete UI completion.
            self.pending = None;
            ticket.generation == self.generation.load(Ordering::SeqCst)
        } else {
            false
        }
    }
}
