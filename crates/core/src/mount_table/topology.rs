use super::{decode_field, fields};
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub const MAX_MOUNTINFO_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MOUNTINFO_RECORDS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountTableError {
    Limit,
    Invalid,
    Ambiguous,
    NoMount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountRecord {
    pub id: u64,
    pub parent_id: u64,
    pub mount_point: PathBuf,
    pub fs_type: String,
}

/// A bounded mountinfo snapshot. Construction and queries perform no I/O.
#[derive(Debug, Clone)]
pub struct MountInfo {
    records: Vec<MountRecord>,
    visible: Vec<usize>,
    ambiguous: Vec<PathBuf>,
}

type Children = BTreeMap<u64, BTreeMap<PathBuf, Vec<usize>>>;

fn number(bytes: &[u8]) -> Result<u64, MountTableError> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(MountTableError::Invalid);
    }
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(MountTableError::Invalid)
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().as_bytes().contains(&0)
        && !path.components().any(|c| c == Component::ParentDir)
}

impl MountInfo {
    pub fn parse(bytes: &[u8]) -> Result<Self, MountTableError> {
        if bytes.len() > MAX_MOUNTINFO_BYTES {
            return Err(MountTableError::Limit);
        }
        let mut records = Vec::new();
        let mut by_id = BTreeMap::new();
        for line in bytes.split_inclusive(|b| *b == b'\n') {
            if records.len() >= MAX_MOUNTINFO_RECORDS {
                return Err(MountTableError::Limit);
            }
            let mut parts = fields(line.strip_suffix(b"\n").unwrap_or(line));
            let id = number(parts.next().ok_or(MountTableError::Invalid)?)?;
            let parent_id = number(parts.next().ok_or(MountTableError::Invalid)?)?;
            let _device = parts.next().ok_or(MountTableError::Invalid)?;
            let _root = parts.next().ok_or(MountTableError::Invalid)?;
            let mount_point =
                PathBuf::from(decode_field(parts.next().ok_or(MountTableError::Invalid)?));
            let _options = parts.next().ok_or(MountTableError::Invalid)?;
            if id == 0 || !valid_path(&mount_point) || !parts.by_ref().any(|f| f == b"-") {
                return Err(MountTableError::Invalid);
            }
            let fs_type = std::str::from_utf8(parts.next().ok_or(MountTableError::Invalid)?)
                .map_err(|_| MountTableError::Invalid)?
                .to_owned();
            let _source = parts.next().ok_or(MountTableError::Invalid)?;
            let _super_options = parts.next().ok_or(MountTableError::Invalid)?;
            if parts.next().is_some() || by_id.insert(id, records.len()).is_some() {
                return Err(MountTableError::Invalid);
            }
            records.push(MountRecord {
                id,
                parent_id,
                mount_point,
                fs_type,
            });
        }
        if records.is_empty() {
            return Err(MountTableError::NoMount);
        }
        // Validate parent chains iteratively, so hostile depth cannot overflow a stack.
        let mut states = vec![0u8; records.len()];
        for start in 0..records.len() {
            let mut chain = Vec::new();
            let mut current = start;
            loop {
                match states[current] {
                    1 => return Err(MountTableError::Invalid),
                    2 => break,
                    _ => {}
                }
                states[current] = 1;
                chain.push(current);
                let record = &records[current];
                if record.parent_id == record.id {
                    if record.mount_point != Path::new("/") {
                        return Err(MountTableError::Invalid);
                    }
                    break;
                }
                let Some(&parent) = by_id.get(&record.parent_id) else {
                    if record.mount_point != Path::new("/") {
                        return Err(MountTableError::Invalid);
                    }
                    break;
                };
                if !record.mount_point.starts_with(&records[parent].mount_point) {
                    return Err(MountTableError::Invalid);
                }
                current = parent;
            }
            for index in chain {
                states[index] = 2;
            }
        }
        let roots: Vec<_> = records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent_id == r.id || !by_id.contains_key(&r.parent_id))
            .map(|(i, _)| i)
            .collect();
        let root = match roots.as_slice() {
            [root] => *root,
            [] => return Err(MountTableError::NoMount),
            _ => return Err(MountTableError::Ambiguous),
        };
        let mut children = Children::new();
        for (index, record) in records.iter().enumerate() {
            if by_id.contains_key(&record.parent_id) && record.id != record.parent_id {
                children
                    .entry(record.parent_id)
                    .or_default()
                    .entry(record.mount_point.clone())
                    .or_default()
                    .push(index);
            }
        }
        let mut visible = Vec::new();
        let mut ambiguous = Vec::new();
        let mut pending = vec![root];
        while let Some(start) = pending.pop() {
            let mut current = start;
            let mut unresolved = false;
            // Absolute lookup starts at the process's held root, not its cover.
            // Root stacks are ambiguous without that anchor; only descend elsewhere.
            loop {
                let record = &records[current];
                let covering = children
                    .get(&record.id)
                    .and_then(|groups| groups.get(&record.mount_point));
                match covering.map(Vec::as_slice) {
                    None => break,
                    Some([child]) if current != root => current = *child,
                    _ => {
                        ambiguous.push(record.mount_point.clone());
                        unresolved = true;
                        break;
                    }
                }
            }
            let record = &records[current];
            if unresolved {
                continue;
            }
            visible.push(current);
            let Some(groups) = children.get(&record.id) else {
                continue;
            };
            let mut groups: Vec<_> = groups.iter().collect();
            groups.sort_unstable_by(|a, b| a.0.components().cmp(b.0.components()));
            let mut covering: Option<&Path> = None;
            for (point, indices) in groups {
                if covering.is_some_and(|parent| point.starts_with(parent)) {
                    continue;
                }
                // A nearer child covers deeper siblings attached to this old parent.
                covering = Some(point);
                match indices.as_slice() {
                    [child] => pending.push(*child),
                    _ => ambiguous.push(point.clone()),
                }
            }
        }
        visible.sort_unstable_by(|a, b| {
            records[*a]
                .mount_point
                .components()
                .cmp(records[*b].mount_point.components())
        });
        Ok(Self {
            records,
            visible,
            ambiguous,
        })
    }

    pub fn resolve(&self, path: &Path) -> Result<&MountRecord, MountTableError> {
        if path.as_os_str().as_bytes().len() > MAX_MOUNTINFO_BYTES {
            return Err(MountTableError::Limit);
        }
        if !valid_path(path) {
            return Err(MountTableError::Invalid);
        }
        if self.ambiguous.iter().any(|p| path.starts_with(p)) {
            return Err(MountTableError::Ambiguous);
        }
        self.visible
            .iter()
            .map(|index| &self.records[*index])
            .filter(|record| path.starts_with(&record.mount_point))
            .max_by_key(|record| record.mount_point.as_os_str().as_bytes().len())
            .ok_or(MountTableError::NoMount)
    }

    /// An ambiguous region makes the whole inventory explicitly unavailable.
    pub fn visible_mounts(&self) -> Result<Vec<&MountRecord>, MountTableError> {
        if !self.ambiguous.is_empty() {
            return Err(MountTableError::Ambiguous);
        }
        Ok(self.visible.iter().map(|i| &self.records[*i]).collect())
    }
}
