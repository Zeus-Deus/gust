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

mod lsblk;
pub use lsblk::{
    parse_lsblk_checked, LsblkParseError, MAX_LSBLK_BYTES, MAX_LSBLK_DEPTH, MAX_LSBLK_ROWS,
};

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
    /// Where it is mounted, if it is. For an encrypted drive, where its
    /// unlocked contents are mounted.
    pub mount_point: Option<PathBuf>,
    /// An encrypted (LUKS) drive: it must be unlocked with its passphrase
    /// before there is anything to open.
    pub encrypted: bool,
    /// While an encrypted drive is unlocked, the device its contents are
    /// read through (e.g. `/dev/dm-0`). `None` while locked.
    pub cleartext: Option<String>,
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
            return match self.kind {
                DeviceKind::Disc => "nothing in the drive".into(),
                // A stick is always "in the drive"; what is missing is a
                // filesystem the kernel can name (blank or wiped).
                DeviceKind::Removable => "no readable filesystem".into(),
            };
        }
        if self.is_locked() {
            return "locked".into();
        }
        match &self.mount_point {
            Some(_) => "connected".into(),
            None if self.encrypted => "unlocked".into(),
            None => "not connected".into(),
        }
    }

    /// An encrypted drive whose passphrase has not been given yet.
    pub fn is_locked(&self) -> bool {
        self.encrypted && self.cleartext.is_none()
    }

    /// The device a mount or unmount acts on: the unlocked contents of an
    /// encrypted drive, else the device itself.
    pub fn mount_target(&self) -> &str {
        self.cleartext.as_deref().unwrap_or(&self.path)
    }

    /// An optical drive with no disc in it, or a device with no filesystem.
    pub fn is_empty_drive(&self) -> bool {
        self.fstype.is_none() && self.mount_point.is_none()
    }

    /// Is there anything here that mounting could open?
    pub fn can_mount(&self) -> bool {
        !self.is_empty_drive()
    }

    pub fn is_mounted(&self) -> bool {
        self.mount_point.is_some()
    }
}

/// udev keeps these folders of links in step with the kernel's block
/// devices. Watching them (not `/dev`, where every new terminal is an
/// event) tells Gust when a device came, went or got a filesystem.
pub const DEVICE_WATCH_DIRS: [&str; 3] = [
    "/dev/disk/by-diskseq",
    "/dev/disk/by-label",
    "/dev/disk/by-uuid",
];

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
    /// The kernel name of the device this one sits on (`lsblk` PKNAME): a
    /// partition's disk, an unlocked volume's encrypted drive.
    pub parent: Option<String>,
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
        encrypted: b.fstype.as_deref() == Some("crypto_LUKS"),
        cleartext: None,
    }
}

/// The devices worth offering, with each encrypted drive joined to its
/// unlocked volume (a `crypt` child), whose mount point it takes. The
/// volume itself is never a row of its own: it is the same drive.
pub fn offerable_devices(raw: &[RawBlock]) -> Vec<Device> {
    raw.iter()
        .filter(|b| is_offerable(b))
        .map(|b| {
            let mut d = to_device(b);
            if d.encrypted {
                if let Some(c) = raw
                    .iter()
                    .find(|c| c.kind == "crypt" && c.parent.as_deref() == Some(b.name.as_str()))
                {
                    d.cleartext = Some(c.path.clone());
                    d.mount_point = c.mount_point.as_ref().map(PathBuf::from);
                }
            }
            d
        })
        .collect()
}

/// Compatibility adapter; errors become an empty list. Fallible refreshes must
/// use `parse_lsblk_checked` to distinguish refusal from an empty inventory.
pub fn parse_lsblk(json: &str) -> Vec<RawBlock> {
    parse_lsblk_checked(json).unwrap_or_default()
}

/// The devices the sidebar shows: only ones a click can do something with.
/// An empty optical drive or a stick with no filesystem the kernel can
/// name has nothing to open, so it gets no row (Flea 0.3.7: "drives with
/// no filesystem no longer leave an unusable sidebar row").
pub fn sidebar_devices(all: Vec<Device>) -> Vec<Device> {
    all.into_iter().filter(|d| !d.is_empty_drive()).collect()
}

