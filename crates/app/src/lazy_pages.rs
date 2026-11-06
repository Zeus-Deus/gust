//! Stable write-once pages. Allocating a page constructs exactly 64 cells;
//! unvisited rows allocate only a page pointer. Values retire with their owner.
use std::sync::OnceLock;
pub const PAGE_ROWS: usize = 64;
pub struct Pages<T> {
    len: usize,
    pages: Vec<OnceLock<Box<[T; PAGE_ROWS]>>>,
}
impl<T: Default> Pages<T> {
    pub fn new(len: usize) -> Self {
        Self::try_new(len, &|| false).unwrap_or_else(|_| unreachable!())
    }
    pub fn try_new(len: usize, cancelled: &impl Fn() -> bool) -> Result<Self, Self> {
        let count = len.div_ceil(PAGE_ROWS);
        let mut result = Self {
            len,
            pages: Vec::with_capacity(count),
        };
        for i in 0..count {
            if i % 128 == 0 && cancelled() {
                return Err(result);
            }
            result.pages.push(OnceLock::new());
        }
        Ok(result)
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        Some(&self.pages.get(index / PAGE_ROWS)?.get()?[index % PAGE_ROWS])
    }
    pub fn cell(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        Some(
            &self.pages[index / PAGE_ROWS]
                .get_or_init(|| Box::new(std::array::from_fn(|_| T::default())))[index % PAGE_ROWS],
        )
    }
    #[cfg(test)]
    pub fn storage_bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<OnceLock<Box<[T; PAGE_ROWS]>>>()
            + self
                .pages
                .iter()
                .filter(|page| page.get().is_some())
                .count()
                * std::mem::size_of::<[T; PAGE_ROWS]>()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_cells_and_exact_last_partial_page_bound() {
        let pages = Pages::<OnceLock<String>>::new(65);
        let first = pages.cell(0).unwrap();
        first.set("first".into()).unwrap();
        let address = first as *const _;
        pages.cell(64).unwrap().set("last".into()).unwrap();
        assert_eq!(pages.get(0).unwrap() as *const _, address);
        assert_eq!(pages.get(64).unwrap().get().unwrap(), "last");
        assert!(pages.cell(65).is_none());
    }
    #[test]
    fn sparse_dense_and_cache_heavy_storage_are_measured() {
        use std::time::Instant;
        for len in [100_000usize, 1_000_000] {
            for touched in [0, 60, len] {
                let start = Instant::now();
                let pages = Pages::<OnceLock<[u64; 11]>>::new(len);
                for i in 0..touched {
                    pages.cell(i).unwrap().set([0; 11]).unwrap();
                }
                let bytes = pages.storage_bytes();
                let dense: Vec<OnceLock<[u64; 11]>> = (0..len).map(|_| OnceLock::new()).collect();
                let dense_bytes = dense.capacity() * std::mem::size_of::<OnceLock<[u64; 11]>>();
                eprintln!("sidecar-storage rows={len} touched={touched} paged={bytes} dense={dense_bytes} setup_us={}",start.elapsed().as_micros());
                if touched == len {
                    assert!(bytes < dense_bytes + dense_bytes / 100)
                } else {
                    assert!(bytes < dense_bytes / 100)
                }
            }
        }
    }
}
