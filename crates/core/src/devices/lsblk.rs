use super::RawBlock;
use serde::{
    de::{IgnoredAny, MapAccess, Visitor},
    Deserialize,
};

pub const MAX_LSBLK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LSBLK_ROWS: usize = 65_536;
pub const MAX_LSBLK_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LsblkParseError {
    InvalidJson,
    InvalidShape,
    Limit,
}
impl std::fmt::Display for LsblkParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidJson => "invalid lsblk JSON",
            Self::InvalidShape => "invalid lsblk device structure",
            Self::Limit => "lsblk inventory exceeds supported limits",
        })
    }
}
impl std::error::Error for LsblkParseError {}

// Derived structs also accept positional sequences. Keep their typed field and
// duplicate-key checks, but admit only a JSON map at each inventory boundary.
struct Object<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for MapVisitor<T> {
            type Value = Object<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                T::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Object)
            }
        }
        deserializer.deserialize_map(MapVisitor(std::marker::PhantomData))
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "M: MetadataFields"))]
struct Envelope<M> {
    blockdevices: Vec<Object<Node<M>>>,
}
struct Node<M> {
    name: String,
    path: String,
    kind: String,
    size: String,
    rm: bool,
    hotplug: bool,
    fstype: Option<String>,
    label: Option<String>,
    mountpoint: Option<String>,
    pkname: Option<String>,
    children: Vec<Object<Node<M>>>,
    metadata: M,
}

// Keep one streaming map decoder. A mode consumes only its recognized keys;
// everything else uses IgnoredAny, without buffering arbitrary JSON numbers or
// unknown nested metadata in Serde Content/Value first.
pub(super) trait MetadataFields: Default {
    fn field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error>;
}
#[derive(Default)]
struct IgnoredMetadata;
impl MetadataFields for IgnoredMetadata {
    fn field<'de, A: MapAccess<'de>>(
        &mut self,
        _key: &str,
        _map: &mut A,
    ) -> Result<bool, A::Error> {
        Ok(false)
    }
}

fn read_field<'de, T: Deserialize<'de>, A: MapAccess<'de>>(
    slot: &mut Option<T>,
    name: &'static str,
    map: &mut A,
) -> Result<(), A::Error> {
    if slot.is_some() {
        return Err(serde::de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

impl<'de, M: MetadataFields> Deserialize<'de> for Node<M> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NodeVisitor<M>(std::marker::PhantomData<M>);
        impl<'de, M: MetadataFields> Visitor<'de> for NodeVisitor<M> {
            type Value = Node<M>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON device object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let (mut name, mut path, mut kind) = (None, None, None);
                let (mut size, mut rm, mut hotplug) = (None, None, None);
                // Outer Option records key presence even when its value is null.
                let mut fstype: Option<Option<String>> = None;
                let mut label: Option<Option<String>> = None;
                let mut mountpoint: Option<Option<String>> = None;
                let mut pkname: Option<Option<String>> = None;
                let mut children = None;
                let mut metadata = M::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => read_field(&mut name, "name", &mut map)?,
                        "path" => read_field(&mut path, "path", &mut map)?,
                        "type" => read_field(&mut kind, "type", &mut map)?,
                        "size" => read_field(&mut size, "size", &mut map)?,
                        "rm" => read_field(&mut rm, "rm", &mut map)?,
                        "hotplug" => read_field(&mut hotplug, "hotplug", &mut map)?,
                        "fstype" => read_field(&mut fstype, "fstype", &mut map)?,
                        "label" => read_field(&mut label, "label", &mut map)?,
                        "mountpoint" => read_field(&mut mountpoint, "mountpoint", &mut map)?,
                        "pkname" => read_field(&mut pkname, "pkname", &mut map)?,
                        "children" => read_field(&mut children, "children", &mut map)?,
                        _ => {
                            if !metadata.field(&key, &mut map)? {
                                map.next_value::<IgnoredAny>()?;
                            }
                        }
                    }
                }
                Ok(Node {
                    name: name.ok_or_else(|| serde::de::Error::missing_field("name"))?,
                    path: path.ok_or_else(|| serde::de::Error::missing_field("path"))?,
                    kind: kind.ok_or_else(|| serde::de::Error::missing_field("type"))?,
                    size: size.unwrap_or_default(),
                    rm: rm.unwrap_or(false),
                    hotplug: hotplug.unwrap_or(false),
                    fstype: fstype.flatten(),
                    label: label.flatten(),
                    mountpoint: mountpoint.flatten(),
                    pkname: pkname.flatten(),
                    children: children.unwrap_or_default(),
                    metadata,
                })
            }
        }
        deserializer.deserialize_map(NodeVisitor(std::marker::PhantomData))
    }
}

pub(super) struct InventoryRow<M> {
    pub block: RawBlock,
    pub parent: Option<usize>,
    pub metadata: M,
}

/// Parse the entire inventory or refuse it. Strings are decoded once by serde;
/// filtering, filesystem admission and device actions remain caller concerns.
pub fn parse_lsblk_checked(json: &str) -> Result<Vec<RawBlock>, LsblkParseError> {
    Ok(parse_inventory::<IgnoredMetadata>(json)?
        .into_iter()
        .map(|row| row.block)
        .collect())
}

pub(super) fn parse_inventory<M: MetadataFields>(
    json: &str,
) -> Result<Vec<InventoryRow<M>>, LsblkParseError> {
    if json.len() > MAX_LSBLK_BYTES {
        return Err(LsblkParseError::Limit);
    }
    let Object(envelope): Object<Envelope<M>> = serde_json::from_str(json).map_err(|error| {
        // Serde's recursion guard can fire before the flattened row-depth check.
        // Only its fixed category is exposed, never its input-bearing diagnostic.
        if error.to_string().starts_with("recursion limit exceeded") {
            LsblkParseError::Limit
        } else if error.is_data() {
            LsblkParseError::InvalidShape
        } else {
            LsblkParseError::InvalidJson
        }
    })?;
    let mut stack: Vec<_> = envelope
        .blockdevices
        .into_iter()
        .rev()
        .map(|Object(node)| (node, 1, None))
        .collect();
    let mut out = Vec::new();
    while let Some((node, depth, parent)) = stack.pop() {
        if out.len() == MAX_LSBLK_ROWS || depth > MAX_LSBLK_DEPTH {
            return Err(LsblkParseError::Limit);
        }
        if node.name.is_empty() || node.path.is_empty() || node.kind.is_empty() {
            return Err(LsblkParseError::InvalidShape);
        }
        stack.extend(
            node.children
                .into_iter()
                .rev()
                .map(|Object(child)| (child, depth + 1, Some(out.len()))),
        );
        out.push(InventoryRow {
            parent,
            metadata: node.metadata,
            block: RawBlock {
                name: node.name,
                path: node.path,
                size: node.size,
                removable: node.rm,
                hotplug: node.hotplug,
                kind: node.kind,
                fstype: nonempty(node.fstype),
                label: nonempty(node.label),
                mount_point: nonempty(node.mountpoint),
                parent: nonempty(node.pkname),
            },
        });
    }
    Ok(out)
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}
