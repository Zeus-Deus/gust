//! Fixed-size callback histograms and opt-in bounded publication attribution.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
const LIMITS: [u64; 19] = [
    1,
    2,
    4,
    8,
    16,
    32,
    64,
    128,
    256,
    512,
    1000,
    2000,
    4000,
    8000,
    16000,
    32000,
    64000,
    128000,
    u64::MAX,
];
#[derive(Default)]
pub struct Histogram {
    count: AtomicU64,
    max: AtomicU64,
    over: AtomicU64,
    bins: [AtomicU64; 19],
}
impl Histogram {
    pub fn timer(&self) -> Timer<'_> {
        Timer {
            histogram: self,
            start: Instant::now(),
        }
    }
    fn record(&self, us: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.max.fetch_max(us, Ordering::Relaxed);
        if us > 8000 {
            self.over.fetch_add(1, Ordering::Relaxed);
        }
        let bin = LIMITS.iter().position(|&limit| us <= limit).unwrap_or(18);
        self.bins[bin].fetch_add(1, Ordering::Relaxed);
    }
    pub fn json(&self) -> serde_json::Value {
        let count = self.count.load(Ordering::Relaxed);
        let target = count.saturating_mul(99).div_ceil(100);
        let mut sum = 0;
        let mut p99 = 0;
        if count > 0 {
            for (i, bin) in self.bins.iter().enumerate() {
                sum += bin.load(Ordering::Relaxed);
                if sum >= target {
                    p99 = LIMITS[i].min(self.max.load(Ordering::Relaxed));
                    break;
                }
            }
        }
        serde_json::json!({"count":count,"p99_us":p99,"max_us":self.max.load(Ordering::Relaxed),"over_8ms":self.over.load(Ordering::Relaxed)})
    }
}
pub struct Timer<'a> {
    histogram: &'a Histogram,
    start: Instant,
}
impl Timer<'_> {
    pub fn elapsed_ns(&self) -> u64 {
        nanos(self.start.elapsed())
    }
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.histogram
            .record(self.start.elapsed().as_micros().min(u64::MAX as u128) as u64)
    }
}
#[derive(Default)]
pub struct PublishPhases {
    pub begin: Histogram,
    pub swap: Histogram,
    pub end: Histogram,
    pub cursor: Histogram,
    pub count: Histogram,
    pub properties: Histogram,
    pub status: Histogram,
    pub viewport: Histogram,
    pub runtime: Histogram,
    pub watch: Histogram,
    pub want_rows: Histogram,
}
impl PublishPhases {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"begin":self.begin.json(),"swap":self.swap.json(),"end":self.end.json(),"cursor":self.cursor.json(),"count":self.count.json(),"properties":self.properties.json(),"status":self.status.json(),"viewport":self.viewport.json(),"runtime":self.runtime.json(),"watch":self.watch.json(),"want_rows":self.want_rows.json()})
    }
}
#[derive(Default)]
pub struct Publications {
    pub resets: AtomicU64,
    pub layouts: AtomicU64,
    pub selection: AtomicU64,
    pub marks: AtomicU64,
}
impl Publications {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"resets":self.resets.load(Ordering::Relaxed),"layouts":self.layouts.load(Ordering::Relaxed),"selection":self.selection.load(Ordering::Relaxed),"marks":self.marks.load(Ordering::Relaxed)})
    }
}
#[derive(Default)]
pub struct Callbacks {
    pub filter: Histogram,
    pub sort: Histogram,
    pub publish: Histogram,
    pub merge: Histogram,
    pub data: Histogram,
    pub targets: Histogram,
    pub selection: Histogram,
    pub publish_structural: Histogram,
    pub publish_notify: Histogram,
    pub publish_followup: Histogram,
    pub publish_phases: PublishPhases,
    pub publications: Publications,
    pub publication_events: Option<Box<PublicationRecorder>>,
}
impl Callbacks {
    pub fn from_environment() -> Self {
        Self::with_publication_probe(Self::probe_enabled(
            std::env::var_os("GUST_QA_STATE").as_deref(),
            std::env::var_os("GUST_QA_PUBLICATION_PROBE").as_deref(),
        ))
    }
    fn probe_enabled(state: Option<&std::ffi::OsStr>, flag: Option<&std::ffi::OsStr>) -> bool {
        state.is_some_and(|path| !path.is_empty()) && flag == Some(std::ffi::OsStr::new("1"))
    }
    pub fn with_publication_probe(enabled: bool) -> Self {
        Self {
            publication_events: enabled.then(|| Box::new(PublicationRecorder::new())),
            ..Self::default()
        }
    }
    pub fn data_timer(&self, name_role: bool) -> DataTimer<'_> {
        DataTimer {
            start: Instant::now(),
            histogram: &self.data,
            recorder: self.publication_events.as_deref(),
            name_role,
        }
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({"filter":self.filter.json(),"sort":self.sort.json(),"publish":self.publish.json(),"merge":self.merge.json(),"data":self.data.json(),"targets":self.targets.json(),"selection":self.selection.json(),"publish_structural":self.publish_structural.json(),"publish_notify":self.publish_notify.json(),"publish_followup":self.publish_followup.json(),"publish_phases":self.publish_phases.json(),"publication_events":self.publication_events.as_deref().map(PublicationRecorder::json).unwrap_or_else(disabled_publications)})
    }
}
fn nanos(duration: std::time::Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

// This replaces the old data timer, sharing its single start/end clock reads.
// Its interval is the Rust body only, before CXX converts the returned QVariant.
pub struct DataTimer<'a> {
    start: Instant,
    histogram: &'a Histogram,
    recorder: Option<&'a PublicationRecorder>,
    name_role: bool,
}
impl Drop for DataTimer<'_> {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        self.histogram
            .record(elapsed.as_micros().min(u64::MAX as u128) as u64);
        if let Some(recorder) = self.recorder {
            recorder.completed_data(nanos(elapsed), self.name_role);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PublicationKind {
    #[default]
    Reset,
    Layout,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublicationEvent {
    pub sequence: u64,
    pub kind: PublicationKind,
    pub shown_request_revision: u64,
    pub listing_revision: u64,
    pub old_rows: usize,
    pub new_rows: usize,
    pub view_first: i32,
    pub view_count: i32,
    pub keep: i32,
    pub total_ns: u64,
    pub end_ns: u64,
    pub end_data_ns: u64,
    pub end_data_calls: u64,
    pub end_name_calls: u64,
}
impl PublicationEvent {
    fn json(self) -> serde_json::Value {
        serde_json::json!({
            "sequence":self.sequence,
            "kind":match self.kind { PublicationKind::Reset => "reset", PublicationKind::Layout => "layout" },
            "shown_request_revision":self.shown_request_revision,"listing_revision":self.listing_revision,
            "old_rows":self.old_rows,"new_rows":self.new_rows,
            "view_first":self.view_first,"view_count":self.view_count,"keep":self.keep,
            "total_ns":self.total_ns,"end_ns":self.end_ns,"end_data_ns":self.end_data_ns,
            "end_data_calls":self.end_data_calls,"end_name_calls":self.end_name_calls,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataSnapshot {
    ns: u64,
    calls: u64,
    name_calls: u64,
}
impl DataSnapshot {
    pub fn finish(self, after: Self, event: &mut PublicationEvent) {
        event.end_data_ns = after.ns.wrapping_sub(self.ns);
        event.end_data_calls = after.calls.wrapping_sub(self.calls);
        event.end_name_calls = after.name_calls.wrapping_sub(self.name_calls);
    }
}
#[derive(Default)]
struct KindDurations {
    total: Histogram,
    end: Histogram,
}
impl KindDurations {
    fn record(&self, event: PublicationEvent) {
        self.total.record(event.total_ns / 1000);
        self.end.record(event.end_ns / 1000);
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"count":self.total.count.load(Ordering::Relaxed),"total":self.total.json(),"end":self.end.json()})
    }
}
const WORST_LIMIT: usize = 8;
#[derive(Default)]
struct Worst {
    events: [PublicationEvent; WORST_LIMIT],
    len: usize,
}
impl Worst {
    // Descending duration, then earlier model-local sequence for stable ties.
    fn insert(&mut self, event: PublicationEvent, duration: fn(PublicationEvent) -> u64) {
        let position = self.events[..self.len]
            .iter()
            .position(|&old| {
                duration(event) > duration(old)
                    || (duration(event) == duration(old) && event.sequence < old.sequence)
            })
            .unwrap_or(self.len);
        if position == WORST_LIMIT {
            return;
        }
        let end = self.len.min(WORST_LIMIT - 1);
        self.events.copy_within(position..end, position + 1);
        self.events[position] = event;
        self.len = (self.len + 1).min(WORST_LIMIT);
    }
    fn json(&self) -> serde_json::Value {
        self.events[..self.len]
            .iter()
            .map(|event| event.json())
            .collect()
    }
}
#[derive(Default)]
struct PublicationState {
    resets: KindDurations,
    layouts: KindDurations,
    worst_end: Worst,
    worst_total: Worst,
}
pub struct PublicationRecorder {
    recorder_id: u64,
    sequence: AtomicU64,
    data_ns: AtomicU64,
    data_calls: AtomicU64,
    name_calls: AtomicU64,
    state: Mutex<PublicationState>,
}
impl PublicationRecorder {
    fn new() -> Self {
        static NEXT_RECORDER_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            recorder_id: NEXT_RECORDER_ID.fetch_add(1, Ordering::Relaxed),
            sequence: AtomicU64::new(0),
            data_ns: AtomicU64::new(0),
            data_calls: AtomicU64::new(0),
            name_calls: AtomicU64::new(0),
            state: Mutex::default(),
        }
    }
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
    fn completed_data(&self, ns: u64, name_role: bool) {
        self.data_ns.fetch_add(ns, Ordering::Relaxed);
        self.data_calls.fetch_add(1, Ordering::Relaxed);
        if name_role {
            self.name_calls.fetch_add(1, Ordering::Relaxed);
        }
    }
    // Same-model GUI calls are synchronous. No guard survives a Qt call;
    // completed bodies before/after end_* delimit the attribution window.
    pub fn data_snapshot(&self) -> DataSnapshot {
        DataSnapshot {
            ns: self.data_ns.load(Ordering::Relaxed),
            calls: self.data_calls.load(Ordering::Relaxed),
            name_calls: self.name_calls.load(Ordering::Relaxed),
        }
    }
    pub fn record(&self, event: PublicationEvent) {
        let mut state = self.state.lock().unwrap();
        match event.kind {
            PublicationKind::Reset => state.resets.record(event),
            PublicationKind::Layout => state.layouts.record(event),
        }
        state.worst_end.insert(event, |event| event.end_ns);
        state.worst_total.insert(event, |event| event.total_ns);
    }
    pub fn json(&self) -> serde_json::Value {
        let state = self.state.lock().unwrap();
        serde_json::json!({"enabled":true,"recorder_id":self.recorder_id,"resets":state.resets.json(),"layouts":state.layouts.json(),
            "worst_end":state.worst_end.json(),"worst_total":state.worst_total.json()})
    }
}
fn disabled_publications() -> serde_json::Value {
    let empty = KindDurations::default().json();
    serde_json::json!({"enabled":false,"resets":empty,"layouts":empty,"worst_end":[],"worst_total":[]})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histogram_has_bounded_storage_and_tail_counts() {
        let h = Histogram::default();
        for _ in 0..99 {
            h.record(13)
        }
        h.record(9000);
        let v = h.json();
        assert_eq!(v["count"], 100);
        assert_eq!(v["p99_us"], 16);
        assert_eq!(v["max_us"], 9000);
        assert_eq!(v["over_8ms"], 1);
    }
    #[test]
    fn publication_probe_gate_and_disabled_pointer_state() {
        use std::ffi::OsStr;
        let state = Some(OsStr::new("/owned/qa.json"));
        for flag in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("0")),
            Some(OsStr::new("true")),
        ] {
            assert!(!Callbacks::probe_enabled(state, flag));
        }
        let on = Some(OsStr::new("1"));
        assert!(!Callbacks::probe_enabled(None, on));
        assert!(!Callbacks::probe_enabled(Some(OsStr::new("")), on));
        assert!(Callbacks::probe_enabled(state, on));
        let disabled = Callbacks::with_publication_probe(false);
        assert!(disabled.publication_events.is_none());
        assert_eq!(
            std::mem::size_of::<Option<Box<PublicationRecorder>>>(),
            std::mem::size_of::<usize>()
        );
        drop(disabled.data_timer(true));
        assert_eq!(disabled.data.json()["count"], 1);
        assert_eq!(
            disabled.json()["publication_events"],
            disabled_publications()
        );
        assert_eq!(
            disabled.publications.json(),
            serde_json::json!({"resets":0,"layouts":0,"selection":0,"marks":0})
        );
        let first = Callbacks::with_publication_probe(true);
        let second = Callbacks::with_publication_probe(true);
        let mut enabled_old_fields = first.json();
        let mut disabled_old_fields = Callbacks::with_publication_probe(false).json();
        enabled_old_fields
            .as_object_mut()
            .unwrap()
            .remove("publication_events");
        disabled_old_fields
            .as_object_mut()
            .unwrap()
            .remove("publication_events");
        assert_eq!(enabled_old_fields, disabled_old_fields);
        let first_id = first.json()["publication_events"]["recorder_id"]
            .as_u64()
            .unwrap();
        let second_id = second.json()["publication_events"]["recorder_id"]
            .as_u64()
            .unwrap();
        assert_ne!(first_id, 0);
        assert_ne!(first_id, second_id);
        assert!(disabled.json()["publication_events"]
            .get("recorder_id")
            .is_none());
    }

    #[test]
    fn publication_top_eights_are_bounded_correlated_and_independently_ordered() {
        let recorder = PublicationRecorder::new();
        let mut events = Vec::new();
        for i in 1..=64 {
            let event = PublicationEvent {
                sequence: recorder.next_sequence(),
                kind: if i % 2 == 0 {
                    PublicationKind::Layout
                } else {
                    PublicationKind::Reset
                },
                shown_request_revision: i + 100,
                listing_revision: i + 200,
                old_rows: i as usize,
                new_rows: i as usize + 1,
                view_first: -1,
                view_count: 60,
                keep: 2,
                total_ns: i * 10_000,
                end_ns: (65 - i) * 1000,
                end_data_ns: i * 13,
                end_data_calls: i * 2,
                end_name_calls: i,
            };
            events.push(event);
            recorder.record(event);
        }
        let json = recorder.json();
        assert_eq!(json["resets"]["count"], 32);
        assert_eq!(json["layouts"]["count"], 32);
        for kind in ["resets", "layouts"] {
            for phase in ["total", "end"] {
                assert_eq!(json[kind][phase]["count"], 32);
            }
        }
        assert_eq!(json["resets"]["total"]["max_us"], 630);
        assert_eq!(json["layouts"]["total"]["max_us"], 640);
        assert_eq!(json["resets"]["end"]["max_us"], 64);
        assert_eq!(json["layouts"]["end"]["max_us"], 63);
        assert!(json.to_string().len() < 16 * 1024);
        assert_eq!(json["worst_end"].as_array().unwrap().len(), WORST_LIMIT);
        assert_eq!(json["worst_total"].as_array().unwrap().len(), WORST_LIMIT);
        for i in 0..WORST_LIMIT {
            assert_eq!(json["worst_end"][i], events[i].json());
            assert_eq!(json["worst_total"][i], events[63 - i].json());
        }
    }

    #[test]
    fn publication_ties_keep_earliest_sequence_and_histograms_include_evicted_events() {
        let recorder = PublicationRecorder::new();
        for sequence in (1..=20).rev() {
            recorder.record(PublicationEvent {
                sequence,
                total_ns: 9_000_999,
                end_ns: 333,
                ..PublicationEvent::default()
            });
        }
        let json = recorder.json();
        assert_eq!(
            json["resets"]["total"],
            serde_json::json!({"count":20,"p99_us":9000,"max_us":9000,"over_8ms":20})
        );
        assert_eq!(
            json["resets"]["end"],
            serde_json::json!({"count":20,"p99_us":0,"max_us":0,"over_8ms":0})
        );
        assert_eq!(json["layouts"]["count"], 0);
        for key in ["worst_end", "worst_total"] {
            for i in 0..WORST_LIMIT {
                assert_eq!(json[key][i]["sequence"], i + 1);
                assert_eq!(json[key][i]["total_ns"], 9_000_999);
                assert_eq!(json[key][i]["end_ns"], 333);
            }
        }
    }

    #[test]
    fn publication_data_deltas_sum_nanoseconds_before_conversion() {
        let recorder = PublicationRecorder::new();
        recorder.completed_data(7000, true);
        let before = recorder.data_snapshot();
        recorder.completed_data(333, false);
        recorder.completed_data(444, true);
        recorder.completed_data(555, true);
        let mut event = PublicationEvent::default();
        before.finish(recorder.data_snapshot(), &mut event);
        recorder.completed_data(9000, false);
        assert_eq!(event.end_data_ns, 1332);
        assert_eq!(event.end_data_calls, 3);
        assert_eq!(event.end_name_calls, 2);
        assert_eq!(event.end_data_ns / 1000, 1);
        // Snapshot subtraction also retains deltas across a cumulative wrap.
        let before = DataSnapshot {
            ns: u64::MAX - 9,
            calls: u64::MAX,
            name_calls: u64::MAX,
        };
        before.finish(
            DataSnapshot {
                ns: 10,
                calls: 1,
                name_calls: 0,
            },
            &mut event,
        );
        assert_eq!(
            (
                event.end_data_ns,
                event.end_data_calls,
                event.end_name_calls
            ),
            (20, 2, 1)
        );
    }
}
