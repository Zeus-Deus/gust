//! The three agent tools: their schemas and what they do.
//!
//! Sending is a copy and nothing else: only `Request::Copy` is built, with
//! "keep both" or "skip" on a name clash. There is no move, replace, merge,
//! delete or eject here (DESIGN §6.27).

use crate::admit;
use crate::devices::{self, Endpoint, Failure};
use gust_core::lister::CancelToken;
use gust_ops::jobs::{self, Request};
use gust_ops::journal::{ConflictPolicy, JobState};
use gust_remote::helper::CallContext;
use serde_json::{json, Map, Value};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What a running tool may tell the client. `progress` must keep growing;
/// the server drops a value that does not.
pub trait Report: Sync {
    fn progress(&self, progress: f64, total: Option<f64>, message: &str);
}

pub struct Call<'a> {
    pub cancel: &'a CancelToken,
    pub report: &'a dyn Report,
}

pub const INSTRUCTIONS: &str = "Send files from this computer to an iPhone, iPad, Android phone or USB drive plugged in by cable, or to the person's devices on Tailscale (Taildrop). Call list_devices first and use an id it returns.\n\nChoosing a route to an iPhone or iPad:\n- Any file, device on USB: send to its apple:… id with a folder from list_device_folders. Each folder is an app that accepts files (VLC, Documents…); the file opens in that app. This does not reach the Photos library.\n- Photos or videos the person wants in Photos: send to the device's taildrop:… entry. They then tap the Tailscale notification and Share › Save Image/Video. No route puts files into Photos without a tap.\n- needs_trust or needs_unlock: the person must unlock the device and tap Trust. If they are away from it, use its taildrop entry instead.\n\nSending only copies. An existing file on the device is never replaced: the new copy is renamed, or skipped if asked. Only regular files in the home folder, the temporary folder or on a removable drive can be sent; hidden files are refused.";

pub fn definitions() -> Value {
    let error_note = "On failure the result has isError true and one JSON text block {\"error\":{\"code\",\"message\",\"action\"}}.";
    json!([
        {
            "name": "list_devices",
            "title": "List devices",
            "description": "List the phones, tablets and USB drives plugged into this computer and the person's Tailscale devices that files can be sent to, with each one's id, state and where sent files land. Reads local state only; contacts no device.",
            "inputSchema": {"type": "object", "additionalProperties": false},
            "outputSchema": {
                "type": "object",
                "required": ["devices", "notes"],
                "properties": {
                    "devices": {"type": "array", "items": {
                        "type": "object",
                        "required": ["id", "name", "kind", "transport", "state", "needs_folder", "lands_in", "detail"],
                        "properties": {
                            "id": {"type": "string"},
                            "name": {"type": "string"},
                            "kind": {"enum": ["apple", "android", "drive", "tailscale"]},
                            "transport": {"enum": ["usb", "taildrop"]},
                            "state": {"enum": ["ready", "offline", "needs_setup"]},
                            "needs_folder": {"type": "boolean"},
                            "lands_in": {"type": "string"},
                            "detail": {"type": "string"}
                        }
                    }},
                    "notes": {"type": "array", "items": {"type": "string"}}
                }
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "list_device_folders",
            "title": "List folders on a device",
            "description": format!("List the folders one level inside a USB phone, tablet or drive, to choose where send_files puts files. On an iPhone or iPad the top level is one folder per app that accepts files. Opens the device if needed, which may ask the person to unlock it and tap Trust. {error_note}"),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["device"],
                "properties": {
                    "device": {"type": "string", "description": "An id from list_devices."},
                    "path": {"type": "string", "description": "A folder inside the device, as returned by an earlier call. Empty or absent for the top level."},
                    "wait_seconds": {"type": "integer", "minimum": 0, "maximum": 300, "default": 30, "description": "How long to wait for the person to unlock the device or tap Trust."}
                }
            },
            "outputSchema": {
                "type": "object",
                "required": ["device", "path", "folders", "truncated"],
                "properties": {
                    "device": {"type": "string"},
                    "path": {"type": "string"},
                    "folders": {"type": "array", "items": {
                        "type": "object",
                        "required": ["path", "name"],
                        "properties": {"path": {"type": "string"}, "name": {"type": "string"}}
                    }},
                    "truncated": {"type": "boolean"}
                }
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "send_files",
            "title": "Send files to a device",
            "description": format!("Copy files from this computer to a phone, tablet or drive on USB, or to a Tailscale device with Taildrop. Never replaces or deletes anything: a name already taken on the device gets a renamed copy, or is skipped. Reports progress while copying. {error_note}"),
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["device", "paths"],
                "properties": {
                    "device": {"type": "string", "description": "An id from list_devices."},
                    "paths": {"type": "array", "minItems": 1, "maxItems": admit::MAX_PATHS, "items": {"type": "string"}, "description": "Absolute paths of regular files on this computer."},
                    "folder": {"type": "string", "description": "Required for phones and tablets: a folder path from list_device_folders. Optional for drives (default: top level). Ignored for Taildrop."},
                    "on_conflict": {"enum": ["rename", "skip"], "default": "rename", "description": "What to do when the name is already taken on the device."},
                    "wait_seconds": {"type": "integer", "minimum": 0, "maximum": 300, "default": 30, "description": "How long to wait for the person to unlock the device or tap Trust."}
                }
            },
            "outputSchema": {
                "type": "object",
                "required": ["device", "transport", "sent", "skipped", "failed", "lands_in", "delivery_uncertain"],
                "properties": {
                    "device": {"type": "string"},
                    "transport": {"enum": ["usb", "taildrop"]},
                    "sent": {"type": "array", "items": {
                        "type": "object",
                        "required": ["source", "name", "bytes"],
                        "properties": {"source": {"type": "string"}, "name": {"type": "string"}, "bytes": {"type": "integer"}}
                    }},
                    "skipped": {"type": "array", "items": {"type": "string"}},
                    "failed": {"type": "array", "items": {
                        "type": "object",
                        "required": ["source", "reason"],
                        "properties": {"source": {"type": "string"}, "reason": {"type": "string"}}
                    }},
                    "lands_in": {"type": "string"},
                    "next_step": {"type": "string"},
                    "delivery_uncertain": {"type": "boolean"}
                }
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        }
    ])
}

pub fn exists(name: &str) -> bool {
    matches!(name, "list_devices" | "list_device_folders" | "send_files")
}

/// Run one tool. `Ok` is the structured result; `Err` becomes `isError`.
pub fn call(name: &str, args: &Map<String, Value>, call: &Call<'_>) -> Result<Value, Failure> {
    match name {
        "list_devices" => {
            only(args, &[])?;
            Ok(list_devices(call.cancel))
        }
        "list_device_folders" => list_device_folders(args, call),
        "send_files" => send_files(args, call),
        _ => Err(invalid("Unknown tool.")),
    }
}

fn invalid(message: impl Into<String>) -> Failure {
    Failure::new(
        "invalid_arguments",
        message,
        "Fix the arguments to match the tool's input schema.",
    )
}

fn only(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), Failure> {
    match args.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(invalid(format!("Unknown argument {k:?}."))),
        None => Ok(()),
    }
}

