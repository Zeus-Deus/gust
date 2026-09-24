//! Transfer speed and time left, from byte counts sampled over time.
//!
//! Pure: the caller passes the clock in, so the arithmetic is tested without
//! sleeping. The speed is measured over a short recent window, so an early
//! burst (page cache, a fast first file) does not keep promising a speed the
//! transfer has stopped reaching — and a stall shows as a stall.

use std::collections::VecDeque;

/// How far back the speed looks, in seconds.
const WINDOW_SECS: f64 = 3.0;
/// Below this much elapsed time a speed is noise, not a measurement.
const MIN_SPAN_SECS: f64 = 0.5;

#[derive(Debug, Default, Clone)]
pub struct RateMeter {
    samples: VecDeque<(f64, u64)>,
}

impl RateMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `bytes` had been transferred at time `t` (seconds, any
    /// monotonic origin).
    pub fn observe(&mut self, t: f64, bytes: u64) {
        if let Some(&(lt, lb)) = self.samples.back() {
            if t < lt || bytes < lb {
                // Time or bytes going backwards means a new transfer.
                self.samples.clear();
            } else if t == lt {
                self.samples.pop_back();
            }
        }
        self.samples.push_back((t, bytes));
        // Keep one sample at or before the window's start, so the window is
        // always fully covered once the transfer is old enough.
        while self.samples.len() > 2 && self.samples[1].0 <= t - WINDOW_SECS {
            self.samples.pop_front();
        }
    }

    /// Bytes per second over the recent window, or `None` until there is
    /// enough history to say.
    pub fn bytes_per_sec(&self) -> Option<f64> {
        let (&(t0, b0), &(t1, b1)) = (self.samples.front()?, self.samples.back()?);
        let span = t1 - t0;
        if span < MIN_SPAN_SECS {
            return None;
        }
        Some((b1 - b0) as f64 / span)
    }

    /// Whole seconds until `remaining` more bytes are done at the current
    /// speed. `None` when there is no speed yet or the transfer is stalled —
    /// "unknown" is honest; "forever" and "0 s" are not.
    pub fn eta_secs(&self, remaining: u64) -> Option<u64> {
        let r = self.bytes_per_sec()?;
        if r < 1.0 {
            return None;
        }
        Some((remaining as f64 / r).ceil() as u64)
    }
}

/// "45 MB/s", "1.5 MB/s", "512 B/s". Binary units, like the size column.
pub fn format_rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes_per_sec.max(0.0);
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 || v >= 10.0 {
        format!("{:.0} {}/s", v, UNITS[u])
    } else {
        format!("{v:.1} {}/s", UNITS[u])
    }
}

/// Time left, in words. Precise when it is short, rounded when it is long,
/// because nobody needs "3 min 17 s" to decide whether to wait.
pub fn format_eta(secs: u64) -> String {
    match secs {
        0..=5 => "a few seconds left".to_string(),
        6..=59 => format!("{secs} s left"),
        60..=3599 => format!("about {} min left", (secs + 30) / 60),
        _ => {
            let mins = (secs + 30) / 60;
            let (h, m) = (mins / 60, mins % 60);
            if m == 0 {
                format!("about {h} h left")
            } else {
                format!("about {h} h {m} min left")
            }
        }
    }
}
