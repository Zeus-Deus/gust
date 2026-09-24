//! Removable devices — USB sticks, cards and discs (DESIGN §6.13, plan §3).
//!
//! Discovery is a local read of the kernel's own view via `lsblk --json`. It
//! cannot hang on a network and needs no daemon, so the device list works on
//! a machine with no UDisks at all — the mount and eject *actions* are simply
//! unavailable there, and say so.
//!
//! Gust never mounts anything on its own. A device is a row with a visible
//! action, exactly as a remote place is.

use std::path::PathBuf;
use std::process::Command;

/// What kind of thing this is, in words a person uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// A USB stick, SD card or external drive.
    Removable,
    /// An optical drive. May be empty, which is a fact worth stating.
    Disc,
}

/// A volume worth offering the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Kernel name, e.g. `sdb1`.
    pub name: String,
    /// Device node, e.g. `/dev/sdb1`.
    pub path: String,
    pub kind: DeviceKind,
    /// Filesystem label when it has one; otherwise the size is the name.
    pub label: Option<String>,
    pub size: String,
    /// Filesystem type, when the kernel could identify one. `None` on an
    /// empty optical drive or an unformatted stick.
    pub fstype: Option<String>,
    /// Where it is mounted, if it is.
    pub mount_point: Option<PathBuf>,
}

impl Device {
    /// What the sidebar shows. A label if there is one, else something
    /// honest: "32 GB drive", never a bare device node.
    pub fn display(&self) -> String {
        if let Some(l) = &self.label {
            if !l.trim().is_empty() {
                return l.clone();
            }
        }
        match self.kind {
            DeviceKind::Disc => "Disc".to_string(),
            DeviceKind::Removable => format!("{} drive", self.size),
        }
    }

    /// The state in words, for the row's second line. Colour alone is not
    /// enough (plan §2 mockup F).
    pub fn state_text(&self) -> String {
        if self.is_empty_drive() {
            return "nothing in the drive".into();
        }
        match &self.mount_point {
            Some(_) => "connected".into(),
            None => "not connected".into(),
        }
    }

    /// An optical drive with no disc in it, or a device with no filesystem.
    pub fn is_empty_drive(&self) -> bool {
        self.fstype.is_none() && self.mount_point.is_none()
    }

    pub fn is_mounted(&self) -> bool {
        self.mount_point.is_some()
    }
}

/// One row of `lsblk --json` output, before Gust decides what to do with it.
#[derive(Debug, Clone)]
pub struct RawBlock {
    pub name: String,
    pub path: String,
    pub size: String,
    pub removable: bool,
    pub hotplug: bool,
    pub kind: String,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub mount_point: Option<String>,
}

/// Should this block device be offered to the user?
///
/// Removable or hotplug, and not something that is really part of the
/// machine. The exclusions matter as much as the inclusions: showing swap or
/// the root disk as an ejectable device is how somebody unmounts their own
/// system.
pub fn is_offerable(b: &RawBlock) -> bool {
    // A loop device is an image file, not a device. Showing it invites
    // "ejecting" a file, which means nothing.
    if b.kind == "loop" {
        return false;
    }
    // Device-mapper and LVM internals are plumbing, not places.
    if b.name.starts_with("dm-") || b.kind == "lvm" {
        return false;
    }
    // zram and swap are not storage the user browses.
    if b.name.starts_with("zram") || b.fstype.as_deref() == Some("swap") {
        return false;
    }
    if b.kind == "rom" {
        return true;
    }
    b.removable || b.hotplug
}

/// Turn an offerable block device into a Device.
pub fn to_device(b: &RawBlock) -> Device {
    Device {
        name: b.name.clone(),
        path: b.path.clone(),
        kind: if b.kind == "rom" {
            DeviceKind::Disc
        } else {
            DeviceKind::Removable
        },
        label: b.label.clone(),
        size: b.size.clone(),
        fstype: b.fstype.clone(),
        mount_point: b.mount_point.as_ref().map(PathBuf::from),
    }
}

/// Parse `lsblk --json` output into raw rows, flattening children.
///
/// Written against the real shape rather than a schema: `lsblk` nests
/// partitions under their disk, and a partition is what a user mounts.
pub fn parse_lsblk(json: &str) -> Vec<RawBlock> {
    let mut out = Vec::new();
    // A tiny scanner rather than a JSON dependency in this crate: the shape
    // is fixed and every value is a string, a bool or null.
    for chunk in json.split('{').skip(1) {
        let obj = chunk.split('}').next().unwrap_or("");
        let Some(name) = json_str(obj, "name") else {
            continue;
        };
        out.push(RawBlock {
            name,
            path: json_str(obj, "path").unwrap_or_default(),
            size: json_str(obj, "size").unwrap_or_default(),
            removable: json_bool(obj, "rm"),
            hotplug: json_bool(obj, "hotplug"),
            kind: json_str(obj, "type").unwrap_or_default(),
            fstype: json_str(obj, "fstype"),
            label: json_str(obj, "label"),
            mount_point: json_str(obj, "mountpoint"),
        });
    }
    out
}

