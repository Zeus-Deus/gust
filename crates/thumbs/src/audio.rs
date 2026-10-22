//! Audio in Quick Look, with the untrusted decoder sandboxed (DESIGN §6.9.1).
//!
//! The file is decoded by `ffmpeg` under the same `bwrap`+`prlimit` sandbox
//! as every preview helper: no network, no home, no audio socket, the file
//! bound read-only. It writes raw PCM (s16le, 48 kHz, stereo) to a pipe and
//! nothing else. Gust paces that PCM into the desktop's own player
//! (`pw-play`, or `paplay`), which only ever sees raw samples it cannot
//! misparse.
//!
//! Giving the decoder the PipeWire socket instead would have been simpler,
//! and would have handed a hijacked decoder the microphone. Letting it pace
//! itself through a pipe (mpv `--ao=pcm`) took ~3 s to honour a pause. Here
//! Gust owns the clock: pause stops the relay, seek restarts the decoder at
//! the new time, and both take effect within the ~0.25 s already queued.
//!
//! Nothing here blocks the caller: [`Player::start`] returns at once and all
//! process work happens on the player's own thread.

use crate::sandbox::{sandbox_available, SandboxSpec};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(test)]
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Raw PCM the decoder produces: s16le, 48 kHz, 2 channels.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
pub const BYTES_PER_SEC: f64 = (RATE * CHANNELS * 2) as f64;
/// How far ahead of the listener the relay may run. Bounds pause latency.
const AHEAD_SECS: f64 = 0.25;
/// One relay step: a tenth of a second of sound.
const CHUNK: usize = (RATE * CHANNELS * 2 / 10) as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Probing the file and starting the decoder.
    Loading,
    Playing,
    Paused,
    /// Reached the end; a seek plays again.
    Ended,
    /// Could not play: see `Status::why`.
    Failed,
}

impl PlayState {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayState::Loading => "loading",
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Ended => "ended",
            PlayState::Failed => "failed",
        }
    }
}

/// What the panel shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub state: PlayState,
    /// Seconds from the start of the file.
    pub pos: f64,
    pub duration: Option<f64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Why it failed, in words.
    pub why: String,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            state: PlayState::Loading,
            pos: 0.0,
            duration: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            why: String::new(),
        }
    }
}

#[derive(Default)]
struct Shared {
    status: Mutex<Status>,
    /// Requested seek, in seconds (taken by the player thread).
    seek_to: Mutex<Option<f64>>,
    paused: AtomicBool,
    stop: AtomicBool,
    cancel: gust_core::CancelToken,
    /// Linear gain encoded as f32 bits; zero means silent until initialized.
    volume: AtomicU32,
    muted: AtomicBool,
}

/// One file playing. Dropping it stops the sound and both processes.
pub struct Player {
    shared: Arc<Shared>,
}

/// Which programs can play: the sandbox, a decoder and a sink.
pub fn available() -> Result<(), &'static str> {
    if !sandbox_available() {
        return Err("Playing audio needs bubblewrap (the preview sandbox)");
    }
    if !crate::sandbox::which("ffmpeg") {
        return Err("Playing audio needs ffmpeg");
    }
    if sink_argv().is_none() {
        return Err("Playing audio needs pw-play (PipeWire) or paplay");
    }
    Ok(())
}

/// The desktop's raw-PCM player: PipeWire's, else PulseAudio's.
pub(crate) fn sink_argv() -> Option<Vec<String>> {
    let fmt = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if crate::sandbox::which("pw-play") {
        return Some(fmt(&[
            "pw-play",
            "--raw",
            "--format=s16",
            "--rate=48000",
            "--channels=2",
            "--latency=60ms",
            "--media-role=Music",
            "-",
        ]));
    }
    if crate::sandbox::which("paplay") {
        return Some(fmt(&[
            "paplay",
            "--raw",
            "--format=s16le",
            "--rate=48000",
            "--channels=2",
        ]));
    }
    None
}

