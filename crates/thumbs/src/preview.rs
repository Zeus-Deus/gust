//! Worker-only rich previews over the existing bwrap/prlimit helper contract.
//! Keep the returned document alive while its pages are displayed. Dropping it
//! removes only its own temporary conversion/profile, never the source.

use crate::{
    helper::{self, Input, Scratch},
    sandbox::SandboxSpec,
};
use gust_core::lister::CancelToken;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

pub const MAX_INPUT_BYTES: u64 = 512 * 1024 * 1024;
/// Media is streamed, never buffered in full; allow ordinary long recordings.
pub const MAX_MEDIA_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_PIXELS: u32 = 2048;
pub const MAX_PAGES: u32 = 10_000;
pub(crate) const MAX_PNG: usize = 24 * 1024 * 1024;
const MAX_PDF: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Image,
    Raw,
    Pdf,
    Office,
    Html,
    Font,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capability {
    pub kind: PreviewKind,
    pub missing: Vec<String>,
}
impl Capability {
    pub fn available(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Check the exact system paths used inside the sandbox, not a different tool
/// on the caller's PATH. A runtime sandbox/codec refusal still returns Helper.
pub fn capability(kind: PreviewKind) -> Capability {
    let mut required = vec!["bwrap", "prlimit", "timeout"];
    required.extend(match kind {
        PreviewKind::Image | PreviewKind::Font => vec!["magick"],
        PreviewKind::Raw => vec!["dcraw_emu", "magick"],
        PreviewKind::Pdf => vec!["pdfinfo", "pdftoppm"],
        PreviewKind::Office | PreviewKind::Html => vec!["libreoffice", "pdfinfo", "pdftoppm"],
        PreviewKind::Video => vec!["ffmpeg", "ffprobe"],
    });
    Capability {
        kind,
        missing: required
            .into_iter()
            .filter(|s| !installed(s))
            .map(str::to_string)
            .collect(),
    }
}

fn installed(bin: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(Path::new("/usr/bin").join(bin))
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum PreviewError {
    Cancelled,
    Unavailable { missing: Vec<String> },
    InvalidRequest,
    Source,
    Changed,
    Limit,
    Helper,
    Output,
}
impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Preview cancelled"),
            Self::Unavailable { missing } => write!(f, "Preview needs {}", missing.join(", ")),
            Self::InvalidRequest => f.write_str("Invalid preview page, time or size"),
            Self::Source => f.write_str("Preview needs a readable regular file"),
            Self::Changed => f.write_str("The file changed; reopen the preview"),
            Self::Limit => f.write_str("Preview exceeded its size, page or time limit"),
            Self::Helper => f.write_str("The sandboxed preview helper could not render this file"),
            Self::Output => f.write_str("The preview helper output was rejected"),
        }
    }
}
impl std::error::Error for PreviewError {}
impl From<helper::Error> for PreviewError {
    fn from(e: helper::Error) -> Self {
        match e {
            helper::Error::Cancelled => Self::Cancelled,
            helper::Error::Limit => Self::Limit,
            helper::Error::Failed => Self::Helper,
            helper::Error::Io => Self::Output,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocumentInfo {
    pub kind: PreviewKind,
    pub pages: u32,
    pub duration: Option<f64>,
    pub has_audio: bool,
    pub metadata: Vec<MetadataField>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RasterPage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub page: u32,
}

pub struct PreviewDocument {
    input: Input,
    converted: Option<Input>,
    // Keep the conversion alive until after its held descriptor is closed.
    _scratch: Scratch,
    info: DocumentInfo,
}

fn run(
    input: &Input,
    scratch: &Scratch,
    binary: &str,
    args: &[&str],
    cancel: &CancelToken,
    output_limit: u64,
) -> Result<Vec<u8>, PreviewError> {
    let mut spec = SandboxSpec::new("/in/input", &scratch.0);
    // LibreOffice maps a substantially larger shared-library set at startup.
    spec.address_space_bytes = if binary == "libreoffice" {
        3 * 1024 * 1024 * 1024
    } else {
        1024 * 1024 * 1024
    };
    spec.max_output_bytes = output_limit;
    spec.wall_timeout_seconds = 30;
    spec.cpu_seconds = 20;
    helper::capture_in(
        &helper::argv(&spec, binary, args),
        cancel,
        Duration::from_secs(30),
        64 * 1024,
        Some(input),
        Some(&scratch.0),
    )
    .map_err(Into::into)
}

pub fn inspect(
    path: &Path,
    kind: PreviewKind,
    cancel: &CancelToken,
) -> Result<PreviewDocument, PreviewError> {
    helper::check(cancel)?;
    let cap = capability(kind);
    if !cap.available() {
        return Err(PreviewError::Unavailable {
            missing: cap.missing,
        });
    }
    let source_limit = if kind == PreviewKind::Video {
        MAX_MEDIA_BYTES
    } else {
        MAX_INPUT_BYTES
    };
    let input = Input::open(path, source_limit).map_err(|e| match e {
        helper::Error::Limit => PreviewError::Limit,
        _ => PreviewError::Source,
    })?;
    helper::check(cancel)?;
    let scratch = Scratch::new()?;
    let mut info = DocumentInfo {
        kind,
        pages: 1,
        duration: None,
        has_audio: false,
        metadata: Vec::new(),
        notes: Vec::new(),
    };
    let converted = match kind {
        PreviewKind::Office | PreviewKind::Html => {
            // A private profile has no extensions/history/trust inherited from
            // the user's desktop. Never grant macros permission during import.
            let profile = scratch.0.join("profile/user");
            std::fs::create_dir_all(&profile).map_err(|_| PreviewError::Output)?;
            std::fs::write(profile.join("registrymodifications.xcu"), br#"<?xml version="1.0"?><oor:items xmlns:oor="http://openoffice.org/2001/registry"><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item><item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item></oor:items>"#).map_err(|_| PreviewError::Output)?;
            // The import filter sniffs the input; its host filename is never
            // copied into argv or converted output names.
            run(
                &input,
                &scratch,
                "libreoffice",
                &[
                    "-env:UserInstallation=file:///out/profile",
                    "--headless",
                    "--nologo",
                    "--nodefault",
                    "--norestore",
                    "--convert-to",
                    "pdf",
                    "--outdir",
                    "/out",
                    "/in/input",
                ],
                cancel,
                MAX_PDF as u64,
            )?;
            let bytes = helper::read_file(&scratch.0.join("input.pdf"), MAX_PDF, cancel)?;
            if !bytes.starts_with(b"%PDF-") {
                return Err(PreviewError::Output);
            }
            info.notes.push(
                "Static document rendering; external resources and macros are unavailable".into(),
            );
            Some(
                Input::open(&scratch.0.join("input.pdf"), MAX_PDF as u64)
                    .map_err(|_| PreviewError::Output)?,
            )
        }
        _ => None,
    };
    let render_input = converted.as_ref().unwrap_or(&input);
    match kind {
        PreviewKind::Pdf | PreviewKind::Office | PreviewKind::Html => {
            let bytes = run(
                render_input,
                &scratch,
                "pdfinfo",
                &["/in/input"],
                cancel,
                MAX_PDF as u64,
            )?;
            info.pages = crate::render::parse_pdf_pages(&String::from_utf8_lossy(&bytes))
                .ok_or(PreviewError::Output)?;
            if info.pages > MAX_PAGES {
                return Err(PreviewError::Limit);
            }
        }
        PreviewKind::Image | PreviewKind::Raw => {
            if kind == PreviewKind::Image {
                let bytes = run(
                    &input,
                    &scratch,
                    "magick",
                    &["identify", "-ping", "-format", "%w %h", "/in/input[0]"],
                    cancel,
                    64 * 1024,
                )?;
                let dimensions = String::from_utf8_lossy(&bytes);
                let mut numbers = dimensions
                    .split_whitespace()
                    .filter_map(|v| v.parse::<u32>().ok());
                let width = numbers.next().ok_or(PreviewError::Output)?;
                let height = numbers.next().ok_or(PreviewError::Output)?;
                if width == 0 || height == 0 || width as u64 * height as u64 > 256_000_000 {
                    return Err(PreviewError::Limit);
                }
                info.metadata.push(MetadataField {
                    name: "Dimensions".into(),
                    value: format!("{width} × {height}"),
                });
            }
            if installed("exiftool") {
                match run(
                    &input,
                    &scratch,
                    "exiftool",
                    &[
                        "-json",
                        "-n",
                        "-ImageWidth",
                        "-ImageHeight",
                        "-Orientation",
                        "-Make",
                        "-Model",
                        "-LensModel",
                        "-ExposureTime",
                        "-FNumber",
                        "-ISO",
                        "-DateTimeOriginal",
                        "/in/input",
                    ],
                    cancel,
                    64 * 1024,
                ) {
                    Ok(bytes) => match parse_exif(&bytes) {
                        Ok(fields) => info.metadata.extend(fields),
                        Err(_) => info.notes.push("EXIF metadata output was rejected".into()),
                    },
                    Err(PreviewError::Cancelled) => return Err(PreviewError::Cancelled),
                    Err(_) => info.notes.push("EXIF metadata could not be read".into()),
                }
            } else {
                info.notes.push("Camera metadata needs exiftool".into());
            }
            if kind == PreviewKind::Raw {
                info.notes.push(
                    "RAW uses half-size camera development; camera support depends on LibRaw"
                        .into(),
                );
            }
        }
        PreviewKind::Video => {
            let bytes = run(
                &input,
                &scratch,
                "ffprobe",
                &[
                    "-v",
                    "error",
                    "-print_format",
                    "json",
                    "-show_entries",
                    "format=duration:format_tags=title,artist,album:stream=codec_type",
                    "/in/input",
                ],
                cancel,
                64 * 1024,
            )?;
            let (duration, title, artist, album) =
                crate::audio::parse_probe(&String::from_utf8_lossy(&bytes));
            info.duration = duration;
            let probe: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| PreviewError::Output)?;
            info.has_audio = probe["streams"]
                .as_array()
                .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "audio"));
            if !probe["streams"]
                .as_array()
                .is_some_and(|streams| streams.iter().any(|s| s["codec_type"] == "video"))
            {
                return Err(PreviewError::Output);
            }
            for (name, value) in [("Title", title), ("Artist", artist), ("Album", album)] {
                if !value.is_empty() {
                    info.metadata.push(MetadataField {
                        name: name.into(),
                        value,
                    });
                }
            }
        }
        PreviewKind::Font => info
            .notes
            .push("Fixed specimen; the original font is loaded only inside the sandbox".into()),
    }
    helper::check(cancel)?;
    if !input.unchanged() {
        return Err(PreviewError::Changed);
    }
    Ok(PreviewDocument {
        input,
        converted,
        _scratch: scratch,
        info,
    })
}

impl PreviewDocument {
    pub(crate) fn ensure_unchanged(&self) -> Result<(), PreviewError> {
        if self.input.unchanged() {
            Ok(())
        } else {
            Err(PreviewError::Changed)
        }
    }
    pub(crate) fn media_input(&self) -> Result<Input, PreviewError> {
        self.input.try_clone().map_err(Into::into)
    }
    pub(crate) fn stream_argv(
        &self,
        scratch: &Scratch,
        start: f64,
        px: u32,
    ) -> Result<Vec<std::ffi::OsString>, PreviewError> {
        if self.info.kind != PreviewKind::Video
            || !start.is_finite()
            || start < 0.0
            || px == 0
            || px > MAX_PIXELS
        {
            return Err(PreviewError::InvalidRequest);
        }
        if !self.input.unchanged() {
            return Err(PreviewError::Changed);
        }
        let mut spec = SandboxSpec::new("/in/input", &scratch.0);
        spec.cpu_seconds = 3600;
        spec.wall_timeout_seconds = 12 * 3600;
        spec.address_space_bytes = 1024 * 1024 * 1024;
        let start = format!("{start:.3}");
        let scale = format!("scale={px}:{px}:force_original_aspect_ratio=decrease,fps=12");
        Ok(helper::argv(
            &spec,
            "ffmpeg",
            &[
                "-nostdin",
                "-v",
                "error",
                "-threads",
                "1",
                "-ss",
                &start,
                "-i",
                "/in/input",
                "-map",
                "0:v:0",
                "-an",
                "-vf",
                &scale,
                "-threads",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "pipe:1",
            ],
        ))
    }
    pub fn info(&self) -> &DocumentInfo {
        &self.info
    }