fn json_str(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = obj.find(&needle)?;
    let after = obj[at + needle.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    if after.starts_with("null") {
        return None;
    }
    let rest = after.strip_prefix('"')?;
    let mut value = String::new();
    let mut escaped = false;
    for c in rest.chars() {
        if escaped {
            value.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return if value.is_empty() { None } else { Some(value) };
        } else {
            value.push(c);
        }
    }
    None
}

fn json_bool(obj: &str, key: &str) -> bool {
    let needle = format!("\"{key}\"");
    let Some(at) = obj.find(&needle) else {
        return false;
    };
    obj[at + needle.len()..]
        .trim_start()
        .strip_prefix(':')
        .map(|r| r.trim_start().starts_with("true"))
        .unwrap_or(false)
}

/// Ask the kernel what block devices exist.
///
/// A local read: this cannot hang on a network and needs no daemon.
pub fn list_devices() -> Vec<Device> {
    let out = Command::new("lsblk")
        .args([
            "--json",
            "-o",
            "NAME,PATH,SIZE,RM,HOTPLUG,TYPE,FSTYPE,LABEL,MOUNTPOINT",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    parse_lsblk(&String::from_utf8_lossy(&out.stdout))
        .iter()
        .filter(|b| is_offerable(b))
        .map(to_device)
        .collect()
}

/// Why a mount or eject could not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// UDisks is not installed. Discovery still works; the actions do not.
    NoUdisks,
    /// A transfer is running on this device.
    Busy(String),
    Refused(String),
}

impl DeviceError {
    pub fn message(&self) -> String {
        match self {
            DeviceError::NoUdisks => {
                "Mounting needs udisks2, which is not installed. The device is \
                 listed, but Gust cannot mount it."
                    .into()
            }
            DeviceError::Busy(what) => {
                format!("{what} is still running. Ejecting now would interrupt it.")
            }
            DeviceError::Refused(why) => why.clone(),
        }
    }
}

/// Is the tool that performs mounts available?
pub fn udisks_available() -> bool {
    Command::new("udisksctl")
        .arg("help")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The argv for mounting a device. Separated from running it so the safety
/// properties can be tested without touching real hardware.
///
/// No shell: the device node is a separate argument, so a device whose name
/// contains shell metacharacters is still just a device node.
pub fn mount_args(device_path: &str) -> Vec<String> {
    vec![
        "mount".into(),
        "-b".into(),
        device_path.to_string(),
        "--no-user-interaction".into(),
    ]
}

/// The argv for unmounting.
pub fn unmount_args(device_path: &str) -> Vec<String> {
    vec![
        "unmount".into(),
        "-b".into(),
        device_path.to_string(),
        "--no-user-interaction".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real machine with a USB stick attached, so the parser
    /// is tested against the shape it will actually meet.
    const REAL_LSBLK: &str = r#"{
   "blockdevices": [
      {"name":"sda","path":"/dev/sda","size":"14.4G","rm":true,"hotplug":true,
       "type":"disk","fstype":null,"label":null,"mountpoint":null,
       "children": [
          {"name":"sda1","path":"/dev/sda1","size":"14.4G","rm":true,"hotplug":true,
           "type":"part","fstype":"vfat","label":"KINGSTON","mountpoint":"/run/media/alex/KINGSTON"}
       ]
      },
      {"name":"nvme0n1","path":"/dev/nvme0n1","size":"476.9G","rm":false,"hotplug":false,
       "type":"disk","fstype":null,"label":null,"mountpoint":null,
       "children": [
          {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","size":"1G","rm":false,"hotplug":false,
           "type":"part","fstype":"vfat","label":"BOOT","mountpoint":"/boot"},
          {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","size":"475.9G","rm":false,"hotplug":false,
           "type":"part","fstype":"ext4","label":null,"mountpoint":"/"}
       ]
      },
      {"name":"sr0","path":"/dev/sr0","size":"1024M","rm":true,"hotplug":false,
       "type":"rom","fstype":null,"label":null,"mountpoint":null},
      {"name":"zram0","path":"/dev/zram0","size":"2.8G","rm":false,"hotplug":false,
       "type":"disk","fstype":"swap","label":null,"mountpoint":"[SWAP]"}
   ]
}"#;

    fn parsed() -> Vec<RawBlock> {
        parse_lsblk(REAL_LSBLK)
    }

    #[test]
    fn nested_partitions_are_found() {
        let all = parsed();
        assert!(
            all.iter().any(|b| b.name == "sda1"),
            "a partition nested under its disk must still be seen"
        );
    }

    #[test]
    fn the_usb_partition_is_offered() {
        let all = parsed();
        let sda1 = all.iter().find(|b| b.name == "sda1").unwrap();
        assert!(is_offerable(sda1));
        let d = to_device(sda1);
        assert_eq!(d.display(), "KINGSTON");
        assert_eq!(d.state_text(), "connected");
        assert!(d.is_mounted());
    }

    #[test]
    fn the_system_disk_is_never_offered() {
        // The single most important exclusion: offering "eject" on the root
        // filesystem is how somebody unmounts their own machine.
        let all = parsed();
        for name in ["nvme0n1", "nvme0n1p1", "nvme0n1p2"] {
            let b = all.iter().find(|b| b.name == name).unwrap();
            assert!(!is_offerable(b), "{name} must not be offered as a device");
        }
    }

    #[test]
    fn swap_and_zram_are_not_places() {
        let all = parsed();
        let z = all.iter().find(|b| b.name == "zram0").unwrap();
        assert!(!is_offerable(z));
    }

    #[test]
    fn an_empty_optical_drive_is_listed_and_described() {
        let all = parsed();
        let sr0 = all.iter().find(|b| b.name == "sr0").unwrap();
        assert!(is_offerable(sr0), "an empty drive is a fact worth showing");
        let d = to_device(sr0);
        assert_eq!(d.kind, DeviceKind::Disc);
        assert!(d.is_empty_drive());
        assert_eq!(d.state_text(), "nothing in the drive");
        assert_eq!(d.display(), "Disc");
    }

    #[test]
    fn a_loop_device_is_not_a_device() {
        // An image file mounted through a loop device is a file, not
        // something the user can eject.
        let json = r#"{"blockdevices":[
            {"name":"loop0","path":"/dev/loop0","size":"32M","rm":false,
             "hotplug":false,"type":"loop","fstype":null,"label":null,
             "mountpoint":null}]}"#;
        let all = parse_lsblk(json);
        assert_eq!(all.len(), 1);
        assert!(!is_offerable(&all[0]));
    }

    #[test]
    fn device_mapper_internals_are_hidden() {
        let json = r#"{"blockdevices":[
            {"name":"dm-0","path":"/dev/dm-0","size":"100G","rm":false,
             "hotplug":false,"type":"lvm","fstype":"ext4","label":null,
             "mountpoint":"/home"}]}"#;
        let all = parse_lsblk(json);
        assert!(!is_offerable(&all[0]));
    }

    #[test]
    fn an_unlabelled_stick_still_has_a_readable_name() {
        let json = r#"{"blockdevices":[
            {"name":"sdb1","path":"/dev/sdb1","size":"7.5G","rm":true,
             "hotplug":true,"type":"part","fstype":"vfat","label":null,
             "mountpoint":null}]}"#;
        let d = to_device(&parse_lsblk(json)[0]);
        assert_eq!(d.display(), "7.5G drive");
        assert!(!d.display().contains("/dev/"), "never show a bare node");
        assert_eq!(d.state_text(), "not connected");
    }

    #[test]
    fn nulls_are_absences_not_the_string_null() {
        let json = r#"{"blockdevices":[
            {"name":"sdc1","path":"/dev/sdc1","size":"1G","rm":true,
             "hotplug":true,"type":"part","fstype":null,"label":null,
             "mountpoint":null}]}"#;
        let b = &parse_lsblk(json)[0];
        assert_eq!(b.fstype, None);
        assert_eq!(b.label, None);
        assert_eq!(b.mount_point, None);
    }

    #[test]
    fn no_shell_is_involved_in_mounting() {
        // A device node is passed as its own argument, so a name containing
        // shell metacharacters cannot become a command.
        let args = mount_args("/dev/sdb1; rm -rf ~");
        assert!(args.contains(&"/dev/sdb1; rm -rf ~".to_string()));
        assert!(args.iter().any(|a| a == "--no-user-interaction"));
        // And nothing is ever joined into one string.
        assert_eq!(args.len(), 4);
    }

    #[test]
    fn unmount_is_the_mirror_of_mount() {
        assert_eq!(mount_args("/dev/x")[0], "mount");
        assert_eq!(unmount_args("/dev/x")[0], "unmount");
        assert_eq!(mount_args("/dev/x")[2], unmount_args("/dev/x")[2]);
    }

    #[test]
    fn errors_explain_themselves() {
        assert!(DeviceError::NoUdisks.message().contains("udisks2"));
        let busy = DeviceError::Busy("A copy to KINGSTON".into()).message();
        assert!(busy.contains("KINGSTON"));
        assert!(busy.contains("interrupt"));
    }

    #[test]
    fn listing_devices_never_panics_on_this_machine() {
        // Whatever this machine has, enumeration must be safe and bounded.
        let found = list_devices();
        for d in &found {
            assert!(!d.display().is_empty());
            assert!(!d.state_text().is_empty());
        }
    }
}
