//! Pure kernel-instance observations. Capture provenance belongs to the caller;
//! neither parsing nor comparison performs I/O or makes a service call atomic.
use super::{
    is_offerable,
    lsblk::{parse_inventory, MetadataFields},
    to_device, Device, LsblkParseError, RawBlock,
};
use serde::de::MapAccess;
use std::{collections::HashMap, num::NonZeroU64, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceNumber {
    pub major: u32,
    pub minor: u32,
}

/// Asserted by trusted capture code, never by a field in the JSON document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvidenceOrigin {
    LsblkKernelSysfsV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEvidenceUnavailable {
    MissingSourceInstance,
    MissingCleartextInstance,
    AmbiguousInstance,
    AmbiguousRelationship,
    UnsupportedOrigin,
    UnverifiedLocator,
}
impl std::fmt::Display for DeviceEvidenceUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingSourceInstance => "source kernel instance is unavailable",
            Self::MissingCleartextInstance => "cleartext kernel instance is unavailable",
            Self::AmbiguousInstance => "kernel instance is ambiguous",
            Self::AmbiguousRelationship => "cleartext relationship is ambiguous",
            Self::UnsupportedOrigin => "device evidence capture profile is unavailable",
            Self::UnverifiedLocator => "device locator could not be verified",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KernelBlockInstance {
    number: DeviceNumber,
    disk_sequence: NonZeroU64,
}
impl KernelBlockInstance {
    pub fn number(&self) -> DeviceNumber {
        self.number
    }
    pub fn disk_sequence(&self) -> NonZeroU64 {
        self.disk_sequence
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleartextBinding {
    instance: KernelBlockInstance,
    parent: KernelBlockInstance,
}
impl CleartextBinding {
    pub fn instance(&self) -> KernelBlockInstance {
        self.instance
    }
    pub fn parent(&self) -> KernelBlockInstance {
        self.parent
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceBinding {
    source: KernelBlockInstance,
    cleartext: Option<CleartextBinding>,
    origin: DeviceEvidenceOrigin,
}
impl DeviceBinding {
    pub fn source(&self) -> KernelBlockInstance {
        self.source
    }
    pub fn cleartext(&self) -> Option<CleartextBinding> {
        self.cleartext
    }
    pub fn origin(&self) -> DeviceEvidenceOrigin {
        self.origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvidence {
    Available(DeviceBinding),
    Unavailable(DeviceEvidenceUnavailable),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceObservation {
    // Keep the fresh-observation transition value small without boxing the
    // public Same variant or exposing mutable display fields to callers.
    device: Box<Device>,
    evidence: DeviceEvidence,
    // Kept even when a child has insufficient evidence. Revalidation never uses
    // the display path to find a replacement for the retained source number.
    source_number: Option<DeviceNumber>,
}
impl DeviceObservation {
    pub fn device(&self) -> &Device {
        &self.device
    }
    pub fn evidence(&self) -> &DeviceEvidence {
        &self.evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRevalidation {
    Same(DeviceObservation),
    Changed,
    Unavailable(DeviceEvidenceUnavailable),
}

#[derive(Default)]
struct KernelMetadata {
    kname: Option<String>,
    number: Option<String>,
    disk_sequence: Option<u64>,
    seen: u8,
}
impl MetadataFields for KernelMetadata {
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        let (bit, name) = match key {
            "kname" => (1, "kname"),
            "maj:min" => (2, "maj:min"),
            "disk-seq" => (4, "disk-seq"),
            _ => return Ok(false),
        };
        if self.seen & bit != 0 {
            return Err(serde::de::Error::duplicate_field(name));
        }
        self.seen |= bit;
        match name {
            "kname" => self.kname = map.next_value()?,
            "maj:min" => self.number = map.next_value()?,
            _ => self.disk_sequence = map.next_value()?,
        }
        Ok(true)
    }
}
struct Row {
    block: RawBlock,
    parent: Option<usize>,
    kname: Option<String>,
    number: Option<DeviceNumber>,
    instance: Option<KernelBlockInstance>,
}

/// The entire report is validated before offerable rows are projected. `None`
/// origin or missing evidence preserves rows, but cannot produce Available.
/// Only the exact capture profile in DESIGN §6.13.3 may assert the origin.
pub fn parse_lsblk_evidence_checked(
    json: &str,
    origin: Option<DeviceEvidenceOrigin>,
) -> Result<Vec<DeviceObservation>, LsblkParseError> {
    let rows = parse_inventory::<KernelMetadata>(json)?
        .into_iter()
        .map(|row| {
            let number = row
                .metadata
                .number
                .as_deref()
                .map(parse_number)
                .transpose()?;
            let instance = number
                .zip(row.metadata.disk_sequence.and_then(NonZeroU64::new))
                .map(|(number, disk_sequence)| KernelBlockInstance {
                    number,
                    disk_sequence,
                });
            Ok(Row {
                block: row.block,
                parent: row.parent,
                kname: row.metadata.kname.filter(|s| !s.is_empty()),
                number,
                instance,
            })
        })
        .collect::<Result<Vec<_>, LsblkParseError>>()?;
    let index = Index::new(&rows);
    Ok(rows
        .iter()
        .enumerate()
        .filter(|(_, row)| is_offerable(&row.block))
        .map(|(i, row)| {
            let mut device = to_device(&row.block);
            let child = if device.encrypted {
                index.child(i)
            } else {
                Ok(None)
            };
            if let Ok(Some(child)) = child {
                device.cleartext = Some(rows[child].block.path.clone());
                device.mount_point = rows[child].block.mount_point.as_ref().map(PathBuf::from);
            }
            let evidence = index
                .binding(i, child, origin)
                .map(DeviceEvidence::Available)
                .unwrap_or_else(DeviceEvidence::Unavailable);
            DeviceObservation {
                device: Box::new(device),
                evidence,
                source_number: row.number,
            }
        })
        .collect())
}

fn parse_number(text: &str) -> Result<DeviceNumber, LsblkParseError> {
    fn component(s: &str) -> Option<u32> {
        if s.is_empty()
            || (s.len() > 1 && s.starts_with('0'))
            || !s.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        s.parse().ok()
    }
    let (major, minor) = text.split_once(':').ok_or(LsblkParseError::InvalidShape)?;
    Ok(DeviceNumber {
        major: component(major).ok_or(LsblkParseError::InvalidShape)?,
        minor: component(minor).ok_or(LsblkParseError::InvalidShape)?,
    })
}

struct Index<'a> {
    rows: &'a [Row],
    numbers: HashMap<DeviceNumber, usize>,
    names: HashMap<&'a str, usize>,
    paths: HashMap<&'a str, usize>,
    crypt_edges: Vec<Vec<usize>>,
    crypt_names: HashMap<&'a str, Vec<usize>>,
}
impl<'a> Index<'a> {
    fn new(rows: &'a [Row]) -> Self {
        let mut out = Self {
            rows,
            numbers: HashMap::new(),
            names: HashMap::new(),
            paths: HashMap::new(),
            crypt_edges: vec![Vec::new(); rows.len()],
            crypt_names: HashMap::new(),
        };
        for (i, row) in rows.iter().enumerate() {
            if let Some(number) = row.number {
                *out.numbers.entry(number).or_default() += 1;
            }
            if let Some(name) = row.kname.as_deref() {
                *out.names.entry(name).or_default() += 1;
            }
            *out.paths.entry(&row.block.path).or_default() += 1;
            if row.block.kind == "crypt" {
                if let Some(parent) = row.parent {
                    out.crypt_edges[parent].push(i);
                }
                if let Some(name) = row.block.parent.as_deref() {
                    out.crypt_names.entry(name).or_default().push(i);
                }
            }
        }
        out
    }
    fn instance(
        &self,
        i: usize,
        missing: DeviceEvidenceUnavailable,
    ) -> Result<KernelBlockInstance, DeviceEvidenceUnavailable> {
        let row = &self.rows[i];
        // lsblk can inherit the containing gendisk's sequence for a partition.
        // That token survives partition removal/recreation and cannot prove its
        // own lifetime, even when all reported fields are positive and unique.
        if row.block.kind == "part" {
            return Err(missing);
        }
        let name = row.kname.as_deref().ok_or(missing)?;
        let instance = row.instance.ok_or(missing)?;
        if self.names.get(name) != Some(&1)
            || self.numbers.get(&instance.number) != Some(&1)
            || self.paths.get(row.block.path.as_str()) != Some(&1)
        {
            return Err(DeviceEvidenceUnavailable::AmbiguousInstance);
        }
        Ok(instance)
    }
    fn child(&self, i: usize) -> Result<Option<usize>, DeviceEvidenceUnavailable> {
        use DeviceEvidenceUnavailable::*;
        let name = self.rows[i].kname.as_deref().ok_or(MissingSourceInstance)?;
        // Prevent repeated/ambiguous KNAME rows from turning this into an
        // unbounded cross product over a many-to-many lsblk tree.
        if self.names.get(name) != Some(&1) {
            return Err(AmbiguousInstance);
        }
        let mut candidates = self.crypt_edges[i].clone();
        if let Some(named) = self.crypt_names.get(name) {
            candidates.extend(named);
        }
        candidates.sort_unstable();
        candidates.dedup();
        match candidates.as_slice() {
            [] => Ok(None),
            [child]
                if self.rows[*child].parent == Some(i)
                    && self.rows[*child].block.parent.as_deref() == Some(name) =>
            {
                Ok(Some(*child))
            }
            _ => Err(AmbiguousRelationship),
        }
    }
    fn binding(
        &self,
        i: usize,
        child: Result<Option<usize>, DeviceEvidenceUnavailable>,
        origin: Option<DeviceEvidenceOrigin>,
    ) -> Result<DeviceBinding, DeviceEvidenceUnavailable> {
        use DeviceEvidenceUnavailable::*;
        let origin = origin.ok_or(UnsupportedOrigin)?;
        let source = self.instance(i, MissingSourceInstance)?;
        let cleartext = child?
            .map(|child| {
                self.instance(child, MissingCleartextInstance)
                    .map(|instance| CleartextBinding {
                        instance,
                        parent: source,
                    })
            })
            .transpose()?;
        Ok(DeviceBinding {
            source,
            cleartext,
            origin,
        })
    }
}

fn binding(observation: &DeviceObservation) -> Result<&DeviceBinding, DeviceEvidenceUnavailable> {
    match &observation.evidence {
        DeviceEvidence::Available(binding) => Ok(binding),
        DeviceEvidence::Unavailable(reason) => Err(*reason),
    }
}
fn current_source<'a>(
    expected: &DeviceBinding,
    current: &'a [DeviceObservation],
) -> Result<&'a DeviceObservation, DeviceEvidenceUnavailable> {
    let mut matches = current
        .iter()
        .filter(|row| row.source_number == Some(expected.source.number));
    let found = matches
        .next()
        .ok_or(DeviceEvidenceUnavailable::MissingSourceInstance)?;
    if matches.next().is_some() {
        return Err(DeviceEvidenceUnavailable::AmbiguousInstance);
    }
    Ok(found)
}

/// Compare both retained kernel instances/relationship, never display locators.
/// Same carries fresh locators; it does not freeze hotplug or service state.
pub fn revalidate_device(
    expected: &DeviceObservation,
    current: &[DeviceObservation],
) -> DeviceRevalidation {
    let compare = || {
        let old = binding(expected)?;
        let fresh = current_source(old, current)?;
        let new = binding(fresh)?;
        Ok(
            if old == new && expected.device.encrypted == fresh.device.encrypted {
                DeviceRevalidation::Same(fresh.clone())
            } else {
                DeviceRevalidation::Changed
            },
        )
    };
    compare().unwrap_or_else(DeviceRevalidation::Unavailable)
}

/// Caller must have an actually successful accepted unlock of this retained
/// selection. This is the sole intentional no-child -> child transition; the
/// successful returned locator must equal the captured child's PATH exactly.
/// Revalidate this fresh binding again before mount/navigation. An unproved
/// alias refuses without I/O, canonicalization or an invented /dev/KNAME path.
pub fn bind_cleartext_after_unlock(
    expected: &DeviceObservation,
    current: &[DeviceObservation],
    returned_locator: &str,
) -> DeviceRevalidation {
    let compare = || {
        let old = binding(expected)?;
        if !expected.device.encrypted || old.cleartext.is_some() {
            return Ok(DeviceRevalidation::Changed);
        }
        let fresh = current_source(old, current)?;
        let new = binding(fresh)?;
        if old.source != new.source || old.origin != new.origin || !fresh.device.encrypted {
            return Ok(DeviceRevalidation::Changed);
        }
        if new.cleartext.is_none() {
            return Err(DeviceEvidenceUnavailable::MissingCleartextInstance);
        }
        if returned_locator.is_empty()
            || fresh.device.cleartext.as_deref() != Some(returned_locator)
        {
            return Err(DeviceEvidenceUnavailable::UnverifiedLocator);
        }
        Ok(DeviceRevalidation::Same(fresh.clone()))
    };
    compare().unwrap_or_else(DeviceRevalidation::Unavailable)
}