    pub fn page(
        &self,
        page: u32,
        px: u32,
        cancel: &CancelToken,
    ) -> Result<RasterPage, PreviewError> {
        self.render(page, px, 0.0, cancel)
    }

    /// Exact-time bounded frame for embedded video scrubbing. Playback workers
    /// may coalesce frame requests; never queue one request per slider event.
    pub fn frame(
        &self,
        seconds: f64,
        px: u32,
        cancel: &CancelToken,
    ) -> Result<RasterPage, PreviewError> {
        if self.info.kind != PreviewKind::Video
            || !seconds.is_finite()
            || seconds < 0.0
            || self.info.duration.is_some_and(|d| seconds >= d)
        {
            return Err(PreviewError::InvalidRequest);
        }
        self.render(1, px, seconds, cancel)
    }

    fn render(
        &self,
        page: u32,
        px: u32,
        seconds: f64,
        cancel: &CancelToken,
    ) -> Result<RasterPage, PreviewError> {
        helper::check(cancel)?;
        if page == 0 || page > self.info.pages || px == 0 || px > MAX_PIXELS {
            return Err(PreviewError::InvalidRequest);
        }
        if !self.input.unchanged() {
            return Err(PreviewError::Changed);
        }
        let scratch = Scratch::new()?;
        let input = self.converted.as_ref().unwrap_or(&self.input);
        let n = px.to_string();
        let p = page.to_string();
        let geom = format!("{px}x{px}>");
        match self.info.kind {
            PreviewKind::Image => {
                run(
                    input,
                    &scratch,
                    "magick",
                    &[
                        "-limit",
                        "thread",
                        "1",
                        "/in/input[0]",
                        "-auto-orient",
                        "-thumbnail",
                        &geom,
                        "-strip",
                        "-depth",
                        "8",
                        "png:/out/page.png",
                    ],
                    cancel,
                    MAX_PNG as u64,
                )?;
            }
            PreviewKind::Raw => {
                // Decode to a bounded file inside the same sandbox, then
                // normalize to PNG before anything reaches the application.
                run(
                    input,
                    &scratch,
                    "dcraw_emu",
                    &["-h", "-w", "-Z", "/out/raw.ppm", "/in/input"],
                    cancel,
                    MAX_PDF as u64,
                )?;
                run(
                    input,
                    &scratch,
                    "magick",
                    &[
                        "-limit",
                        "thread",
                        "1",
                        "/out/raw.ppm",
                        "-thumbnail",
                        &geom,
                        "-strip",
                        "-depth",
                        "8",
                        "png:/out/page.png",
                    ],
                    cancel,
                    MAX_PNG as u64,
                )?;
            }
            PreviewKind::Pdf | PreviewKind::Office | PreviewKind::Html => {
                run(
                    input,
                    &scratch,
                    "pdftoppm",
                    &[
                        "-png",
                        "-f",
                        &p,
                        "-l",
                        &p,
                        "-singlefile",
                        "-scale-to",
                        &n,
                        "/in/input",
                        "/out/page",
                    ],
                    cancel,
                    MAX_PNG as u64,
                )?;
            }
            PreviewKind::Font => {
                run(input, &scratch, "magick", &["-limit", "thread", "1", "-background", "white", "-fill", "black", "-font", "/in/input", "-pointsize", "32", "label:Aa Bb Cc Dd Ee Ff Gg\nThe quick brown fox\njumps over the lazy dog\n0123456789 !? @#%&", "-thumbnail", &geom, "-strip", "-depth", "8", "png:/out/page.png"], cancel, MAX_PNG as u64)?;
            }
            PreviewKind::Video => {
                let time = format!("{seconds:.3}");
                let scale = format!("scale={px}:{px}:force_original_aspect_ratio=decrease");
                run(
                    input,
                    &scratch,
                    "ffmpeg",
                    &[
                        "-nostdin",
                        "-v",
                        "error",
                        "-threads",
                        "1",
                        "-ss",
                        &time,
                        "-i",
                        "/in/input",
                        "-map",
                        "0:v:0",
                        "-an",
                        "-frames:v",
                        "1",
                        "-vf",
                        &scale,
                        "-threads",
                        "1",
                        "-y",
                        "/out/page.png",
                    ],
                    cancel,
                    MAX_PNG as u64,
                )?;
            }
        }
        let png = helper::read_file(&scratch.0.join("page.png"), MAX_PNG, cancel)?;
        let (width, height) = png_dimensions(&png, px)?;
        if !self.input.unchanged() || !input.unchanged() {
            return Err(PreviewError::Changed);
        }
        Ok(RasterPage {
            png,
            width,
            height,
            page,
        })
    }
}

pub(crate) fn png_dimensions(bytes: &[u8], max: u32) -> Result<(u32, u32), PreviewError> {
    if bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || bytes[8..12] != 13u32.to_be_bytes()
        || &bytes[12..16] != b"IHDR"
    {
        return Err(PreviewError::Output);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| PreviewError::Output)?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| PreviewError::Output)?);
    if width == 0
        || height == 0
        || width > max
        || height > max
        || bytes[24] > 16
        || bytes[26] != 0
        || bytes[27] != 0
        || bytes[28] > 1
    {
        return Err(PreviewError::Output);
    }
    let mut offset = 33usize;
    let mut data = false;
    for _ in 0..4096 {
        let head = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or(PreviewError::Output)?;
        let len =
            u32::from_be_bytes(head[..4].try_into().map_err(|_| PreviewError::Output)?) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .ok_or(PreviewError::Output)?;
        if end > bytes.len() {
            return Err(PreviewError::Output);
        }
        if &head[4..] == b"IDAT" {
            data = true;
        }
        if &head[4..] == b"IEND" {
            return if data && len == 0 && end == bytes.len() {
                Ok((width, height))
            } else {
                Err(PreviewError::Output)
            };
        }
        offset = end;
    }
    Err(PreviewError::Output)
}