fn string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, Failure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(invalid(format!("{key} must be a string."))),
    }
}

fn required<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, Failure> {
    string(args, key)?.ok_or_else(|| invalid(format!("{key} is required.")))
}

fn wait_seconds(args: &Map<String, Value>) -> Result<Duration, Failure> {
    match args.get("wait_seconds") {
        None | Some(Value::Null) => Ok(Duration::from_secs(30)),
        Some(v) => v
            .as_u64()
            .filter(|s| *s <= 300)
            .map(Duration::from_secs)
            .ok_or_else(|| invalid("wait_seconds must be a whole number from 0 to 300.")),
    }
}

fn list_devices(cancel: &CancelToken) -> Value {
    let listing = devices::discover(cancel);
    json!({
        "devices": listing.endpoints.iter().map(Endpoint::describe).collect::<Vec<_>>(),
        "notes": listing.notes,
    })
}

const MAX_FOLDERS: usize = 500;

fn list_device_folders(args: &Map<String, Value>, call: &Call<'_>) -> Result<Value, Failure> {
    only(args, &["device", "path", "wait_seconds"])?;
    let id = required(args, "device")?;
    let path = string(args, "path")?.unwrap_or("");
    let wait = wait_seconds(args)?;
    let endpoint = devices::find(id, call.cancel)?;
    let root = endpoint.open(wait, call.cancel, &mut |words| {
        call.report.progress(0.0, None, &words)
    })?;
    let dir = admit::device_folder(&root, path).map_err(|m| {
        Failure::new(
            "folder_not_found",
            m,
            "Call list_device_folders without a path and pick from its folders.",
        )
    })?;
    let read = std::fs::read_dir(&dir).map_err(|_| {
        Failure::new(
            "device_unavailable",
            "The folder could not be read.",
            "Check that the device is still connected and unlocked, then try again.",
        )
    })?;
    let base = path.trim_matches('/');
    let mut folders = Vec::new();
    let mut truncated = false;
    for entry in read.flatten() {
        if call.cancel.is_cancelled() {
            break;
        }
        let name = entry.file_name();
        // Hidden folders and names that are not text are not offered.
        let Some(text) = name.to_str() else { continue };
        if name.as_bytes().starts_with(b".") || text.chars().any(char::is_control) {
            continue;
        }
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if folders.len() == MAX_FOLDERS {
            truncated = true;
            break;
        }
        let full = if base.is_empty() {
            text.to_string()
        } else {
            format!("{base}/{text}")
        };
        folders.push((full, text.to_string()));
    }
    folders.sort();
    Ok(json!({
        "device": id,
        "path": base,
        "folders": folders.into_iter().map(|(path, name)| json!({"path": path, "name": name})).collect::<Vec<_>>(),
        "truncated": truncated,
    }))
}

