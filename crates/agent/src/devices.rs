//! Every place an agent can send files to, behind one enum. A new kind of
//! device is one more variant; the compiler then lists every match to extend.
//!
//! Discovery reads local state only (sysfs, udev, `lsblk`, the Tailscale
//! daemon). A device is contacted only by a call that names it.

use gust_core::devices::{Device, DeviceKind};
use gust_core::lister::CancelToken;
use gust_remote::helper::CallContext;
use gust_remote::phones::{self, Phone, PhoneKind, PhoneState};
use gust_remote::sharing::SendTarget;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug)]
pub enum Endpoint {
    /// An iPhone, iPad or Android phone on USB.
    Phone { phone: Phone, name: String },
    /// A USB stick, SD card or external drive.
    Drive(Device),
    /// One of this account's devices on the tailnet.
    Taildrop(SendTarget),
}

/// A request that could not be carried out, said so the agent can act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub action: String,
    /// What did happen, for a call that partly succeeded.
    pub details: Option<Value>,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            action: action.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl Endpoint {
    pub fn id(&self) -> String {
        match self {
            Endpoint::Phone { phone, .. } => phone.id(),
            Endpoint::Drive(d) => format!("drive:{}", d.name),
            Endpoint::Taildrop(t) => format!("taildrop:{}", t.address),
        }
    }

    fn what(&self) -> &'static str {
        match self {
            Endpoint::Phone { phone, .. } if phone.kind == PhoneKind::Apple => "Apple device",
            Endpoint::Phone { .. } => "phone",
            Endpoint::Drive(_) => "drive",
            Endpoint::Taildrop(_) => "device",
        }
    }

    /// A folder must be chosen on phones: they have no writable top level.
    pub fn needs_folder(&self) -> bool {
        matches!(self, Endpoint::Phone { .. })
    }

    /// Where a sent file ends up, and what the person must still do.
    pub fn lands_in(&self) -> &'static str {
        match self {
            Endpoint::Phone { phone, .. } => match phone.kind {
                PhoneKind::Apple => "The shared folder of one app on the iPhone or iPad (choose it with list_device_folders; folders are named by app). Open the file from that app. It is not added to the Photos library: use the taildrop entry for photos and videos meant for Photos.",
                _ => "The chosen folder on the phone's storage. Pictures and videos placed in DCIM or Pictures appear in the phone's gallery.",
            },
            Endpoint::Drive(_) => "The chosen folder on the drive, or its top level.",
            Endpoint::Taildrop(_) => "The device's Taildrop inbox. On an iPhone or iPad the person taps the Tailscale notification and finds the file in Files › Tailscale; Share › Save Image or Save Video then puts it in Photos. Nothing sent from a computer enters Photos by itself.",
        }
    }

    pub fn describe(&self) -> Value {
        let (name, kind, transport, state, detail) = match self {
            Endpoint::Phone { phone, name } => {
                let (state, detail) = if phones::backend_installed(phone.kind) {
                    ("ready", match phone.kind {
                        PhoneKind::Apple => "Plugged in. The first send may ask the person to unlock it and tap Trust.".to_string(),
                        _ => "Plugged in. The phone must be unlocked and set to File transfer in its USB notification.".to_string(),
                    })
                } else {
                    (
                        "needs_setup",
                        format!(
                            "Gust needs the {} package to reach it.",
                            phone.kind.package()
                        ),
                    )
                };
                let kind = match phone.kind {
                    PhoneKind::Apple => "apple",
                    _ => "android",
                };
                (name.clone(), kind, "usb", state, detail)
            }
            Endpoint::Drive(d) => {
                let (state, detail) = if d.encrypted && d.cleartext.is_none() {
                    ("needs_setup", "Encrypted and locked. Unlock it in Gust first; passwords never pass through this tool.".to_string())
                } else if d.mount_point.is_some() {
                    ("ready", "Mounted.".to_string())
                } else {
                    (
                        "ready",
                        "Plugged in; it is mounted when something is sent to it.".to_string(),
                    )
                };
                (d.display(), "drive", "usb", state, detail)
            }
            Endpoint::Taildrop(t) => {
                let (state, detail) = if t.online {
                    ("ready", "Online on the tailnet.")
                } else {
                    ("offline", "Offline: start Tailscale on it, or wake it.")
                };
                (
                    t.name.clone(),
                    "tailscale",
                    "taildrop",
                    state,
                    detail.to_string(),
                )
            }
        };
        json!({
            "id": self.id(),
            "name": name,
            "kind": kind,
            "transport": transport,
            "state": state,
            "needs_folder": self.needs_folder(),
            "lands_in": self.lands_in(),
            "detail": detail,
        })
    }

    /// Open the device and return its local folder. For an iPhone or iPad
    /// this is where apps share files, the only place a computer can write.
    /// `waiting` hears, once, what the person must do on the device.
    pub fn open(
        &self,
        wait: Duration,
        cancel: &CancelToken,
        waiting: &mut dyn FnMut(String),
    ) -> Result<PathBuf, Failure> {
        match self {
            Endpoint::Phone { phone, .. } => {
                let mut target = phone.clone();
                if phone.kind == PhoneKind::Apple {
                    let Some(apps) = &phone.apps_uri else {
                        return Err(Failure::new(
                            "unsupported",
                            "This Apple device offers no app folders.",
                            "Send it with the taildrop entry instead.",
                        ));
                    };
                    target.uri = apps.clone();
                }
                let mut asked: Option<PhoneState> = None;
                let kind = phone.kind;
                let result = phones::mount(
                    &target,
                    Duration::from_secs(5) + wait,
                    &mut |state| {
                        if asked.is_none() {
                            waiting(state.words(kind));
                        }
                        asked = Some(state);
                    },
                    &|| cancel.is_cancelled(),
                );
                result.map_err(|state| {
                    // Running out of time while the device still waits for the
                    // person ends gio with "cancelled"; what it waited for is
                    // the useful answer.
                    let state = match (state, asked) {
                        (PhoneState::Failed(_), Some(waited)) if !cancel.is_cancelled() => waited,
                        (state, _) => state,
                    };
                    phone_failure(state, kind, self.what())
                })
            }
            Endpoint::Drive(d) => {
                if let Some(p) = &d.mount_point {
                    return Ok(p.clone());
                }
                if d.encrypted {
                    return Err(Failure::new(
                        "needs_setup",
                        "The drive is encrypted and locked.",
                        "Ask the person to unlock it in Gust, then send again.",
                    ));
                }
                gust_remote::block::mount(
                    &d.path,
                    &CallContext {
                        cancel,
                        timeout: Duration::from_secs(30),
                    },
                )
                .map_err(|f| {
                    Failure::new(
                        "device_unavailable",
                        f.error.message,
                        "Check that the drive is still plugged in, then try again.",
                    )
                })
            }
            Endpoint::Taildrop(_) => Err(Failure::new(
                "unsupported",
                "A Taildrop device has no folders to open.",
                "Send to it without a folder.",
            )),
        }
    }
}

