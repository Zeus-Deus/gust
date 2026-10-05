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
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
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
    /// PIDs to kill on stop, so a thread blocked in a read or write wakes.
    decoder_pid: AtomicU32,
    sink_pid: AtomicU32,
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
fn sink_argv() -> Option<Vec<String>> {
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
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("gust-audio".into())
            .spawn(move || {
                if let Err(why) = run(&path, sink, &s) {
                    if !s.stop.load(Ordering::Relaxed) {
                        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = PlayState::Failed;
                        st.why = why;
                    }
                }
                kill_all(&s);
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
        kill_all(&self.shared);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

fn kill_all(s: &Shared) {
    for pid in [&s.decoder_pid, &s.sink_pid] {
        let p = pid.swap(0, Ordering::Relaxed);
        if let Some(p) = rustix::process::Pid::from_raw(p as i32) {
            let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
        }
    }
}

/// A child process, killed and reaped when dropped.
struct Proc(Child);
impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(argv: &[String], stdin: Stdio, stdout: Stdio) -> Result<Proc, String> {
    spawn_err(argv, stdin, stdout, Stdio::null())
}

fn spawn_err(argv: &[String], stdin: Stdio, stdout: Stdio, stderr: Stdio) -> Result<Proc, String> {
    Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map(Proc)
        .map_err(|e| format!("{}: {e}", argv[0]))
}

fn set(s: &Shared, f: impl FnOnce(&mut Status)) {
    f(&mut s.status.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The scratch folder the sandbox wants for `/out` (unused by the decoder).
fn scratch_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let d = base.join(format!(
        "gust-audio-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn run(path: &Path, sink: Option<Vec<String>>, s: &Shared) -> Result<(), String> {
    available()?;
    let sink = sink.ok_or("Playing audio needs pw-play (PipeWire) or paplay")?;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let scratch = scratch_dir()?;
    let _cleanup = Cleanup(scratch.clone());

    // Duration and tags (sandboxed; at most 64 KiB of its output is read).
    if let Ok(mut probe) = spawn(&probe_argv(path, &scratch), Stdio::null(), Stdio::piped()) {
        let mut text = Vec::new();
        if let Some(out) = probe.0.stdout.take() {
            let _ = out.take(64 * 1024).read_to_end(&mut text);
        }
        drop(probe);
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

    let mut sink_proc = spawn(&sink, Stdio::piped(), Stdio::null())?;
    s.sink_pid.store(sink_proc.0.id(), Ordering::Relaxed);
    let mut sink_in: ChildStdin = sink_proc.0.stdin.take().ok_or("no sink input")?;

    let mut start_at = 0.0f64;
    let mut buf = vec![0u8; CHUNK];
    'decoders: loop {
        let mut dec = spawn_err(
            &decoder_argv(path, &scratch, start_at),
            Stdio::null(),
            Stdio::piped(),
            Stdio::piped(),
        )?;
        s.decoder_pid.store(dec.0.id(), Ordering::Relaxed);
        let mut out = dec.0.stdout.take().ok_or("no decoder output")?;
        let mut written = 0.0f64; // seconds handed to the sink
        let mut played = 0.0f64; // seconds the listener has heard
        let mut last = Instant::now();
        let mut eof = false;
        set(s, |st| {
            st.state = PlayState::Playing;
            st.pos = start_at;
        });
        loop {
            if s.stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64();
            last = now;
            let paused = s.paused.load(Ordering::Relaxed);
            if !paused {
                played = (played + dt).min(written);
            }
            if let Some(t) = s.seek_to.lock().unwrap_or_else(|e| e.into_inner()).take() {
                start_at = t;
                s.decoder_pid.store(0, Ordering::Relaxed);
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
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            match out.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    if written == 0.0 && start_at == 0.0 {
                        // Say what the decoder said (its last line, bounded).
                        let mut err = Vec::new();
                        if let Some(e) = dec.0.stderr.take() {
                            let _ = e.take(4096).read_to_end(&mut err);
                        }
                        let last = String::from_utf8_lossy(&err)
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .map(|l| gust_core::peek::sanitise_line(l.as_bytes()))
                            .unwrap_or_default();
                        let status = dec.0.wait().map(|s| s.to_string()).unwrap_or_default();
                        return Err(if last.is_empty() {
                            format!("the file could not be decoded as audio ({status})")
                        } else {
                            format!("the file could not be decoded as audio: {last}")
                        });
                    }
                }
                Ok(n) => {
                    // Whole frames only (4 bytes), so channels never swap.
                    let n = n - n % 4;
                    if sink_in.write_all(&buf[..n]).is_err() {
                        return Err("the desktop's audio player stopped".into());
                    }
                    written += n as f64 / BYTES_PER_SEC;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