fn send_files(args: &Map<String, Value>, call: &Call<'_>) -> Result<Value, Failure> {
    only(
        args,
        &["device", "paths", "folder", "on_conflict", "wait_seconds"],
    )?;
    let id = required(args, "device")?;
    let raw: Vec<&str> = match args.get("paths") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().ok_or_else(|| invalid("paths must be strings.")))
            .collect::<Result<_, _>>()?,
        _ => return Err(invalid("paths must be a list of absolute file paths.")),
    };
    if raw.is_empty() || raw.len() > admit::MAX_PATHS {
        return Err(invalid(format!(
            "Send from 1 to {} files per call.",
            admit::MAX_PATHS
        )));
    }
    let folder = string(args, "folder")?;
    let policy = match string(args, "on_conflict")?.unwrap_or("rename") {
        "rename" => ConflictPolicy::KeepBoth,
        "skip" => ConflictPolicy::Skip,
        _ => return Err(invalid("on_conflict must be \"rename\" or \"skip\".")),
    };
    let wait = wait_seconds(args)?;

    let roots = admit::source_roots();
    let mut sources = Vec::new();
    let mut refused = Vec::new();
    for (i, p) in raw.iter().enumerate() {
        if raw[..i].contains(p) {
            refused.push(format!("{p}: listed twice."));
            continue;
        }
        match admit::source(p, &roots) {
            Ok(a) => sources.push(a),
            Err(r) => refused.push(format!("{}: {}", r.path, r.reason)),
        }
    }
    if !refused.is_empty() {
        return Err(Failure::new(
            "source_refused",
            refused.join("\n"),
            "Nothing was sent. Fix or remove these paths, then send again.",
        ));
    }

    let endpoint = devices::find(id, call.cancel)?;
    match &endpoint {
        Endpoint::Taildrop(target) => {
            taildrop(&endpoint, &target.address, target.online, &sources, call)
        }
        _ => copy(&endpoint, folder, policy, wait, &sources, call),
    }
}

fn result(
    endpoint: &Endpoint,
    transport: &str,
    sent: Vec<Value>,
    skipped: Vec<String>,
    failed: Vec<Value>,
    next_step: Option<&str>,
    uncertain: bool,
) -> Value {
    let mut v = json!({
        "device": endpoint.id(),
        "transport": transport,
        "sent": sent,
        "skipped": skipped,
        "failed": failed,
        "lands_in": endpoint.lands_in(),
        "delivery_uncertain": uncertain,
    });
    if let Some(n) = next_step {
        v["next_step"] = json!(n);
    }
    v
}

fn display(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn taildrop(
    endpoint: &Endpoint,
    address: &str,
    online: bool,
    sources: &[admit::Admitted],
    call: &Call<'_>,
) -> Result<Value, Failure> {
    if !online {
        return Err(Failure::new(
            "device_offline",
            "That device is offline on the tailnet.",
            "Ask the person to open Tailscale on it, then send again.",
        ));
    }
    let ctx = CallContext {
        cancel: call.cancel,
        timeout: Duration::from_secs(10),
    };
    gust_remote::sharing::eligible_target(address, &ctx).map_err(|e| {
        Failure::new(
            "device_offline",
            e.message,
            "Check Tailscale on both devices, then send again.",
        )
    })?;
    let paths: Vec<PathBuf> = sources.iter().map(|s| s.path.clone()).collect();
    // Taildrop reports nothing until it is done, so the elapsed time is the
    // progress: it keeps a long send from looking stalled to the client.
    let done = AtomicBool::new(false);
    let outcome = std::thread::scope(|scope| {
        scope.spawn(|| {
            let started = Instant::now();
            let mut last = started;
            call.report.progress(0.0, None, "Sending with Taildrop…");
            while !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(250));
                if last.elapsed() >= Duration::from_secs(10) {
                    last = Instant::now();
                    let secs = started.elapsed().as_secs_f64().floor();
                    call.report.progress(secs, None, "Sending with Taildrop…");
                }
            }
        });
        let outcome = gust_remote::send::taildrop(
            address,
            &paths,
            &CallContext {
                cancel: call.cancel,
                timeout: Duration::from_secs(60 * 60),
            },
        );
        done.store(true, Ordering::Release);
        outcome
    });
    let entry = |s: &admit::Admitted| {
        json!({
            "source": display(&s.path),
            "name": s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "bytes": s.bytes,
        })
    };
    let next = "On an iPhone or iPad: tap the Tailscale notification; the file is in Files › Tailscale, and Share › Save Image or Save Video adds it to Photos.";
    match outcome {
        Ok(_) => Ok(result(
            endpoint,
            "taildrop",
            sources.iter().map(entry).collect(),
            Vec::new(),
            Vec::new(),
            Some(next),
            false,
        )),
        Err(f) if f.accepted_files == 0 && !f.delivery_uncertain => Err(Failure::new(
            if f.error.code == gust_remote::helper::ErrorCode::Cancelled {
                "cancelled"
            } else {
                "transfer_failed"
            },
            f.error.message,
            "Check Tailscale on both devices, then send again.",
        )),
        Err(f) => {
            let n = f.accepted_files.min(sources.len());
            let failed = sources[n..]
                .iter()
                .map(|s| json!({"source": display(&s.path), "reason": f.error.message}))
                .collect();
            Ok(result(
                endpoint,
                "taildrop",
                sources[..n].iter().map(entry).collect(),
                Vec::new(),
                failed,
                Some(next),
                f.delivery_uncertain,
            ))
        }
    }
}

