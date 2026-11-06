//! Pane-local sort choices which could not become remembered preferences.
#[cfg(test)]
use gust_core::entry::{Entry, SortField};
use gust_core::{entry::SortSpec, location::LocationKey, sorts::SortMemory};

#[derive(Default)]
pub struct SortState {
    local: Option<(LocationKey, SortSpec)>,
}
impl SortState {
    /// A suppressed/rejected intent still describes the requested runtime
    /// listing. A successfully admitted intent already lives in SortMemory.
    pub fn requested(&mut self, key: &LocationKey, spec: SortSpec, admitted: bool) {
        self.local = (!admitted).then(|| (key.clone(), spec));
    }

    /// Keep a transient choice through same-location reloads. Leaving that
    /// location retires it; navigation/back and later reloads consult memory.
    pub fn for_reload(&mut self, key: &LocationKey, remembered: &SortMemory) -> SortSpec {
        if let Some((location, spec)) = &self.local {
            if location == key {
                return *spec;
            }
        }
        self.local = None;
        gust_core::sorts::spec_for(remembered, key)
    }
}

/// The same branch used by the bridge before loading missing sort metadata.
#[cfg(test)]
pub fn needs_reload(spec: SortSpec, settled: bool, entries: &[Entry]) -> bool {
    !settled
        || (matches!(spec.field, SortField::Size | SortField::Modified)
            && entries.iter().any(|entry| entry.meta.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gust_core::lister::{fill_metadata, list_names, CancelToken};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn suppressed_choices_reach_the_actual_metadata_and_unsettled_reload_paths() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "gust-sort-reload-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir(&fixture.0).unwrap();
        std::fs::write(fixture.0.join("a-large"), b"larger").unwrap();
        std::fs::write(fixture.0.join("b-small"), b"x").unwrap();
        let key = LocationKey::local(&fixture.0);
        let mut remembered = SortMemory::new();
        remembered.remember(&key, SortField::Name, true);
        for mode in ["chooser", "admin"] {
            for (field, settled) in [(SortField::Size, true), (SortField::Name, false)] {
                let cancel = CancelToken::new();
                let mut listing = list_names(&key, 1, &cancel, |_| {});
                let requested = SortSpec {
                    field,
                    ascending: field == SortField::Size,
                    dirs_first: true,
                };
                assert!(listing.entries.iter().all(|entry| entry.meta.is_none()));
                assert!(needs_reload(requested, settled, &listing.entries), "{mode}");
                let mut state = SortState::default();
                state.requested(&key, requested, false);
                // This is the resolver load_current_cached uses, and the
                // resulting spec goes through the real metadata/sort pipeline.
                listing.sort = state.for_reload(&key, &remembered);
                fill_metadata(&mut listing, &cancel);
                assert!(listing.settled);
                assert_eq!(listing.entries[0].name, "b-small", "{mode}: {field:?}");
                assert_eq!(listing.sort, requested);
                assert_eq!(state.for_reload(&key, &remembered), requested);
            }
        }
    }

    #[test]
    fn admitted_choices_and_navigation_keep_using_current_shared_memory() {
        let a = LocationKey::local("/synthetic/a");
        let b = LocationKey::local("/synthetic/b");
        let mut remembered = SortMemory::new();
        remembered.remember(&a, SortField::Kind, false);
        remembered.remember(&b, SortField::Modified, true);
        let transient = SortSpec {
            field: SortField::Size,
            ascending: false,
            dirs_first: true,
        };
        let mut state = SortState::default();
        state.requested(&a, transient, false);
        assert_eq!(state.for_reload(&a, &remembered), transient);
        assert_eq!(
            state.for_reload(&b, &remembered),
            gust_core::sorts::spec_for(&remembered, &b)
        );
        assert_eq!(
            state.for_reload(&a, &remembered),
            gust_core::sorts::spec_for(&remembered, &a)
        );
        state.requested(&a, transient, false);
        remembered.remember(&a, SortField::Name, false);
        state.requested(&a, gust_core::sorts::spec_for(&remembered, &a), true);
        remembered.remember(&a, SortField::Modified, false);
        assert_eq!(
            state.for_reload(&a, &remembered),
            gust_core::sorts::spec_for(&remembered, &a)
        );
    }
}
