//! GUI publication boundary. Work prepared in projection.rs is installed here
//! with Qt signals around one pointer replacement; row maps remain borrowed.
use super::*;
use crate::callback_metrics::{PublicationEvent, PublicationKind};
use crate::projection::{Gesture, Request};
use std::sync::Arc;
impl qobject::DirModel {
    pub(super) fn projection_subscribe(mut self: Pin<&mut Self>) {
        if self.as_ref().projection_subscribed {
            return;
        }
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        self.as_ref().runtime.subscribe(Arc::new(move || {
            thread.queue(|model| model.drain_projection()).is_ok()
        }));
        self.as_mut().rust_mut().projection_subscribed = true;
    }
    pub(super) fn focus_on_name(mut self: Pin<&mut Self>, name: std::ffi::OsString) {
        let mut state = self.as_mut().rust_mut();
        state.focus_revision = state.focus_revision.wrapping_add(1);
        state.focus_name = Some(name);
    }
    fn projection_request(&self, keep: i32) -> Request {
        Request {
            generation: self.generation,
            request: self.request_revision,
            selection_revision: self.selection_revision,
            query: self.filter.to_string(),
            sort: self.sort_spec(),
            sort_revision: self.sort_request_revision,
            hidden: self.show_hidden,
            cursor: self.cursor,
            focus: self.focus_name.clone(),
            focus_revision: self.focus_revision,
            keep,
            submitted: std::time::Instant::now(),
            clipboard: self.clip_set.clone(),
            marks_revision: self.runtime.marks_revision(),
        }
    }
    pub(super) fn queue_projection(mut self: Pin<&mut Self>, keep: i32) {
        self.as_mut().projection_subscribe();
        let revision = self.as_ref().request_revision.wrapping_add(1);
        self.as_mut().rust_mut().request_revision = revision;
        let request = self.as_ref().projection_request(keep);
        self.as_ref().runtime.request(request);
        self.as_mut().publish_pending();
    }
    fn publish_pending(mut self: Pin<&mut Self>) {
        let r = self.as_ref();
        let sorting = r.frame.all.generation != r.generation
            || (r.frame.sort != r.sort_spec() || r.frame.sort_revision != r.sort_request_revision);
        let filtering = r.frame.all.generation != r.generation
            || r.frame.query != r.filter.to_string()
            || r.frame.hidden != r.show_hidden;
        let selection = r.runtime.pending_selection();
        self.as_mut().set_sort_pending(sorting);
        self.as_mut().set_filter_pending(filtering);
        self.as_mut().set_selection_pending(selection);
    }
    pub(super) fn submit_selection(mut self: Pin<&mut Self>, gesture: Gesture) -> bool {
        let metrics = self.as_ref().callbacks.clone();
        let _timer = metrics.selection.timer();
        self.as_mut().projection_subscribe();
        let revision = self.as_ref().selection_revision.wrapping_add(1);
        let mut request = self.as_ref().projection_request(1);
        request.selection_revision = revision;
        match self.as_ref().runtime.gesture(gesture, request) {
            Ok(()) => {
                self.as_mut().rust_mut().selection_revision = revision;
                self.as_mut().set_selection_pending(true);
                true
            }
            Err(message) => {
                self.as_mut().set_status(QString::from(message));
                false
            }
        }
    }
    pub(super) fn refuse_pending_selection(mut self: Pin<&mut Self>) -> bool {
        if !self.as_ref().selection_pending {
            return false;
        }
        self.as_mut().set_status(QString::from(
            "Selection is still being applied; wait and try again.",
        ));
        true
    }
    pub(super) fn drain_projection(mut self: Pin<&mut Self>) {
        let metrics = self.as_ref().callbacks.clone();
        let _timer = metrics.publish.timer();
        if let Some(error) = self.as_ref().runtime.take_error() {
            self.as_mut().set_status(QString::from(&error));
        }
        let Some(frame) = self
            .as_ref()
            .runtime
            .take(self.as_ref().live_hold || self.as_ref().drag_hold)
        else {
            self.as_mut().publish_pending();
            return;
        };
        if frame.publication == crate::projection::Publication::Marks {
            let old = {
                let mut r = self.as_mut().rust_mut();
                r.marked_count = frame.marked;
                std::mem::replace(&mut r.frame, frame.clone())
            };
            let notify = metrics.publish_notify.timer();
            metrics
                .publications
                .marks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.as_mut().refresh_clip_marks();
            drop(notify);
            let _followup = metrics.publish_followup.timer();
            self.as_ref().runtime.installed(frame, old);
            self.as_mut().publish_pending();
            return;
        }
        let moved = frame.row_change;
        // A pure sort preserves membership. Qt remaps only persistent indices;
        // Rust supplies the already prepared full row map without cloning it.
        let layout = moved
            && Arc::ptr_eq(&frame.all, &self.as_ref().frame.all)
            && frame.query == self.as_ref().frame.query
            && frame.hidden == self.as_ref().frame.hidden
            && (frame.sort != self.as_ref().frame.sort
                || frame.sort_revision != self.as_ref().frame.sort_revision);
        let recorder = metrics.publication_events.as_deref();
        let mut publication = if moved {
            recorder.map(|recorder| PublicationEvent {
                sequence: recorder.next_sequence(),
                kind: if layout {
                    PublicationKind::Layout
                } else {
                    PublicationKind::Reset
                },
                shown_request_revision: frame.request,
                listing_revision: frame.all.listing,
                old_rows: self.as_ref().frame.visible.len(),
                new_rows: frame.visible.len(),
                view_first: self.as_ref().view_rows.0,
                view_count: self.as_ref().view_rows.1,
                ..PublicationEvent::default()
            })
        } else {
            None
        };
        let mapping_owner = frame.clone();
        let structural = moved.then(|| metrics.publish_structural.timer());
        if moved {
            let _phase = metrics.publish_phases.begin.timer();
            if layout {
                metrics
                    .publications
                    .layouts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                metrics
                    .publications
                    .resets
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            unsafe {
                if layout {
                    self.as_mut().begin_sort_layout();
                } else {
                    self.as_mut().begin_reset_model();
                }
            }
        }
        let swap = moved.then(|| metrics.publish_phases.swap.timer());
        let old = {
            let mut r = self.as_mut().rust_mut();
            std::mem::replace(&mut r.frame, frame.clone())
        };
        {
            let mut r = self.as_mut().rust_mut();
            r.key = frame.all.key.clone();
            r.publishing_mapping = moved;
            r.count = frame.visible.len() as i32;
            if frame.marks_revision == r.runtime.marks_revision() {
                r.marked_count = frame.marked;
            }
            if moved {
                r.thumb_cancel.cancel();
                r.thumb_work.invalidate();
                r.size_cancel.cancel();
                r.size_work.invalidate();
            }
            if moved && !Arc::ptr_eq(&old.all, &frame.all) {
                r.meta_cancel.cancel();
                r.meta_again = Some(r.view_rows);
            }
        }
        drop(swap);
        if moved {
            let before = recorder.map(|recorder| recorder.data_snapshot());
            let phase = metrics.publish_phases.end.timer();
            unsafe {
                if layout {
                    self.as_mut().end_sort_layout(&mapping_owner.mapping);
                } else {
                    self.as_mut().end_reset_model();
                }
            }
            if let (Some(event), Some(recorder), Some(before)) =
                (publication.as_mut(), recorder, before)
            {
                event.end_ns = phase.elapsed_ns();
                before.finish(recorder.data_snapshot(), event);
            }
        }
        drop(structural);
        let notify = metrics.publish_notify.timer();
        if !moved {
            metrics
                .publications
                .selection
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let cursor_phase = metrics.publish_phases.cursor.timer();
        let request_focus = frame.focus_revision != 0;
        let cursor = if moved && frame.keep != 0 && !request_focus {
            usize::try_from(self.as_ref().cursor)
                .ok()
                .and_then(|i| frame.mapping.get(i).copied())
                .filter(|i| *i >= 0)
                .unwrap_or(frame.cursor)
        } else if moved {
            frame.cursor
        } else {
            self.as_ref().cursor
        };
        let cursor_changed = self.as_ref().cursor != cursor;
        self.as_mut().set_cursor(cursor);
        if moved && !cursor_changed {
            self.as_mut().cursor_changed();
        }
        drop(cursor_phase);
        {
            let _phase = metrics.publish_phases.count.timer();
            self.as_mut().count_changed();
        }
        let properties = metrics.publish_phases.properties.timer();
        self.as_mut().set_shown_filter(QString::from(&frame.query));
        self.as_mut()
            .set_selection_text(QString::from(&frame.summary));
        let settled = frame.all.settled && frame.all.generation == self.as_ref().generation;
        self.as_mut().set_settled(settled);
        drop(properties);
        if moved {
            let status_phase = metrics.publish_phases.status.timer();
            let display = frame.all.key.display_path();
            let location_changed_by_token = old.all.key != frame.all.key;
            let same_display = self.as_ref().location.to_string() == display;
            self.as_mut().set_location(QString::from(&display));
            if location_changed_by_token && same_display {
                self.as_mut().location_changed();
            }
            let shown = frame
                .all
                .key
                .as_path()
                .and_then(|p| {
                    let (root, label) = place_of(p)?;
                    let rest = p.strip_prefix(root).ok()?;
                    Some(if rest.as_os_str().is_empty() {
                        label
                    } else {
                        format!(
                            "{label}/{}",
                            gust_core::location::escape_for_display(rest.as_os_str())
                        )
                    })
                })
                .unwrap_or(display);
            self.as_mut().set_location_display(QString::from(&shown));
            self.as_mut().publish_empty_reason(&frame.all.complete);
            let status = match &frame.all.complete {
                gust_core::entry::Completeness::Failed { error } => format!("cannot open: {error}"),
                _ if frame.all.ranked && self.as_ref().finding => {
                    let count = frame.visible.len();
                    let needle = self.as_ref().find_needle.to_string();
                    if count == 0 {
                        format!("Nothing named \"{needle}\" here")
                    } else {
                        format!(
                            "{count} result{} for \"{needle}\" — Escape to go back",
                            if count == 1 { "" } else { "s" }
                        )
                    }
                }
                _ if frame.query.is_empty() => count_items(frame.visible.len()),
                _ => format!(
                    "{} of {} match \"{}\"",
                    frame.visible.len(),
                    frame.all.len(),
                    frame.query
                ),
            };
            self.as_mut().set_status(QString::from(&status));
            if frame.focus_revision == self.as_ref().focus_revision {
                self.as_mut().rust_mut().focus_name = None;
            }
            self.as_mut().rust_mut().live_busy = false;
            drop(status_phase);
            let _viewport = metrics.publish_phases.viewport.timer();
            let keep = if layout {
                2
            } else if request_focus {
                1
            } else {
                frame.keep
            };
            if let Some(event) = publication.as_mut() {
                event.keep = keep;
            }
            self.as_mut().rows_replaced(keep);
            self.as_mut().rust_mut().publishing_mapping = false;
        } else {
            self.as_mut().refresh_selection_flags();
        }
        drop(notify);
        let _followup = metrics.publish_followup.timer();
        {
            let _phase = metrics.publish_phases.runtime.timer();
            self.as_ref().runtime.installed(frame, old);
            self.as_mut().publish_pending();
        }
        if moved {
            {
                let _phase = metrics.publish_phases.watch.timer();
                self.as_mut().ensure_watch();
                self.as_mut().start_cloud_notice();
            }
            let (first, count) = self.as_ref().view_rows;
            let _phase = metrics.publish_phases.want_rows.timer();
            self.as_mut()
                .want_rows(first, if count > 0 { count } else { 60 });
        }
        if self.as_ref().live_dirty && !self.as_ref().live_hold && !self.as_ref().selection_pending
        {
            self.as_mut().start_live_relist();
        }
        drop(_followup);
        if let (Some(mut event), Some(recorder)) = (publication, recorder) {
            // Same origin as the existing publish timer. Its eventual drop
            // still includes this final bounded insertion and lock cost.
            event.total_ns = _timer.elapsed_ns();
            recorder.record(event);
        }
    }
    pub(super) fn published_row(&self, old: i32) -> i32 {
        if !self.publishing_mapping {
            return -1;
        }
        usize::try_from(old)
            .ok()
            .and_then(|i| self.frame.mapping.get(i).copied())
            .unwrap_or(-1)
    }
    pub(super) fn projection_json(&self) -> serde_json::Value {
        serde_json::json!({"display_revision":self.frame.display_revision,"listing_revision":self.frame.all.listing,"shown_load_revision":self.frame.all.load_revision,
            "request_revision":self.request_revision,"shown_request_revision":self.frame.request,
            "selection_revision":self.selection_revision,"shown_selection_revision":self.frame.selection_revision,
            "shown_generation":self.frame.all.generation,"shown_location":self.frame.all.key.to_token(),
            "shown_filter":self.shown_filter.to_string(),"sort_pending":self.sort_pending,"filter_pending":self.filter_pending,
            "selection_pending":self.selection_pending,"validated_thumbnail_count":self.frame.validated_thumbnail_count,"publications":self.callbacks.publications.json(),"slots":self.runtime.slots(),"worker":self.runtime.metrics().json(),"callbacks":self.callbacks.json()})
    }
}