fn parse_exif(bytes: &[u8]) -> Result<Vec<MetadataField>, PreviewError> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| PreviewError::Output)?;
    let Some(obj) = v.get(0).and_then(|v| v.as_object()) else {
        return Err(PreviewError::Output);
    };
    let mut out = Vec::new();
    for key in [
        "ImageWidth",
        "ImageHeight",
        "Orientation",
        "Make",
        "Model",
        "LensModel",
        "ExposureTime",
        "FNumber",
        "ISO",
        "DateTimeOriginal",
    ] {
        if let Some(value) = obj.get(key) {
            let value = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => continue,
            };
            out.push(MetadataField {
                name: key.into(),
                value: gust_core::peek::sanitise_line(value.as_bytes())
                    .chars()
                    .take(200)
                    .collect(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};

    fn require(kind: PreviewKind) -> bool {
        let c = capability(kind);
        if c.available() {
            true
        } else {
            gust_testkit::record_skip(
                "PREVIEWS",
                &gust_testkit::current_test(),
                &format!("missing {}", c.missing.join(", ")),
            );
            false
        }
    }

    fn pdf(pages: u32) -> Vec<u8> {
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
        let kids = (3..3 + pages)
            .map(|n| format!("{n} 0 R"))
            .collect::<Vec<_>>()
            .join(" ");
        objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"));
        for _ in 0..pages {
            objects.push(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>".into(),
            );
        }
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        bytes
    }

    fn synthetic_dng() -> Vec<u8> {
        // Uncompressed Bayer TIFF/DNG. Synthetic sensor bytes, never a photo.
        let long = |v: u32| v.to_le_bytes().to_vec();
        let short = |v: u16| v.to_le_bytes().to_vec();
        let rationals = |values: &[i32]| {
            values
                .iter()
                .flat_map(|v| [v.to_le_bytes(), 1i32.to_le_bytes()].concat())
                .collect::<Vec<_>>()
        };
        let mut fields = vec![
            (254u16, 4u16, 1u32, long(0)),
            (256, 4, 1, long(128)),
            (257, 4, 1, long(128)),
            (258, 3, 1, short(16)),
            (259, 3, 1, short(1)),
            (262, 3, 1, short(32803)),
            (271, 2, 10, b"Synthetic\0".to_vec()),
            (272, 2, 10, b"Synthetic\0".to_vec()),
            (273, 4, 1, long(0)),
            (274, 3, 1, short(1)),
            (277, 3, 1, short(1)),
            (278, 4, 1, long(128)),
            (279, 4, 1, long(128 * 128 * 2)),
            (284, 3, 1, short(1)),
            (33421, 3, 2, vec![2, 0, 2, 0]),
            (33422, 1, 4, vec![0, 1, 1, 2]),
            (50706, 1, 4, vec![1, 4, 0, 0]),
            (50707, 1, 4, vec![1, 1, 0, 0]),
            (50708, 2, 10, b"Synthetic\0".to_vec()),
            (50714, 4, 1, long(0)),
            (50717, 4, 1, long(65535)),
            (50721, 10, 9, rationals(&[1, 0, 0, 0, 1, 0, 0, 0, 1])),
            (50728, 5, 3, rationals(&[1, 1, 1])),
            (50778, 3, 1, short(21)),
        ];
        fields.sort_by_key(|f| f.0);
        let mut table = Vec::new();
        let mut data = Vec::new();
        let extra = 8 + 2 + fields.len() * 12 + 4;
        let mut strip_offset_at = 0;
        for (tag, ty, count, value) in fields.iter() {
            table.extend_from_slice(&tag.to_le_bytes());
            table.extend_from_slice(&ty.to_le_bytes());
            table.extend_from_slice(&count.to_le_bytes());
            if *tag == 273 {
                strip_offset_at = table.len();
            }
            if value.len() <= 4 {
                table.extend_from_slice(value);
                table.resize(table.len() + 4 - value.len(), 0);
            } else {
                table.extend_from_slice(&((extra + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(value);
                if data.len() % 2 != 0 {
                    data.push(0);
                }
            }
        }
        table[strip_offset_at..strip_offset_at + 4]
            .copy_from_slice(&((extra + data.len()) as u32).to_le_bytes());
        let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&(fields.len() as u16).to_le_bytes());
        bytes.extend(table);
        bytes.extend([0; 4]);
        bytes.extend(data);
        for y in 0..128u16 {
            for x in 0..128u16 {
                bytes.extend_from_slice(&(1000 + x * 200 + y * 100).to_le_bytes());
            }
        }
        bytes
    }

    fn docx() -> Vec<u8> {
        let files = [
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            (
                "word/document.xml",
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Synthetic Office preview</w:t></w:r></w:p></w:body></w:document>"#,
            ),
        ];
        let mut bytes = Vec::new();
        let mut directory = Vec::new();
        for (name, content) in files {
            let mut crc = !0u32;
            for b in content.bytes() {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
                }
            }
            crc = !crc;
            let offset = bytes.len() as u32;
            let size = content.len() as u32;
            bytes.extend(0x04034b50u32.to_le_bytes());
            for v in [20u16, 0, 0, 0, 0] {
                bytes.extend(v.to_le_bytes());
            }
            bytes.extend(crc.to_le_bytes());
            bytes.extend(size.to_le_bytes());
            bytes.extend(size.to_le_bytes());
            bytes.extend((name.len() as u16).to_le_bytes());
            bytes.extend(0u16.to_le_bytes());
            bytes.extend(name.bytes());
            bytes.extend(content.bytes());
            directory.extend(0x02014b50u32.to_le_bytes());
            for v in [20u16, 20, 0, 0, 0, 0] {
                directory.extend(v.to_le_bytes());
            }
            directory.extend(crc.to_le_bytes());
            directory.extend(size.to_le_bytes());
            directory.extend(size.to_le_bytes());
            directory.extend((name.len() as u16).to_le_bytes());
            for _ in 0..4 {
                directory.extend(0u16.to_le_bytes());
            }
            directory.extend(0u32.to_le_bytes());
            directory.extend(offset.to_le_bytes());
            directory.extend(name.bytes());
        }
        let start = bytes.len() as u32;
        let size = directory.len() as u32;
        bytes.extend(directory);
        bytes.extend(0x06054b50u32.to_le_bytes());
        for v in [0u16, 0, 3, 3] {
            bytes.extend(v.to_le_bytes());
        }
        bytes.extend(size.to_le_bytes());
        bytes.extend(start.to_le_bytes());
        bytes.extend(0u16.to_le_bytes());
        bytes
    }

    #[test]
    fn png_header_and_exif_are_bounded_and_allowlisted() {
        for n in 0..34 {
            assert_eq!(png_dimensions(&vec![0; n], 256), Err(PreviewError::Output));
        }
        let fields = parse_exif(br#"[{"Make":"Camera\u202e", "ISO":200,"GPSLatitude":1,"SerialNumber":"private","SourceFile":"private"}]"#).unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].value, "Camera·");
        assert_eq!(fields[1].value, "200");
        assert!(parse_exif(b"[]").is_err());
    }

    #[test]
    fn image_pages_hold_byte_paths_and_reject_changes_and_invalid_requests() {
        if !require(PreviewKind::Image) {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = d.0.join(OsString::from_vec(b"image-\xff.ppm".to_vec()));
        std::fs::write(&path, b"P3\n2 1\n255\n255 0 0 0 255 0\n").unwrap();
        let cancel = CancelToken::new();
        let doc = inspect(&path, PreviewKind::Image, &cancel).unwrap();
        let page = doc.page(1, 128, &cancel).unwrap();
        assert_eq!((page.width, page.height), (2, 1));
        assert_eq!(
            doc.page(0, 128, &cancel).unwrap_err(),
            PreviewError::InvalidRequest
        );
        assert_eq!(
            doc.page(1, u32::MAX, &cancel).unwrap_err(),
            PreviewError::InvalidRequest
        );
        let stop = CancelToken::new();
        stop.cancel();
        assert_eq!(
            doc.page(1, 128, &stop).unwrap_err(),
            PreviewError::Cancelled
        );
        std::fs::write(&path, b"changed").unwrap();
        assert_eq!(
            doc.page(1, 128, &cancel).unwrap_err(),
            PreviewError::Changed
        );
        let huge = d.0.join("huge");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        assert!(matches!(
            inspect(&huge, PreviewKind::Image, &cancel),
            Err(PreviewError::Limit)
        ));
        let link = d.0.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(matches!(
            inspect(&link, PreviewKind::Image, &cancel),
            Err(PreviewError::Source)
        ));
        assert!(matches!(
            inspect(&path, PreviewKind::Image, &cancel),
            Err(PreviewError::Helper)
        ));
    }

    #[test]
    fn pdf_pages_are_independent_and_bounded() {
        if !require(PreviewKind::Pdf) {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = d.0.join("document.pdf");
        std::fs::write(&path, pdf(2)).unwrap();
        let c = CancelToken::new();
        let doc = inspect(&path, PreviewKind::Pdf, &c).unwrap();
        assert_eq!(doc.info().pages, 2);
        for page in 1..=2 {
            let raster = doc.page(page, 320, &c).unwrap();
            assert_eq!((raster.width, raster.height, raster.page), (240, 320, page));
        }
        assert_eq!(
            doc.page(3, 320, &c).unwrap_err(),
            PreviewError::InvalidRequest
        );
        let scratch = doc._scratch.0.clone();
        drop(doc);
        assert!(!scratch.exists(), "document owns its conversion lifetime");
    }

    #[test]
    fn office_and_html_convert_in_private_profile_and_render() {
        let docx = docx();
        let rtf = b"{\\rtf1\\ansi A synthetic document.\\par Second paragraph.}";
        let html = b"<!doctype html><html><body><h1>Preview</h1><p>Static document</p><script>throw 'must not execute';</script></body></html>";
        for (kind, bytes) in [
            (PreviewKind::Office, rtf.as_slice()),
            (PreviewKind::Html, html.as_slice()),
            (PreviewKind::Office, docx.as_slice()),
        ] {
            if !require(kind) {
                continue;
            }
            let d = Scratch::new().unwrap();
            let path = d.0.join("document");
            std::fs::write(&path, bytes).unwrap();
            let c = CancelToken::new();
            let doc = inspect(&path, kind, &c).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert!(doc.info().pages >= 1);
            let page = doc.page(1, 320, &c).unwrap();
            assert!(page.height <= 320 && page.width <= 320);
        }
    }

    #[test]
    fn font_specimen_is_rendered_inside_sandbox() {
        if !require(PreviewKind::Font) {
            return;
        }
        let font = [
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        ]
        .into_iter()
        .find(|p| Path::new(p).is_file());
        let Some(font) = font else {
            gust_testkit::skip!("PREVIEWS", "no synthetic specimen font available");
        };
        let c = CancelToken::new();
        let doc = inspect(Path::new(font), PreviewKind::Font, &c).unwrap();
        let page = doc.page(1, 640, &c).unwrap();
        assert!(page.width > 10 && page.height > 10);
    }

    #[test]
    fn raw_camera_preview_is_developed_only_in_the_sandbox() {
        if !require(PreviewKind::Raw) {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = d.0.join("sensor.dng");
        std::fs::write(&path, synthetic_dng()).unwrap();
        let c = CancelToken::new();
        let doc = inspect(&path, PreviewKind::Raw, &c).unwrap();
        let page = doc.page(1, 128, &c).unwrap();
        assert!(page.width > 10 && page.width <= 128 && page.height > 10 && page.height <= 128);
    }

    #[test]
    fn video_seek_frame_uses_sandbox_and_rejects_nonfinite_time() {
        if !require(PreviewKind::Video) {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = d.0.join("clip.mp4");
        assert!(Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=10:duration=2",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-preset",
                "ultrafast"
            ])
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let c = CancelToken::new();
        let doc = inspect(&path, PreviewKind::Video, &c).unwrap();
        assert!(doc.info.duration.is_some_and(|d| d >= 1.9));
        let a = doc.frame(0.0, 160, &c).unwrap();
        let b = doc.frame(1.0, 160, &c).unwrap();
        assert_eq!((a.width, a.height), (160, 90));
        assert_ne!(a.png, b.png);
        assert_eq!(
            doc.frame(f64::NAN, 160, &c).unwrap_err(),
            PreviewError::InvalidRequest
        );
        assert_eq!(
            doc.frame(f64::INFINITY, 160, &c).unwrap_err(),
            PreviewError::InvalidRequest
        );
    }
}
