//! Embedded video service: sandboxed PNG frames, bounded latest-frame output,
//! and the existing sandboxed PCM service for audio. No GUI/window is launched.

use crate::{
    audio::{self, PlayState},
    helper::{self, OwnedChild, Scratch},
    preview::{self, PreviewError, PreviewKind, RasterPage},
};
use gust_core::CancelToken;
use std::{
    io::Read,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Status {
    pub state: PlayState,
    pub pos: f64,
    pub duration: Option<f64>,
    pub frame: Option<Arc<RasterPage>>,
    pub frame_id: u64,
    pub volume: f32,
    pub muted: bool,
    pub why: String,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            state: PlayState::Loading,
            pos: 0.0,
            duration: None,
            frame: None,
            frame_id: 0,
            volume: 1.0,
            muted: false,
            why: String::new(),
        }
    }
}

struct Shared {
    status: Mutex<Status>,
    seek: Mutex<Option<f64>>,
    paused: AtomicBool,
    volume: AtomicU32,
    muted: AtomicBool,
    cancel: CancelToken,
}

pub struct Player {
    shared: Arc<Shared>,
}
impl Player {
    /// Returns immediately. Poll status on the UI thread; frame data is shared
    /// by Arc, so an unchanged poll never copies a PNG. Only one frame is kept.
    pub fn start(path: &Path, px: u32) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status::default()),
            seek: Mutex::new(None),
            paused: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            cancel: CancelToken::new(),
        });
        let s = shared.clone();
        let path = path.to_owned();
        let result = std::thread::Builder::new()
            .name("gust-video".into())
            .spawn(move || {
                if let Err(error) = run(&path, px, &s) {
                    if !s.cancel.is_cancelled() {
                        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = PlayState::Failed;
                        st.why = error.to_string();
                    }
                }
            });
        if result.is_err() {
            let mut st = shared.status.lock().unwrap_or_else(|e| e.into_inner());
            st.state = PlayState::Failed;
            st.why = "Could not start the video worker".into();
        }
        Self { shared }
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
        if self.status().state == PlayState::Ended {
            self.seek(0.0);
            self.set_paused(false);
        } else {
            self.set_paused(!self.shared.paused.load(Ordering::Relaxed));
        }
    }
    pub fn seek(&self, seconds: f64) {
        if !seconds.is_finite() {
            return;
        }
        let max = self
            .status()
            .duration
            .map(|d| (d - 0.05).max(0.0))
            .unwrap_or(f64::MAX);
        *self.shared.seek.lock().unwrap_or_else(|e| e.into_inner()) = Some(seconds.clamp(0.0, max));
    }
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
    pub fn stop(&self) {
        self.shared.cancel.cancel();
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(path: &Path, px: u32, s: &Shared) -> Result<(), PreviewError> {
    if px == 0 || px > preview::MAX_PIXELS {
        return Err(PreviewError::InvalidRequest);
    }
    let doc = preview::inspect(path, PreviewKind::Video, &s.cancel)?;
    let scratch = Scratch::new()?;
    let audio = if doc.info().has_audio && audio::available().is_ok() {
        Some(audio::Player::start_held(doc.media_input()?))
    } else {
        None
    };
    {
        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
        st.duration = doc.info().duration;
        if doc.info().has_audio && audio.is_none() {
            st.why = audio::available()
                .err()
                .unwrap_or("Audio is unavailable")
                .into();
        }
    }
    let mut start = 0.0;
    'decoders: loop {
        helper::check(&s.cancel)?;
        let mut child = OwnedChild::spawn_input(
            &doc.stream_argv(&scratch, start, px)?,
            &doc.media_input()?,
            Stdio::piped(),
        )?;
        let mut pipe = child.child().stdout.take().ok_or(PreviewError::Helper)?;
        helper::nonblocking(&pipe)?;
        let mut buffer = Vec::new();
        let mut next_frame = 0u64;
        let mut clock = start;
        let mut last = Instant::now();
        let mut last_bytes = Instant::now();
        let mut eof = false;
        loop {
            helper::check(&s.cancel)?;
            let requested_seek = s.seek.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(seconds) = requested_seek {
                start = seconds;
                if let Some(audio) = &audio {
                    audio.seek(seconds);
                }
                continue 'decoders;
            }
            let paused = s.paused.load(Ordering::Relaxed);
            let volume = f32::from_bits(s.volume.load(Ordering::Relaxed));
            let muted = s.muted.load(Ordering::Relaxed);
            let now = Instant::now();
            if !paused && next_frame > 0 {
                clock += now.duration_since(last).as_secs_f64();
            }
            last = now;
            if let Some(audio) = &audio {
                audio.set_paused(paused);
                audio.set_volume(volume);
                audio.set_muted(muted);
                let status = audio.status();
                if matches!(
                    status.state,
                    PlayState::Playing | PlayState::Paused | PlayState::Ended
                ) {
                    clock = status.pos.max(start);
                }
                if status.state == PlayState::Failed {
                    s.status.lock().unwrap_or_else(|e| e.into_inner()).why = status.why;
                }
            }
            let end = doc
                .info()
                .duration
                .unwrap_or(start + next_frame as f64 / 12.0);
            let ended = eof && clock >= end;
            {
                let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                st.pos = clock.min(end);
                st.volume = volume;
                st.muted = muted;
                st.state = if next_frame == 0 {
                    PlayState::Loading
                } else if ended {
                    PlayState::Ended
                } else if paused {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                };
            }
            if (paused && next_frame > 0)
                || ended
                || eof
                || start + next_frame as f64 / 12.0 > clock + 0.05
            {
                last_bytes = Instant::now();
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            if let Some(len) = frame_length(&buffer)? {
                let png: Vec<u8> = buffer.drain(..len).collect();
                let (width, height) = preview::png_dimensions(&png, px)?;
                let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                st.frame = Some(Arc::new(RasterPage {
                    png,
                    width,
                    height,
                    page: 1,
                }));
                st.frame_id = st.frame_id.wrapping_add(1);
                next_frame += 1;
                continue;
            }
            if last_bytes.elapsed() > Duration::from_secs(15) {
                return Err(PreviewError::Limit);
            }
            let mut buf = [0u8; 8192];
            match pipe.read(&mut buf) {
                Ok(0) => {
                    if next_frame == 0 || !buffer.is_empty() {
                        return Err(PreviewError::Output);
                    }
                    match child.child().try_wait().map_err(|_| PreviewError::Helper)? {
                        Some(status) if status.success() => eof = true,
                        Some(_) => return Err(PreviewError::Helper),
                        None => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
                Ok(n) => {
                    if buffer.len().saturating_add(n) > preview::MAX_PNG {
                        return Err(PreviewError::Limit);
                    }
                    buffer.extend_from_slice(&buf[..n]);
                    last_bytes = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(PreviewError::Helper),
            }
        }
    }
}

fn frame_length(bytes: &[u8]) -> Result<Option<usize>, PreviewError> {
    if bytes.len() < 8 {
        return Ok(None);
    }
    if &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(PreviewError::Output);
    }
    let mut offset = 8usize;
    for _ in 0..4096 {
        let Some(head) = bytes.get(offset..offset.saturating_add(8)) else {
            return Ok(None);
        };
        let len =
            u32::from_be_bytes(head[..4].try_into().map_err(|_| PreviewError::Output)?) as usize;
        let end = offset
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .ok_or(PreviewError::Limit)?;
        if end > preview::MAX_PNG {
            return Err(PreviewError::Limit);
        }
        if end > bytes.len() {
            return Ok(None);
        }
        if &head[4..] == b"IEND" {
            return Ok(Some(end));
        }
        offset = end;
    }
    Err(PreviewError::Limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn stream_parser_refuses_hostile_lengths_and_non_png() {
        assert_eq!(frame_length(b"not png at all"), Err(PreviewError::Output));
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(u32::MAX.to_be_bytes());
        bytes.extend(b"IDAT");
        assert_eq!(frame_length(&bytes), Err(PreviewError::Limit));
        assert_eq!(frame_length(b"\x89PNG"), Ok(None));
    }

    #[test]
    fn silent_video_plays_pauses_seeks_and_cancels_without_a_window() {
        let cap = preview::capability(PreviewKind::Video);
        gust_testkit::skip_unless!(
            cap.available(),
            "PREVIEWS",
            "video tools missing: {:?}",
            cap.missing
        );
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
                "testsrc2=size=320x180:rate=12:duration=4",
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
        let p = Player::start(&path, 160);
        let wait = |predicate: &dyn Fn(&Status) -> bool| {
            let until = Instant::now() + Duration::from_secs(8);
            while Instant::now() < until {
                let status = p.status();
                assert_ne!(status.state, PlayState::Failed, "{status:?}");
                if predicate(&status) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("video timeout: {:?}", p.status());
        };
        wait(&|s| s.frame_id >= 3);
        let first = p.status();
        assert_eq!(
            (
                first.frame.as_ref().unwrap().width,
                first.frame.as_ref().unwrap().height
            ),
            (160, 90)
        );
        p.set_paused(true);
        wait(&|s| s.state == PlayState::Paused);
        let paused = p.status();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(paused.frame_id, p.status().frame_id);
        p.set_volume(0.35);
        p.set_muted(true);
        p.seek(f64::NAN);
        p.set_volume(f32::NAN);
        p.seek(2.5);
        wait(&|s| s.frame_id > paused.frame_id && s.pos >= 2.5);
        assert!((p.status().volume - 0.35).abs() < 0.001);
        assert!(p.status().muted);
        p.set_paused(false);
        wait(&|s| s.state == PlayState::Ended);
        p.stop();
        let until = Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&p.shared) > 1 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            Arc::strong_count(&p.shared),
            1,
            "cancelled worker must release helpers and source"
        );
        let p = Player::start(&path, 160);
        p.stop();
        let until = Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&p.shared) > 1 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            Arc::strong_count(&p.shared),
            1,
            "startup cancellation must release worker"
        );
    }
}