/// The sandboxed decoder: the file at `/in/input`, PCM on stdout.
pub fn decoder_argv(path: &Path, scratch: &Path, start: f64) -> Vec<String> {
    let mut spec = SandboxSpec::new(path, scratch);
    // A long album or an audiobook decodes for hours; the budget is CPU time
    // and memory, not the preview helpers' 20-second wall clock.
    spec.cpu_seconds = 3600;
    spec.wall_timeout_seconds = 12 * 3600;
    // ffmpeg's decoder threads reserve address space for their stacks; under
    // the helpers' 512 MiB cap about 1 run in 10 failed to start one (EINVAL
    // / EAGAIN from pthread_create) on an 8-core VM. 1 GiB: 0 in 48. This
    // caps address space, not memory actually used.
    spec.address_space_bytes = 1024 * 1024 * 1024;
    let start = format!("{:.3}", start.max(0.0));
    spec.bwrap_argv(
        "ffmpeg",
        &[
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &start,
            "-i",
            "/in/input",
            // Audio only: an embedded cover image is not decoded.
            "-map",
            "0:a:0",
            "-vn",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "2",
            "-ar",
            "48000",
            "pipe:1",
        ],
    )
}

/// The sandboxed probe: duration and the three tags the panel shows.
pub fn probe_argv(path: &Path, scratch: &Path) -> Vec<String> {
    let spec = SandboxSpec::new(path, scratch);
    spec.bwrap_argv(
        "ffprobe",
        &[
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_entries",
            "format=duration:format_tags=title,artist,album",
            "/in/input",
        ],
    )
}

/// Read ffprobe's JSON. Tags are sanitised: they are the file's words.
pub fn parse_probe(json: &str) -> (Option<f64>, String, String, String) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let f = &v["format"];
    let dur = f["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    let tag = |k: &str| {
        let t = &f["tags"];
        // Tag keys come in any case (TITLE, Title, title).
        let s = t
            .as_object()
            .and_then(|o| o.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)))
            .and_then(|(_, v)| v.as_str())
            .unwrap_or("");
        let clean = gust_core::peek::sanitise_line(s.as_bytes());
        clean.chars().take(200).collect::<String>()
    };
    (dur, tag("title"), tag("artist"), tag("album"))
}

impl Player {
    /// Start playing `path` from the beginning. Returns at once.
    pub fn start(path: &Path) -> Player {
        Self::start_with_sink(path, sink_argv())
    }

    /// As [`start`](Self::start), with a chosen sink (tests write to a file).
    pub fn start_with_sink(path: &Path, sink: Option<Vec<String>>) -> Player {
        Self::start_worker(path, sink, None)
    }

    pub(crate) fn start_held(input: crate::helper::Input, sink: Vec<String>) -> Player {
        Self::start_worker(Path::new(""), Some(sink), Some(input))
    }

    fn start_worker(
        path: &Path,
        sink: Option<Vec<String>>,
        input: Option<crate::helper::Input>,
    ) -> Player {
        let shared = Arc::new(Shared::default());
        shared.volume.store(1.0f32.to_bits(), Ordering::Relaxed);
        let s = shared.clone();
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("gust-audio".into())
            .spawn(move || {
                if let Err(why) = run(&path, sink, &s, input) {
                    if !s.stop.load(Ordering::Relaxed) {
                        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = PlayState::Failed;
                        st.why = why;
                    }
                }
            });
        if spawned.is_err() {
            let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = PlayState::Failed;
            st.why = "could not start the player".into();
        }
        Player { shared }
    }

    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn toggle(&self) {
        let st = self.status().state;
        if st == PlayState::Ended {
            self.seek(0.0);
            self.set_paused(false);
        } else {
            let p = self.shared.paused.load(Ordering::Relaxed);
            self.set_paused(!p);
        }
    }

    /// Jump to `secs` from the start (clamped to the file).
    pub fn seek(&self, secs: f64) {
        if !secs.is_finite() {
            return;
        }
        let dur = self.status().duration;
        let t = match dur {
            Some(d) => secs.clamp(0.0, (d - 0.05).max(0.0)),
            None => secs.max(0.0),
        };
        *self
            .shared
            .seek_to
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(t);
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cancel.cancel();
    }