fn phone_failure(state: PhoneState, kind: PhoneKind, what: &str) -> Failure {
    match state {
        PhoneState::NeedsPackage(pkg) => Failure::new("needs_setup", format!("Gust needs the {pkg} package to reach this {what}."), format!("Ask the person to install {pkg} (Gust's device setup offers it), then try again.")),
        PhoneState::WaitingForTrust => Failure::new("needs_trust", format!("The {what} has not trusted this computer yet."), state.words(kind) + ", then try again. If the person is away from it, use its taildrop entry if it has one."),
        PhoneState::WaitingForUnlock => Failure::new("needs_unlock", format!("The {what} is locked."), state.words(kind) + ", then try again. If the person is away from it, use its taildrop entry if it has one."),
        PhoneState::Refused => Failure::new("trust_refused", format!("The {what} refused to trust this computer."), "Unplug it and plug it in again to be asked again."),
        PhoneState::Ready => Failure::new("cancelled", "Cancelled before the device opened.", "Nothing was sent."),
        PhoneState::Failed(w) => Failure::new("device_unavailable", w, "Check the cable and the device, then try again."),
        PhoneState::Opening | PhoneState::Open(_) => Failure::new("device_unavailable", "The device did not open.", "Try again."),
    }
}

/// What is plugged in or reachable now, with notes on what could not be read.
pub struct Listing {
    pub endpoints: Vec<Endpoint>,
    pub notes: Vec<String>,
}

pub fn discover(cancel: &CancelToken) -> Listing {
    let mut endpoints = Vec::new();
    let mut notes = Vec::new();
    let found: Vec<Phone> = phones::scan()
        .into_iter()
        .filter(|p| p.kind != PhoneKind::Camera)
        .collect();
    if !found.is_empty() {
        let names = phones::friendly_names();
        endpoints.extend(found.into_iter().map(|phone| {
            let name = phones::display_name(&phone, &names);
            Endpoint::Phone { phone, name }
        }));
    }
    let ctx = CallContext {
        cancel,
        timeout: Duration::from_secs(5),
    };
    match gust_remote::block::list_devices(&ctx) {
        Ok(drives) => endpoints.extend(
            drives
                .into_iter()
                .filter(|d| d.kind == DeviceKind::Removable && d.fstype.is_some())
                .map(Endpoint::Drive),
        ),
        Err(f) => notes.push(format!("Drives: {}", f.error.message)),
    }
    let ctx = CallContext {
        cancel,
        timeout: Duration::from_secs(5),
    };
    match gust_remote::sharing::send_targets(&ctx) {
        Ok(targets) => endpoints.extend(targets.into_iter().map(Endpoint::Taildrop)),
        Err(e) if e.code == gust_remote::helper::ErrorCode::MissingHelper => {
            notes.push("Taildrop: Tailscale is not installed.".into())
        }
        Err(e) => notes.push(format!("Taildrop: {}", e.message)),
    }
    Listing { endpoints, notes }
}

/// The device an id names, looked up in a fresh scan: an id is never turned
/// into an address or a URI by itself.
pub fn find(id: &str, cancel: &CancelToken) -> Result<Endpoint, Failure> {
    discover(cancel)
        .endpoints
        .into_iter()
        .find(|e| e.id() == id)
        .ok_or_else(|| {
            Failure::new(
                "unknown_device",
                format!("No device with id {id:?} is connected now."),
                "Call list_devices and use one of its ids.",
            )
        })
}
