//! Listing-local cache with at most two stable write-once values per key.
//! Workers allocate page indices; first-touch publication may construct one
//! bounded 64-cell page. Invalidated/replaced values remain owned until the
//! backing retires. Exhaustion asks the bridge for a fresh bounded backing.
use std::ffi::{OsStr, OsString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
pub struct Cache<T> {
    index: Arc<crate::name_index::NameIndex>,
    slots: crate::lazy_pages::Pages<Slot<T>>,
    renewals: crate::lazy_pages::Pages<OnceLock<T>>,
    disabled: AtomicBool,
    count: AtomicUsize,
}
struct Slot<T> {
    value: OnceLock<T>,
    invalid: AtomicBool,
    renewal_active: AtomicBool,
}
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: OnceLock::new(),
            invalid: AtomicBool::new(false),
            renewal_active: AtomicBool::new(false),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum InsertResult {
    Changed,
    Unchanged,
    Exhausted,
    Unavailable,
}
impl<T> Cache<T> {
    pub fn new(index: Arc<crate::name_index::NameIndex>) -> Self {
        Self::try_new(index, &|| false).unwrap_or_else(|_| unreachable!())
    }
    pub fn try_new(
        index: Arc<crate::name_index::NameIndex>,
        cancel: &impl gust_core::projection::Cancellation,
    ) -> Result<Self, Self> {
        let mut cache = Self {
            slots: crate::lazy_pages::Pages::new(0),
            renewals: crate::lazy_pages::Pages::new(0),
            index,
            disabled: AtomicBool::new(false),
            count: AtomicUsize::new(0),
        };
        match crate::lazy_pages::Pages::try_new(cache.index.len(), &|| cancel.checkpoint()) {
            Ok(slots) => cache.slots = slots,
            Err(slots) => {
                cache.slots = slots;
                return Err(cache);
            }
        }
        match crate::lazy_pages::Pages::try_new(cache.index.len(), &|| cancel.checkpoint()) {
            Ok(renewals) => cache.renewals = renewals,
            Err(renewals) => {
                cache.renewals = renewals;
                return Err(cache);
            }
        }
        Ok(cache)
    }
    pub fn get(&self, name: &OsStr) -> Option<&T> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        let slot = self.slots.get(*self.index.get(name)?)?;
        if slot.invalid.load(Ordering::Acquire) {
            None
        } else if slot.renewal_active.load(Ordering::Acquire) {
            self.renewals.get(*self.index.get(name)?)?.get()
        } else {
            slot.value.get()
        }
    }
    pub fn contains_key(&self, name: &OsStr) -> bool {
        self.get(name).is_some()
    }
    /// Publications have a single writer; concurrent readers keep stable
    /// references to either retained cell. Repeated values reuse that cell.
    pub fn insert(&self, name: OsString, mut value: T) -> InsertResult
    where
        T: PartialEq,
    {
        if self.disabled.load(Ordering::Acquire) {
            return InsertResult::Unavailable;
        }
        let Some(&index) = self.index.get(&name) else {
            return InsertResult::Unavailable;
        };
        let slot = self.slots.cell(index).expect("listing index");
        if slot.value.get().is_none() {
            match slot.value.set(value) {
                Ok(()) => {
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return InsertResult::Changed;
                }
                Err(returned) => value = returned,
            }
        }
        let activate = |renewal: bool| {
            let previous = slot.renewal_active.swap(renewal, Ordering::AcqRel);
            let invalid = slot.invalid.swap(false, Ordering::AcqRel);
            if invalid {
                self.count.fetch_add(1, Ordering::Relaxed);
            }
            if invalid || previous != renewal {
                InsertResult::Changed
            } else {
                InsertResult::Unchanged
            }
        };
        if slot.value.get() == Some(&value) {
            return activate(false);
        }
        let renewal = self.renewals.cell(index).expect("listing index");
        if renewal.get().is_none() {
            match renewal.set(value) {
                Ok(()) => {
                    activate(true);
                    return InsertResult::Changed;
                }
                Err(returned) => value = returned,
            }
        }
        if renewal.get() == Some(&value) {
            return activate(true);
        }
        // Both cells remain alive for existing borrows. A third version is
        // never silently displayed under an obsolete URL/value.
        self.remove(&name);
        InsertResult::Exhausted
    }
    pub fn remove(&self, name: &OsStr) -> Option<&T> {
        let value = self.get(name)?;
        let slot = self.slots.get(*self.index.get(name)?)?;
        if !slot.invalid.swap(true, Ordering::AcqRel) {
            self.count.fetch_sub(1, Ordering::Relaxed);
            Some(value)
        } else {
            None
        }
    }
    pub fn clear(&self) {
        self.disabled.store(true, Ordering::Release);
        self.count.store(0, Ordering::Relaxed);
    }
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Directory totals are Copy scalars, so they need no stable retained versions.
/// One publisher writes while old-backing workers may concurrently copy reads.
/// Guards protect only scalar operations and never reach a caller or callback.
pub struct SizeCache {
    index: Arc<crate::name_index::NameIndex>,
    slots: crate::lazy_pages::Pages<std::sync::Mutex<Option<gust_core::dirsize::Sized>>>,
    disabled: AtomicBool,
    count: AtomicUsize,
}
impl SizeCache {
    pub fn new(index: Arc<crate::name_index::NameIndex>) -> Self {
        Self::try_new(index, &|| false).unwrap_or_else(|_| unreachable!())
    }
    pub fn try_new(
        index: Arc<crate::name_index::NameIndex>,
        cancel: &impl gust_core::projection::Cancellation,
    ) -> Result<Self, Self> {
        let mut cache = Self {
            slots: crate::lazy_pages::Pages::new(0),
            index,
            disabled: AtomicBool::new(false),
            count: AtomicUsize::new(0),
        };
        match crate::lazy_pages::Pages::try_new(cache.index.len(), &|| cancel.checkpoint()) {
            Ok(slots) => {
                cache.slots = slots;
                Ok(cache)
            }
            Err(slots) => {
                cache.slots = slots;
                Err(cache)
            }
        }
    }
    pub fn get(&self, name: &OsStr) -> Option<gust_core::dirsize::Sized> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        let slot = self.slots.get(*self.index.get(name)?)?;
        let value = *slot.lock().unwrap_or_else(|e| e.into_inner());
        value
    }
    pub fn contains_key(&self, name: &OsStr) -> bool {
        self.get(name).is_some()
    }
    /// Single-writer publication replaces even a third distinct scalar directly.
    /// Sized owns no allocation or destructor; displaced copies need no disposal.
    pub fn insert(&self, name: OsString, value: gust_core::dirsize::Sized) -> InsertResult {
        if self.disabled.load(Ordering::Acquire) {
            return InsertResult::Unavailable;
        }
        let Some(&index) = self.index.get(&name) else {
            return InsertResult::Unavailable;
        };
        let slot = self.slots.cell(index).expect("listing index");
        let previous = {
            let mut current = slot.lock().unwrap_or_else(|e| e.into_inner());
            current.replace(value)
        };
        if previous == Some(value) {
            return InsertResult::Unchanged;
        }
        if previous.is_none() {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        InsertResult::Changed
    }
    pub fn remove(&self, name: &OsStr) -> Option<gust_core::dirsize::Sized> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        let slot = self.slots.get(*self.index.get(name)?)?;
        let previous = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if previous.is_some() {
            self.count.fetch_sub(1, Ordering::Relaxed);
        }
        previous
    }
    /// Constant-time logical disablement. Pages/index retire with the backing.
    pub fn clear(&self) {
        self.disabled.store(true, Ordering::Release);
        self.count.store(0, Ordering::Relaxed);
    }
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn size_reads_are_independent_copies_including_partial_and_empty_totals() {
        use gust_core::dirsize::Sized;
        let cache = SizeCache::new(Arc::new(crate::name_index::NameIndex::for_test([
            "a".into(),
            "cold".into(),
        ])));
        let empty = Sized {
            bytes: 0,
            files: 0,
            complete: true,
        };
        let partial = Sized {
            bytes: 731,
            files: 19,
            complete: false,
        };
        assert_eq!(cache.get(OsStr::new("cold")), None);
        assert_eq!(cache.insert("a".into(), empty), InsertResult::Changed);
        let first: Sized = cache.get(OsStr::new("a")).unwrap();
        assert_eq!(cache.remove(OsStr::new("a")), Some(empty));
        assert_eq!(cache.insert("a".into(), partial), InsertResult::Changed);
        let second: Sized = cache.get(OsStr::new("a")).unwrap();
        cache.clear();
        assert_eq!((first, second), (empty, partial));
        assert_eq!(cache.get(OsStr::new("a")), None);
    }
    #[test]
    fn scalar_sequences_match_an_independent_current_value_oracle() {
        use gust_core::dirsize::Sized;
        use std::collections::HashMap;
        use std::os::unix::ffi::OsStringExt;
        let names: Vec<OsString> = vec![
            "a".into(),
            "cold".into(),
            OsString::from_vec(vec![0xff]),
            OsString::from_vec(vec![0xfe]),
        ];
        let mut seed = 0xcafef00du64;
        for _ in 0..8 {
            let cache = SizeCache::new(Arc::new(crate::name_index::NameIndex::for_test(
                names.clone(),
            )));
            let mut expected = HashMap::new();
            let mut disabled = false;
            for step in 0..1000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let name = &names[(seed >> 32) as usize % names.len()];
                let value = Sized {
                    bytes: seed % 17,
                    files: (seed >> 8) % 13,
                    complete: seed & 1 == 0,
                };
                match (seed >> 16) % 5 {
                    0 | 1 => {
                        let result = if disabled {
                            InsertResult::Unavailable
                        } else {
                            let previous = expected.insert(name.clone(), value);
                            if previous == Some(value) {
                                InsertResult::Unchanged
                            } else {
                                InsertResult::Changed
                            }
                        };
                        assert_eq!(cache.insert(name.clone(), value), result);
                    }
                    2 => {
                        assert_eq!(cache.remove(name), expected.remove(name));
                    }
                    3 => {
                        if let Some(&old) = expected.get(name) {
                            assert_eq!(cache.insert(name.clone(), old), InsertResult::Unchanged);
                        }
                    }
                    _ => {
                        assert_eq!(cache.get(name), expected.get(name).copied());
                    }
                }
                if step == 900 {
                    cache.clear();
                    cache.clear();
                    disabled = true;
                    expected.clear();
                }
                assert_eq!(cache.len(), expected.len());
                for key in &names {
                    assert_eq!(cache.get(key), expected.get(key).copied());
                }
                assert_eq!(cache.get(OsStr::new("unknown")), None);
                assert_eq!(
                    cache.insert("unknown".into(), value),
                    InsertResult::Unavailable
                );
            }
        }
    }
    #[test]
    fn third_and_later_scalars_replace_without_retaining_history() {
        use gust_core::dirsize::Sized;
        let cache = SizeCache::new(Arc::new(crate::name_index::NameIndex::for_test([
            "a".into()
        ])));
        let cold = cache.slots.storage_bytes();
        let mut populated = 0;
        for bytes in 0..1000 {
            let value = Sized {
                bytes,
                files: bytes ^ 0xabcd,
                complete: bytes % 2 == 0,
            };
            assert_eq!(cache.insert("a".into(), value), InsertResult::Changed);
            assert_eq!(cache.get(OsStr::new("a")), Some(value));
            assert_eq!(cache.insert("a".into(), value), InsertResult::Unchanged);
            assert_eq!(cache.len(), 1);
            if bytes == 0 {
                populated = cache.slots.storage_bytes();
            }
            assert_eq!(cache.slots.storage_bytes(), populated);
        }
        assert_eq!(
            populated - cold,
            64 * std::mem::size_of::<std::sync::Mutex<Option<Sized>>>()
        );
        assert!(!std::mem::needs_drop::<Sized>());
    }
    #[test]
    fn scalar_single_writer_and_concurrent_worker_copies_never_tear() {
        use gust_core::dirsize::Sized;
        let index = Arc::new(crate::name_index::NameIndex::for_test([
            "a".into(),
            "cold".into(),
        ]));
        let cache = SizeCache::new(index.clone());
        let barrier = std::sync::Barrier::new(5);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let cache = &cache;
                let barrier = &barrier;
                let index = index.clone();
                scope.spawn(move || {
                    barrier.wait();
                    let copied = SizeCache::new(index);
                    for _ in 0..20000 {
                        if let Some(value) = cache.get(OsStr::new("a")) {
                            assert_eq!(value.files, value.bytes ^ 0xdeadbeef);
                            assert_eq!(value.complete, value.bytes & 1 == 0);
                            copied.insert("a".into(), value);
                            assert_eq!(copied.get(OsStr::new("a")), Some(value));
                        }
                        assert_eq!(cache.get(OsStr::new("cold")), None);
                    }
                });
            }
            barrier.wait();
            for bytes in 0..20000 {
                cache.insert(
                    "a".into(),
                    Sized {
                        bytes,
                        files: bytes ^ 0xdeadbeef,
                        complete: bytes & 1 == 0,
                    },
                );
                if bytes % 7 == 0 {
                    cache.remove(OsStr::new("a"));
                }
            }
        });
        cache.clear();
        assert_eq!(cache.get(OsStr::new("a")), None);
        assert_eq!(cache.len(), 0);
    }
    #[test]
    fn cancelled_scalar_construction_keeps_index_owned_until_recipient_drop() {
        let index = Arc::new(crate::name_index::NameIndex::for_test(
            (0..100000).map(|i| i.to_string().into()),
        ));
        let weak = Arc::downgrade(&index);
        let count = AtomicUsize::new(0);
        let partial = SizeCache::try_new(index, &|| count.fetch_add(1, Ordering::Relaxed) > 2)
            .err()
            .unwrap();
        assert!(partial.slots.storage_bytes() > 0);
        assert!(weak.upgrade().is_some());
        std::thread::Builder::new()
            .name("scalar-retirement-recipient".into())
            .spawn(move || {
                assert!(weak.upgrade().is_some());
                drop(partial);
                assert!(weak.upgrade().is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }
    struct Notice(Arc<AtomicUsize>);
    impl PartialEq for Notice {
        fn eq(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.0, &other.0)
        }
    }
    impl Drop for Notice {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    #[test]
    fn invalidation_never_destroys_slots_and_names_are_listing_local() {
        let index = Arc::new(crate::name_index::NameIndex::for_test([
            "a".into(),
            "b".into(),
        ]));
        let cache = Cache::new(index);
        let drops = Arc::new(AtomicUsize::new(0));
        cache.insert("a".into(), Notice(drops.clone()));
        cache.insert("b".into(), Notice(drops.clone()));
        assert_eq!(cache.len(), 2);
        assert!(cache.remove(OsStr::new("a")).is_some());
        assert!(cache.get(OsStr::new("a")).is_none());
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        cache.clear();
        assert!(cache.get(OsStr::new("b")).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(cache);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn partial_slot_construction_returns_its_owner_on_cancellation() {
        let index = Arc::new(crate::name_index::NameIndex::for_test(
            (0..100_000).map(|i| OsString::from(format!("{i}"))),
        ));
        let count = AtomicUsize::new(0);
        let partial =
            Cache::<String>::try_new(index, &|| count.fetch_add(1, Ordering::Relaxed) > 2)
                .err()
                .unwrap();
        assert!(partial.slots.storage_bytes() > 0);
    }
    #[test]
    fn renewal_preserves_borrows_and_repeated_values_do_not_spend_capacity() {
        use InsertResult::*;
        let cache = Cache::new(Arc::new(crate::name_index::NameIndex::for_test([
            "a".into(),
            "cold".into(),
        ])));
        let initial_slots = cache.slots.storage_bytes();
        let initial_renewals = cache.renewals.storage_bytes();
        assert!(cache.remove(OsStr::new("cold")).is_none());
        assert_eq!(cache.slots.storage_bytes(), initial_slots);
        assert_eq!(cache.insert("a".into(), String::from("first")), Changed);
        let first = cache.get(OsStr::new("a")).unwrap();
        for _ in 0..300 {
            assert_eq!(cache.insert("a".into(), String::from("first")), Unchanged);
        }
        assert_eq!(cache.renewals.storage_bytes(), initial_renewals);
        assert_eq!(cache.remove(OsStr::new("a")), Some(first));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.insert("a".into(), String::from("second")), Changed);
        let second = cache.get(OsStr::new("a")).unwrap();
        for _ in 0..300 {
            assert_eq!(cache.insert("a".into(), String::from("second")), Unchanged);
        }
        assert_eq!(cache.len(), 1);
        assert_eq!(first, "first");
        assert_eq!(second, "second");
        assert_eq!(cache.insert("a".into(), String::from("third")), Exhausted);
        assert!(cache.get(OsStr::new("a")).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(first, "first");
        assert_eq!(second, "second");
        assert_eq!(cache.insert("a".into(), String::from("second")), Changed);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.insert("a".into(), String::from("first")), Changed);
        assert_eq!(cache.get(OsStr::new("a")), Some(first));
        assert_eq!(cache.insert("a".into(), String::from("third")), Exhausted);
        assert_eq!(
            cache.insert("cold".into(), String::from("first-cold-value")),
            Changed
        );
    }
    #[test]
    fn primary_slot_layout_is_unchanged_by_the_active_cell_flag() {
        #[allow(dead_code)]
        struct Legacy<T> {
            value: OnceLock<T>,
            invalid: AtomicBool,
        }
        assert_eq!(
            std::mem::size_of::<Slot<String>>(),
            std::mem::size_of::<Legacy<String>>()
        );
        assert_eq!(
            std::mem::size_of::<Slot<gust_core::dirsize::Sized>>(),
            std::mem::size_of::<Legacy<gust_core::dirsize::Sized>>()
        );
        assert_eq!(std::mem::size_of::<Slot<String>>(), 40);
    }
    #[test]
    fn both_retained_versions_drop_only_on_the_recipient_thread() {
        struct Tracked {
            id: u32,
            drops: Arc<std::sync::Mutex<Vec<(u32, String)>>>,
        }
        impl PartialEq for Tracked {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id
            }
        }
        impl Drop for Tracked {
            fn drop(&mut self) {
                self.drops.lock().unwrap().push((
                    self.id,
                    std::thread::current().name().unwrap_or("unnamed").into(),
                ));
            }
        }
        let drops = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cache = Cache::new(Arc::new(crate::name_index::NameIndex::for_test([
            "a".into()
        ])));
        cache.insert(
            "a".into(),
            Tracked {
                id: 1,
                drops: drops.clone(),
            },
        );
        cache.insert(
            "a".into(),
            Tracked {
                id: 2,
                drops: drops.clone(),
            },
        );
        cache.remove(OsStr::new("a"));
        cache.clear();
        assert!(drops.lock().unwrap().is_empty());
        std::thread::Builder::new()
            .name("cache-retirement-recipient".into())
            .spawn(move || drop(cache))
            .unwrap()
            .join()
            .unwrap();
        let mut records = drops.lock().unwrap().clone();
        records.sort();
        assert_eq!(
            records,
            vec![
                (1, "cache-retirement-recipient".into()),
                (2, "cache-retirement-recipient".into())
            ]
        );
    }
}