    /// Linear PCM gain, clamped to 0..=1. Nonfinite requests are ignored.
    pub fn set_volume(&self, volume: f32) {
        if volume.is_finite() {
            self.shared
                .volume
                .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }
    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }
    pub fn is_stopped(&self) -> bool {
        self.shared.stop.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

fn set(s: &Shared, f: impl FnOnce(&mut Status)) {
    f(&mut s.status.lock().unwrap_or_else(|e| e.into_inner()));
}

fn run(
    path: &Path,
    sink: Option<Vec<String>>,
    s: &Shared,
    input: Option<crate::helper::Input>,
) -> Result<(), String> {
    use crate::helper::{self, OwnedChild};
    let fail = |_| "the sandboxed media helper stopped or exceeded its limit".to_string();
    helper::check(&s.cancel).map_err(fail)?;
    if !sandbox_available() {
        return Err("Playing audio needs bubblewrap and prlimit".into());
    }
    if !crate::sandbox::which("ffmpeg") {
        return Err("Playing audio needs ffmpeg".into());
    }
    let sink = sink.ok_or("Playing audio needs pw-play (PipeWire) or paplay")?;
    let input = input
        .map(Ok)
        .unwrap_or_else(|| helper::Input::open(path, crate::preview::MAX_MEDIA_BYTES))
        .map_err(|_| "Audio needs a regular file within the preview size limit")?;
    let scratch = helper::Scratch::new().map_err(fail)?;
    let argv = |a: Vec<String>| {
        a.into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    };

    // Duration and tags (sandboxed; at most 64 KiB of its output is read).
    if let Ok(text) = helper::capture_in(
        &helper::pinned_argv(probe_argv(Path::new("/in/input"), &scratch.0), &scratch.0),
        &s.cancel,
        Duration::from_secs(5),
        64 * 1024,
        Some(&input),
        Some(&scratch.0),
    ) {
        let (dur, title, artist, album) = parse_probe(&String::from_utf8_lossy(&text));
        set(s, |st| {
            st.duration = dur;
            st.title = title;
            st.artist = artist;
            st.album = album;
        });
    }
    if s.stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let mut sink_proc =
        OwnedChild::spawn(&argv(sink), Stdio::piped(), Stdio::null()).map_err(fail)?;
    let mut sink_in = sink_proc.child().stdin.take().ok_or("no sink input")?;
    helper::nonblocking(&sink_in).map_err(fail)?;

    let mut start_at = 0.0f64;
    let mut buf = vec![0u8; CHUNK];
    'decoders: loop {
        helper::check(&s.cancel).map_err(fail)?;
        if !input.unchanged() {
            return Err("The media file changed; reopen the preview".into());
        }
        let mut dec = OwnedChild::spawn_stream_input(
            &helper::pinned_argv(
                decoder_argv(Path::new("/in/input"), &scratch.0, start_at),
                &scratch.0,
            ),
            &input,
        )
        .map_err(fail)?;
        let mut out = dec.child().stdout.take().ok_or("no decoder output")?;
        helper::nonblocking(&out).map_err(fail)?;
        let mut pending = Vec::new();
        let mut sent = 0usize;
        let mut tail = Vec::new();
        let mut written = 0.0f64; // seconds handed to the sink
        let mut played = 0.0f64; // seconds the listener has heard
        let mut last = Instant::now();
        let mut eof = false;
        let mut progress = Instant::now();
        let mut revision_check = Instant::now();
        set(s, |st| {
            st.state = PlayState::Playing;
            st.pos = start_at;
        });
        loop {
            if s.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if revision_check.elapsed() >= Duration::from_millis(100) {
                if !input.unchanged() {
                    return Err("The media file changed; reopen the preview".into());
                }
                revision_check = Instant::now();
            }
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64();
            last = now;
            let paused = s.paused.load(Ordering::Relaxed);
            if !paused {
                played = (played + dt).min(written);
            }
            let requested_seek = s.seek_to.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(t) = requested_seek {
                start_at = t;
                drop(dec);
                continue 'decoders;
            }
            let ended = eof && played >= written;
            set(s, |st| {
                st.pos = start_at + played;
                st.state = if ended {
                    PlayState::Ended
                } else if paused {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                };
            });
            if paused || ended || eof || written - played > AHEAD_SECS {
                progress = Instant::now();
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if progress.elapsed() > Duration::from_secs(15) {
                return Err("the media decoder or audio sink stopped responding".into());
            }
            // A blocked decoder or desktop sink cannot block Stop or Seek.
            if sent < pending.len() {
                if !input.unchanged() {
                    return Err("The media file changed; reopen the preview".into());
                }
                match sink_in.write(&pending[sent..]) {
                    Ok(0) => return Err("the desktop's audio player stopped".into()),
                    Ok(n) => {
                        progress = Instant::now();
                        sent += n;
                        written += n as f64 / BYTES_PER_SEC;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err("the desktop's audio player stopped".into()),
                }
                continue;
            }
            match out.read(&mut buf) {
                Ok(0) => match dec.child().try_wait() {
                    Ok(Some(status))
                        if status.success()
                            && tail.is_empty()
                            && (written > 0.0 || start_at > 0.0) =>
                    {
                        eof = true
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    _ => return Err("the file could not be decoded as audio".into()),
                },
                Ok(n) => {
                    progress = Instant::now();
                    pending.clear();
                    pending.extend_from_slice(&tail);
                    pending.extend_from_slice(&buf[..n]);
                    let whole = pending.len() - pending.len() % 4;
                    tail = pending.split_off(whole);
                    let volume = if s.muted.load(Ordering::Relaxed) {
                        0.0
                    } else {
                        f32::from_bits(s.volume.load(Ordering::Relaxed))
                    };
                    scale_pcm(&mut pending, volume);
                    sent = 0;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err("the media decoder stopped".into()),
            }
        }
    }
}

fn scale_pcm(bytes: &mut [u8], volume: f32) {
    for sample in bytes.as_chunks_mut::<2>().0 {
        let value = i16::from_le_bytes([sample[0], sample[1]]);
        let scaled = (value as f32 * volume).round() as i16;
        sample.copy_from_slice(&scaled.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_gain_and_controls_are_finite_and_do_not_overflow() {
        let mut bytes = [
            i16::MIN.to_le_bytes(),
            i16::MAX.to_le_bytes(),
            (-1234i16).to_le_bytes(),
        ]
        .concat();
        scale_pcm(&mut bytes, 0.5);
        assert_eq!(i16::from_le_bytes(bytes[0..2].try_into().unwrap()), -16384);
        assert_eq!(i16::from_le_bytes(bytes[2..4].try_into().unwrap()), 16384);
        scale_pcm(&mut bytes, 0.0);
        assert_eq!(bytes, vec![0; 6]);
        let p = Player {
            shared: Arc::new(Shared::default()),
        };
        p.set_volume(2.0);
        assert_eq!(p.volume(), 1.0);
        p.set_volume(f32::NAN);
        assert_eq!(p.volume(), 1.0);
        p.seek(f64::NAN);
        p.seek(f64::INFINITY);
        assert!(p.shared.seek_to.lock().unwrap().is_none());
        p.set_muted(true);
        assert!(p.muted());
    }

    #[test]
    fn paused_audio_source_change_refuses_resume_without_relaying_more_pcm() {
        gust_testkit::skip_unless!(
            sandbox_available() && gust_testkit::have("ffmpeg"),
            "AUDIO",
            "audio helpers missing"
        );
        let d = crate::helper::Scratch::new().unwrap();
        let song = d.0.join("tone.flac");
        assert!(Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4"
            ])
            .arg(&song)
            .status()
            .unwrap()
            .success());
        let pcm = d.0.join("heard.raw");
        let player = Player::start_with_sink(
            &song,
            Some(vec![
                "dd".into(),
                format!("of={}", pcm.display()),
                "status=none".into(),
            ]),
        );
        let wait = |wanted: PlayState| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let status = player.status();
                if status.state == wanted {
                    return status;
                }
                assert_ne!(status.state, PlayState::Failed, "{status:?}");
                assert!(Instant::now() < deadline, "{status:?}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        wait(PlayState::Playing);
        std::thread::sleep(Duration::from_millis(150));
        player.set_paused(true);
        wait(PlayState::Paused);
        std::thread::sleep(Duration::from_millis(100));
        let heard = std::fs::metadata(&pcm).unwrap().len();
        assert!(heard > 0);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&song)
            .unwrap()
            .write_all(b"source changed")
            .unwrap();
        player.set_paused(false);
        let failed = wait(PlayState::Failed);
        assert!(failed.why.contains("changed"), "{failed:?}");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&player.shared) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            Arc::strong_count(&player.shared),
            1,
            "relay must release decoder and sink"
        );
        assert_eq!(std::fs::metadata(pcm).unwrap().len(), heard);
    }

    #[test]
    fn the_decoder_runs_sandboxed_with_no_audio_or_network() {
        let a = decoder_argv(Path::new("/m/song.flac"), Path::new("/s"), 12.5);
        assert_eq!(a[0], "bwrap");
        assert!(a.contains(&"--unshare-all".to_string()));
        assert!(a.contains(&"--clearenv".to_string()));
        // No socket of any kind is bound in: not PipeWire, not Pulse.
        assert!(!a
            .iter()
            .any(|x| x.contains("pipewire") || x.contains("pulse")));
        assert!(!a.iter().any(|x| x.contains("XDG_RUNTIME_DIR")));
        let i = a.iter().position(|x| x == "-ss").unwrap();
        assert_eq!(a[i + 1], "12.500");
        assert!(a.iter().any(|x| x == "/in/input"));
        assert!(a.iter().any(|x| x == "--cpu=3600"));
        assert!(a.iter().any(|x| x == "--as=1073741824"));
        assert_eq!(a.last().unwrap(), "pipe:1");
    }

    #[test]
    fn probe_output_is_read_and_tags_are_cleaned() {
        let j = r#"{"format":{"duration":"183.4","tags":{"TITLE":"Song\u001b[31m","Artist":"Band","album":"LP"}}}"#;
        let (d, t, a, al) = parse_probe(j);
        assert_eq!(d, Some(183.4));
        assert_eq!(t, "Song·[31m", "escape shown, not obeyed");
        assert_eq!((a.as_str(), al.as_str()), ("Band", "LP"));
        assert_eq!(parse_probe("not json").0, None);
        assert_eq!(parse_probe(r#"{"format":{"duration":"N/A"}}"#).0, None);
    }

    /// Real decode, sandboxed, into a file instead of the speakers.
    #[test]
    fn a_file_plays_pauses_and_seeks_through_the_sandbox() {
        gust_testkit::skip_unless!(
            sandbox_available() && gust_testkit::have("ffmpeg") && gust_testkit::have("ffprobe"),
            "AUDIO",
            "bwrap, ffmpeg or ffprobe is not installed"
        );
        let d = std::env::temp_dir().join(format!("gust-audio-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let song = d.join("tone.flac");
        let ok = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=20")
            .arg("-metadata")
            .arg("title=Test tone")
            .arg(&song)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let sink_file = d.join("heard.raw");
        let sink = vec![
            "dd".to_string(),
            format!("of={}", sink_file.display()),
            "status=none".into(),
        ];
        let p = Player::start_with_sink(&song, Some(sink));
        let wait = |want: PlayState, secs: u64| {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(secs) {
                if p.status().state == want {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(wait(PlayState::Playing, 10), "{:?}", p.status());
        std::thread::sleep(Duration::from_millis(1200));
        let st = p.status();
        assert_eq!(st.title, "Test tone");
        assert!((st.duration.unwrap() - 20.0).abs() < 0.1, "{st:?}");
        assert!(st.pos > 0.8 && st.pos < 1.6, "real-time pace: {st:?}");

        p.set_paused(true);
        std::thread::sleep(Duration::from_millis(100));
        let a = p.status().pos;
        std::thread::sleep(Duration::from_millis(800));
        let b = p.status();
        assert_eq!(b.state, PlayState::Paused);
        assert!(
            (b.pos - a).abs() < 0.01,
            "moved while paused: {a} -> {}",
            b.pos
        );

        p.set_paused(false);
        p.seek(15.0);
        std::thread::sleep(Duration::from_millis(600));
        let c = p.status().pos;
        assert!((15.0..16.0).contains(&c), "seek: {c}");
        // Runs to the end on its own.
        assert!(wait(PlayState::Ended, 10), "{:?}", p.status());
        p.stop();
        // The sink received about as much sound as was played.
        let heard = std::fs::metadata(&sink_file).unwrap().len() as f64 / BYTES_PER_SEC;
        assert!(heard > 5.0 && heard < 8.5, "heard {heard}s");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_is_not_audio_fails_with_a_reason() {
        gust_testkit::skip_unless!(
            sandbox_available() && gust_testkit::have("ffmpeg"),
            "AUDIO",
            "bwrap or ffmpeg is not installed"
        );
        let d = std::env::temp_dir().join(format!("gust-audio-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("fake.mp3");
        std::fs::write(&f, b"this is not audio at all").unwrap();
        let p = Player::start_with_sink(&f, Some(vec!["true".into()]));
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) && p.status().state != PlayState::Failed {
            std::thread::sleep(Duration::from_millis(20));
        }
        let st = p.status();
        assert_eq!(st.state, PlayState::Failed, "{st:?}");
        assert!(!st.why.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
