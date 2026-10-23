use super::RawBlock;
use serde::Deserialize;

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

#[derive(Deserialize)]
struct Envelope {
    blockdevices: Vec<Node>,
}
#[derive(Deserialize)]
struct Node {
    name: String,
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    rm: bool,
    #[serde(default)]
    hotplug: bool,
    fstype: Option<String>,
    label: Option<String>,
    mountpoint: Option<String>,
    pkname: Option<String>,
    #[serde(default)]
    children: Vec<Node>,
}

/// Parse the entire inventory or refuse it. Strings are decoded once by serde;
/// filtering, filesystem admission and device actions remain caller concerns.
pub fn parse_lsblk_checked(json: &str) -> Result<Vec<RawBlock>, LsblkParseError> {
    if json.len() > MAX_LSBLK_BYTES {
        return Err(LsblkParseError::Limit);
    }
    let envelope: Envelope = serde_json::from_str(json).map_err(|error| {
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
        .map(|node| (node, 1))
        .collect();
    let mut out = Vec::new();
    while let Some((node, depth)) = stack.pop() {
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
                .map(|child| (child, depth + 1)),
        );
        out.push(RawBlock {
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
        });
    }
    Ok(out)
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}