/// Ask the kernel what block devices exist.
///
/// A local read: this cannot hang on a network and needs no daemon.
pub fn list_devices() -> Vec<Device> {
    let out = Command::new("lsblk")
        .args([
            "--json",
            "-o",
            "NAME,PATH,PKNAME,SIZE,RM,HOTPLUG,TYPE,FSTYPE,LABEL,MOUNTPOINT",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    offerable_devices(&parse_lsblk(&String::from_utf8_lossy(&out.stdout)))
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

/// The argv for unlocking an encrypted drive. The passphrase is NOT here:
/// it is written to the child's stdin, which udisksctl reads as the key
/// (`--key-file /dev/stdin`), so it never appears in argv, the environment
/// or a file (AGENTS.md: no secret in argv, env, logs or config).
pub fn unlock_args(device_path: &str) -> Vec<String> {
    vec![
        "unlock".into(),
        "-b".into(),
        device_path.to_string(),
        "--no-user-interaction".into(),
        "--key-file".into(),
        "/dev/stdin".into(),
    ]
}

/// The argv for locking an encrypted drive again (after its contents are
/// unmounted), so it is safe to unplug.
pub fn lock_args(device_path: &str) -> Vec<String> {
    vec![
        "lock".into(),
        "-b".into(),
        device_path.to_string(),
        "--no-user-interaction".into(),
    ]
}

/// udisksctl's last error line, in words a person can act on.
pub fn unlock_failure(stderr: &str) -> String {
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.contains("controlling terminal"))
        .unwrap_or("")
        .trim();
    if last.contains("Incorrect passphrase") || last.contains("No key available") {
        "That passphrase did not unlock the drive. Nothing was changed.".into()
    } else if last.contains("NotAuthorized") {
        "This account is not allowed to unlock drives (polkit said no).".into()
    } else if last.contains("AlreadyUnlocked") || last.contains("already unlocked") {
        "That drive is already unlocked.".into()
    } else if last.is_empty() {
        "Could not unlock the drive; udisks did not say why.".into()
    } else {
        format!("Could not unlock the drive: {last}")
    }
}

/// udisksctl's mount error, in words a person can act on.
pub fn mount_failure(stderr: &str) -> String {
    let last = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.contains("controlling terminal"))
        .unwrap_or("")
        .trim();
    if last.contains("not a mountable filesystem") {
        "There is no filesystem on it that Linux can open. If it is encrypted, it is \
         unlocked now; eject it to lock it again."
            .into()
    } else if last.contains("NotAuthorized") {
        "This account is not allowed to mount drives (polkit said no).".into()
    } else if last.is_empty() {
        "Could not mount that device; udisks did not say why.".into()
    } else {
        format!("Could not mount that device: {last}")
    }
}

/// Where udisksctl said it unlocked to: "Unlocked /dev/sda as /dev/dm-0."
pub fn unlocked_as(stdout: &str) -> Option<String> {
    let (_, after) = stdout.rsplit_once(" as ")?;
    let dev = after.trim().trim_end_matches('.');
    dev.starts_with("/dev/").then(|| dev.to_string())
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
    fn a_stick_with_no_filesystem_is_not_called_empty() {
        // A removable disk the kernel sees but whose filesystem it cannot
        // name (blank, or wiped) has something in the drive; saying
        // "nothing in the drive" is false. Found with a real hot-plugged
        // SCSI disk in the VM.
        let json = r#"{"blockdevices":[
            {"name":"sda","path":"/dev/sda","size":"64M","rm":true,
             "hotplug":false,"type":"disk","fstype":null,"label":null,
             "mountpoint":null}]}"#;
        let b = &parse_lsblk(json)[0];
        assert!(is_offerable(b));
        let d = to_device(b);
        assert!(!d.can_mount(), "nothing to mount");
        assert_eq!(d.state_text(), "no readable filesystem");
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

    /// Captured in the VM: a USB drive formatted as LUKS, unlocked by
    /// udisksctl. The volume is a child of type "crypt".
    const LUKS_UNLOCKED: &str = r#"{"blockdevices":[{"name":"sda","path":"/dev/sda","pkname":null,"size":"64M","rm":true,"hotplug":false,"type":"disk","fstype":"crypto_LUKS","label":"SECRETS","mountpoint":null,"children":[{"name":"SECRETS","path":"/dev/mapper/SECRETS","pkname":"sda","size":"48M","rm":false,"hotplug":false,"type":"crypt","fstype":"ext4","label":"inner","mountpoint":"/run/media/alex/inner"}]}]}"#;
    const LUKS_LOCKED: &str = r#"{"blockdevices":[{"name":"sda","path":"/dev/sda","pkname":null,"size":"64M","rm":true,"hotplug":false,"type":"disk","fstype":"crypto_LUKS","label":"SECRETS","mountpoint":null}]}"#;

    #[test]
    fn a_locked_drive_is_one_row_that_says_locked() {
        let devs = offerable_devices(&parse_lsblk(LUKS_LOCKED));
        assert_eq!(devs.len(), 1);
        let d = &devs[0];
        assert!(d.encrypted && d.is_locked());
        assert_eq!(d.state_text(), "locked");
        assert_eq!(d.display(), "SECRETS");
        // Something to do with it: it is not an empty drive.
        assert!(!d.is_empty_drive() && d.can_mount());
        assert_eq!(sidebar_devices(devs).len(), 1);
    }

    #[test]
    fn an_unlocked_drive_is_still_one_row_and_mounts_its_volume() {
        let devs = offerable_devices(&parse_lsblk(LUKS_UNLOCKED));
        // The crypt volume is never a second row.
        assert_eq!(devs.len(), 1, "{devs:?}");
        let d = &devs[0];
        assert!(!d.is_locked());
        assert_eq!(d.cleartext.as_deref(), Some("/dev/mapper/SECRETS"));
        assert_eq!(d.mount_target(), "/dev/mapper/SECRETS");
        assert_eq!(
            d.mount_point.as_deref(),
            Some(std::path::Path::new("/run/media/alex/inner"))
        );
        assert_eq!(d.state_text(), "connected");
        // Unlocked but not mounted says so.
        let mut u = d.clone();
        u.mount_point = None;
        assert_eq!(u.state_text(), "unlocked");
    }

    #[test]
    fn a_plain_drive_mounts_itself() {
        let devs = offerable_devices(&parse_lsblk(REAL_LSBLK));
        let usb = devs.iter().find(|d| d.name == "sda1").unwrap();
        assert!(!usb.encrypted && !usb.is_locked());
        assert_eq!(usb.mount_target(), "/dev/sda1");
    }

    #[test]
    fn the_passphrase_is_never_an_argument() {
        let a = unlock_args("/dev/sda");
        assert_eq!(a[0], "unlock");
        assert!(a.iter().any(|x| x == "--no-user-interaction"));
        // The key comes from stdin; nothing else in argv could carry it.
        let i = a.iter().position(|x| x == "--key-file").unwrap();
        assert_eq!(a[i + 1], "/dev/stdin");
        assert_eq!(a.len(), 6);
        assert_eq!(lock_args("/dev/sda")[0], "lock");
        assert_eq!(lock_args("/dev/sda")[2], "/dev/sda");
    }

    #[test]
    fn unlock_failures_are_in_plain_words() {
        // Real udisksctl stderr from the VM.
        let wrong = "Error unlocking /dev/sda: GDBus.Error:org.freedesktop.UDisks2.Error.Failed: Error unlocking /dev/sda: Failed to activate device: Incorrect passphrase.";
        assert!(unlock_failure(wrong).contains("did not unlock"));
        let noauth = "** (udisksctl unlock:1): WARNING **: Error opening current controlling terminal /dev/tty\nError unlocking /dev/sda: GDBus.Error:org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain: Not authorized to perform operation";
        assert!(unlock_failure(noauth).contains("not allowed"));
        assert!(unlock_failure("").contains("did not say"));
        assert_eq!(
            unlocked_as("Unlocked /dev/sda as /dev/dm-0.\n").as_deref(),
            Some("/dev/dm-0")
        );
        assert_eq!(unlocked_as("something else"), None);
    }

    #[test]
    fn mount_failures_are_in_plain_words() {
        // Real stderr from the VM: an unlocked LUKS drive with nothing inside.
        let raw = "Error mounting /dev/dm-0: GDBus.Error:org.freedesktop.UDisks2.Error.Failed: Object /org/freedesktop/UDisks2/block_devices/dm_2d0 is not a mountable filesystem.";
        let m = mount_failure(raw);
        assert!(m.contains("no filesystem") && !m.contains("GDBus"), "{m}");
        assert!(mount_failure("").contains("did not say"));
    }

    #[test]
    fn a_drive_with_nothing_to_open_gets_no_sidebar_row() {
        let mk = |name: &str, fstype: Option<&str>, mp: Option<&str>, kind| Device {
            name: name.into(),
            path: format!("/dev/{name}"),
            kind,
            label: None,
            size: "8G".into(),
            fstype: fstype.map(str::to_string),
            mount_point: mp.map(PathBuf::from),
            encrypted: false,
            cleartext: None,
        };
        let shown = sidebar_devices(vec![
            mk("sr0", None, None, DeviceKind::Disc),
            mk("sdb", None, None, DeviceKind::Removable),
            mk("sdc1", Some("vfat"), None, DeviceKind::Removable),
            mk(
                "sdd1",
                Some("exfat"),
                Some("/run/media/u/X"),
                DeviceKind::Removable,
            ),
        ]);
        let names: Vec<&str> = shown.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["sdc1", "sdd1"]);
    }
}
