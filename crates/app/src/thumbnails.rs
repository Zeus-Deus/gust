//! Admission for one thumbnail worker, including its cancelled helper.

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