fn copy(
    endpoint: &Endpoint,
    folder: Option<&str>,
    policy: ConflictPolicy,
    wait: Duration,
    sources: &[admit::Admitted],
    call: &Call<'_>,
) -> Result<Value, Failure> {
    if endpoint.needs_folder() && folder.is_none_or(|f| admit::first_component(f).is_none()) {
        return Err(Failure::new(
            "folder_required",
            "A phone or tablet has no top-level folder to send to.",
            "Call list_device_folders for this device and pass one of its folders as folder.",
        ));
    }
    let root = endpoint.open(wait, call.cancel, &mut |words| {
        call.report.progress(0.0, None, &words)
    })?;
    let dest = admit::device_folder(&root, folder.unwrap_or("")).map_err(|m| {
        Failure::new(
            "folder_not_found",
            m,
            "Call list_device_folders and pick one of its folders.",
        )
    })?;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut job = jobs::start_with(
        format!(
            "agent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        Request::Copy {
            sources: sources.iter().map(|s| s.path.clone()).collect(),
            dest_dir: dest,
        },
        policy,
        None,
        None,
    );
    if job.start_error().is_some() {
        return Err(Failure::new(
            "transfer_failed",
            "Could not start the copy; nothing was changed.",
            "Try again.",
        ));
    }
    loop {
        if call.cancel.is_cancelled() {
            job.cancel();
        }
        let s = job.snapshot();
        if s.is_finished() || s.state == JobState::Conflict {
            break;
        }
        if s.bytes_done > 0 {
            let n = (s.entries_done + 1).min(s.entries_total.max(1));
            call.report.progress(
                s.bytes_done as f64,
                Some(s.bytes_total as f64),
                &format!("Copying {n} of {}…", s.entries_total.max(1)),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let end = job.wait();
    let record = job.record();
    if end.state == JobState::Cancelled {
        return Err(Failure::new(
            "cancelled",
            "Cancelled. Files already copied were kept.",
            "Call list_device_folders to see what arrived.",
        ));
    }

    let mut sent = Vec::new();
    let mut failed = Vec::new();
    for s in sources {
        let entry = record.entries.iter().find(|e| e.source == s.path);
        match entry {
            Some(e) if e.done => sent.push(json!({
                "source": display(&s.path),
                "name": e.destination.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                "bytes": s.bytes,
            })),
            Some(e) => failed.push(json!({
                "source": display(&s.path),
                "reason": e.error.clone().unwrap_or_else(|| end.message.clone()),
            })),
            None => {}
        }
    }
    let accounted: Vec<String> = sent
        .iter()
        .chain(failed.iter())
        .filter_map(|v| v["source"].as_str().map(str::to_string))
        .collect();
    let mut skipped = Vec::new();
    for s in sources {
        let p = display(&s.path);
        if accounted.contains(&p) {
            continue;
        }
        if end.state == JobState::Confirmed {
            skipped.push(p);
        } else {
            failed.push(json!({"source": p, "reason": end.message}));
        }
    }
    if sent.is_empty() && !failed.is_empty() {
        let reasons: Vec<&str> = failed.iter().filter_map(|f| f["reason"].as_str()).collect();
        return Err(Failure::new(
            "transfer_failed",
            reasons.join("\n"),
            "Nothing was sent. Check the device and its free space, then try again.",
        ));
    }
    let next = match endpoint {
        Endpoint::Phone { phone, .. } if phone.kind == gust_remote::phones::PhoneKind::Apple => {
            Some("Open the file from the app whose folder it was sent to.")
        }
        _ => None,
    };
    Ok(result(endpoint, "usb", sent, skipped, failed, next, false))
}
