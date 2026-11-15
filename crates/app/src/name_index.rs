//! Randomized lookup of exact raw names without a second owned name set.
//! The canonical immutable backing owns bytes; table values are local indices.
use gust_core::Entry;
use hashbrown::HashTable;
use std::collections::hash_map::RandomState;
use std::ffi::OsStr;
use std::hash::BuildHasher;
use std::sync::Arc;
pub struct NameIndex {
    raw: Arc<Vec<Entry>>,
    table: HashTable<usize>,
    hash: RandomState,
}
impl NameIndex {
    pub fn empty(raw: Arc<Vec<Entry>>) -> Self {
        Self {
            raw,
            table: HashTable::new(),
            hash: RandomState::new(),
        }
    }
    pub fn try_new(
        raw: Arc<Vec<Entry>>,
        cancel: &impl gust_core::projection::Cancellation,
    ) -> Result<Self, Self> {
        let mut result = Self::empty(raw);
        result.table = HashTable::with_capacity(result.raw.len());
        for i in 0..result.raw.len() {
            if i % 128 == 0 && cancel.checkpoint() {
                return Err(result);
            }
            let hash = result.hash.hash_one(&result.raw[i].name);
            // Lister names are unique. Equality on lookup always checks bytes.
            result
                .table
                .insert_unique(hash, i, |&i| result.hash.hash_one(&result.raw[i].name));
        }
        Ok(result)
    }
    pub fn get(&self, name: &OsStr) -> Option<&usize> {
        self.table
            .find(self.hash.hash_one(name), |&i| self.raw[i].name == name)
    }
    pub fn contains_key(&self, name: &OsStr) -> bool {
        self.get(name).is_some()
    }
    pub fn len(&self) -> usize {
        self.table.len()
    }
    #[cfg(test)]
    pub fn for_test(names: impl IntoIterator<Item = std::ffi::OsString>) -> Self {
        let raw = Arc::new(
            names
                .into_iter()
                .map(|name| Entry::new(name, gust_core::EntryKind::File))
                .collect(),
        );
        Self::try_new(raw, &|| false).unwrap_or_else(|_| unreachable!())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn lookup_checks_raw_bytes_and_keeps_canonical_names_alive() {
        let names = [
            "a".into(),
            std::ffi::OsString::from_vec(vec![0xff]),
            std::ffi::OsString::from_vec(vec![0xfe]),
        ];
        let index = NameIndex::for_test(names.clone());
        for (i, name) in names.iter().enumerate() {
            assert_eq!(index.get(name), Some(&i));
        }
        assert_eq!(index.get(OsStr::new("missing")), None);
        assert_eq!(index.table.len(), 3);
    }
}
