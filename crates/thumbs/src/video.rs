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
        Self::start_worker(path, px, audio::sink_argv)
    }

    #[cfg(test)]
    fn start_with_audio_sink(path: &Path, px: u32, sink: Option<Vec<String>>) -> Self {
        Self::start_worker(path, px, move || sink)
    }

    fn start_worker(
        path: &Path,
        px: u32,
        sink: impl FnOnce() -> Option<Vec<String>> + Send + 'static,
    ) -> Self {
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
                // Tool discovery, including the desktop PCM sink, stays on
                // this worker; tests supply an inert sink through the factory.
                if let Err(error) = run(&path, px, &s, sink()) {
                    if !s.cancel.is_cancelled() {
                        let mut st = s.status.lock().unwrap_or_else(|e| e.into_inner());
                        st.state = PlayState::Failed;
                        st.why = error.to_string();
                        if error == PreviewError::Changed {
                            st.frame = None;
                            st.frame_id = st.frame_id.wrapping_add(1);
                        }
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
        let max = self.status().duration.unwrap_or(f64::MAX);
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

fn run(path: &Path, px: u32, s: &Shared, sink: Option<Vec<String>>) -> Result<(), PreviewError> {
    if px == 0 || px > preview::MAX_PIXELS {
        return Err(PreviewError::InvalidRequest);
    }
    let doc = preview::inspect(path, PreviewKind::Video, &s.cancel)?;
    let scratch = Scratch::new()?;
    let audio = if doc.info().has_audio {
        match sink {
            Some(sink) => Some(audio::Player::start_held(doc.media_input()?, sink)),
            None => None,
        }
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
    let mut decode_start = 0.0;
    let mut tail_retry = false;
    let mut tail_attempts = 0u32;
    let mut tail_deadline = None;
    'decoders: loop {
        helper::check(&s.cancel)?;
        let mut child = OwnedChild::spawn_stream_input(
            &doc.stream_argv(&scratch, decode_start, px)?,
            &doc.media_input()?,
        )?;
        let mut pipe = child.child().stdout.take().ok_or(PreviewError::Helper)?;
        helper::nonblocking(&pipe)?;
        let mut buffer = Vec::new();
        let mut next_frame = 0u64;
        let mut clock = start;
        let mut last = Instant::now();
        let mut last_bytes = Instant::now();
        let mut eof = false;
        let mut revision_check = Instant::now();
        loop {
            helper::check(&s.cancel)?;
            if revision_check.elapsed() >= Duration::from_millis(100) {
                doc.ensure_unchanged()?;
                revision_check = Instant::now();
            }
            if tail_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(PreviewError::Limit);
            }
            let requested_seek = s.seek.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(seconds) = requested_seek {
                start = seconds;
                decode_start = seconds;
                tail_retry = false;
                tail_attempts = 0;
                tail_deadline = None;
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
                if !eof
                    && !tail_retry
                    && matches!(status.state, PlayState::Playing | PlayState::Paused)
                {
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
                st.state = if next_frame == 0 || (tail_retry && !eof) {
                    PlayState::Loading
                } else if ended {
                    PlayState::Ended
                } else if paused {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                };
            }
            if ((paused && next_frame > 0) && !tail_retry)
                || ended
                || eof
                || (!tail_retry && start + next_frame as f64 / 12.0 > clock + 0.05)
            {
                last_bytes = Instant::now();
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            if let Some(len) = frame_length(&buffer)? {
                let png: Vec<u8> = buffer.drain(..len).collect();
                let (width, height) = preview::png_dimensions(&png, px)?;
                doc.ensure_unchanged()?;
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
                    if !buffer.is_empty() {
                        return Err(PreviewError::Output);
                    }
                    match child.child().try_wait().map_err(|_| PreviewError::Helper)? {
                        Some(status) if status.success() => {
                            if next_frame == 0 {
                                // Accurate seeks can discard the final frame,
                                // including throughout a long audio-only tail.
                                // Recover from the video endpoint, not the full
                                // media duration. The final bounded attempt at
                                // zero covers missing/inaccurate stream hints.
                                if start == 0.0 || decode_start == 0.0 || tail_attempts == 8 {
                                    return Err(PreviewError::Output);
                                }
                                let look_behind = 0.25 * 2f64.powi(tail_attempts as i32);
                                let video_end = doc.video_end().unwrap_or(start).min(start);
                                decode_start = if tail_attempts == 7 {
                                    0.0
                                } else {
                                    (video_end - look_behind).max(0.0)
                                };
                                tail_attempts += 1;
                                tail_retry = true;
                                tail_deadline.get_or_insert_with(|| {
                                    Instant::now() + Duration::from_secs(15)
                                });
                                continue 'decoders;
                            }
                            eof = true;
                            if tail_retry {
                                tail_deadline = None;
                            }
                        }
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

    fn fixture(
        directory: &Path,
        video_seconds: f64,
        audio_seconds: Option<f64>,
    ) -> std::path::PathBuf {
        let path = directory.join("clip.mp4");
        let mut command = Command::new("ffmpeg");
        command
            .args(["-nostdin", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc2=size=64x36:rate=12:duration={video_seconds}"
            ));
        if let Some(seconds) = audio_seconds {
            command.args(["-f", "lavfi", "-i"]).arg(format!(
                "sine=frequency=440:sample_rate=48000:duration={seconds}"
            ));
        }
        command.args(["-c:v", "libx264", "-threads", "1", "-preset", "ultrafast"]);
        if audio_seconds.is_some() {
            command.args(["-c:a", "aac"]);
        }
        assert!(command.arg(&path).status().unwrap().success());
        path
    }

    fn wait_until(player: &Player, predicate: impl Fn(&Status) -> bool) -> Status {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let status = player.status();
            if predicate(&status) {
                return status;
            }
            assert_ne!(status.state, PlayState::Failed, "{status:?}");
            assert!(Instant::now() < deadline, "video timeout: {status:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn require_video() -> bool {
        let cap = preview::capability(PreviewKind::Video);
        if cap.available() {
            true
        } else {
            gust_testkit::record_skip(
                "PREVIEWS",
                &gust_testkit::current_test(),
                &format!("video tools missing: {:?}", cap.missing),
            );
            false
        }
    }

    #[test]
    fn unequal_audio_video_lengths_reach_the_video_end() {
        if !require_video() {
            return;
        }
        for (video_seconds, audio_seconds) in [(2.0, 0.5), (1.0, 2.0)] {
            let d = Scratch::new().unwrap();
            let path = fixture(&d.0, video_seconds, Some(audio_seconds));
            let pcm = d.0.join("heard.raw");
            let sink = vec![
                "dd".into(),
                format!("of={}", pcm.display()),
                "status=none".into(),
            ];
            let player = Player::start_with_audio_sink(&path, 64, Some(sink));
            let status = wait_until(&player, |s| s.state == PlayState::Ended);
            assert!(
                status.why.is_empty(),
                "audio must actually play: {status:?}"
            );
            assert!(
                status.pos >= video_seconds.max(audio_seconds) - 0.05,
                "{status:?}"
            );
            assert!(
                status.frame_id >= (video_seconds * 12.0) as u64 - 1,
                "{status:?}"
            );
            let heard = std::fs::metadata(pcm).unwrap().len() as f64 / audio::BYTES_PER_SEC;
            assert!(
                (heard - audio_seconds).abs() < 0.15,
                "PCM sink received {heard} seconds"
            );
        }
    }

    #[test]
    fn paused_end_and_beyond_duration_seeks_recover_the_last_frame() {
        if !require_video() {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = fixture(&d.0, 2.0, None);
        let player = Player::start_with_audio_sink(&path, 64, None);
        let ended = wait_until(&player, |s| s.state == PlayState::Ended);
        let last_png = &ended.frame.as_ref().unwrap().png;
        player.set_paused(true);
        for seconds in [2.0, 200.0] {
            let before = player.status().frame_id;
            player.seek(seconds);
            let status = wait_until(&player, |s| {
                s.state == PlayState::Ended && s.frame_id > before
            });
            assert_eq!(&status.frame.as_ref().unwrap().png, last_png);
            assert!((status.pos - 2.0).abs() < 0.01, "{status:?}");
        }
        let before = player.status().frame_id;
        player.seek(1.95);
        let near_end = wait_until(&player, |s| {
            s.state == PlayState::Paused && s.frame_id > before
        });
        assert_eq!(&near_end.frame.as_ref().unwrap().png, last_png);
        assert!((near_end.pos - 1.95).abs() < 0.01, "{near_end:?}");
        player.set_paused(false);
        wait_until(&player, |s| s.state == PlayState::Ended);
        player.set_paused(true);
        let before = player.status().frame_id;
        player.seek(1.0);
        wait_until(&player, |s| {
            s.state == PlayState::Paused && s.frame_id > before
        });
        player.set_paused(false);
        wait_until(&player, |s| s.state == PlayState::Ended);
    }

    #[test]
    fn long_audio_tail_seeks_keep_the_timeline_and_final_video_frame() {
        if !require_video() {
            return;
        }
        let d = Scratch::new().unwrap();
        let mp4 = fixture(&d.0, 2.0, Some(40.0));
        let mkv = d.0.join("clip.mkv");
        assert!(Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&mp4)
            .args(["-c", "copy"])
            .arg(&mkv)
            .status()
            .unwrap()
            .success());
        for (path, has_stream_duration) in [(&mp4, true), (&mkv, false)] {
            let doc = preview::inspect(path, PreviewKind::Video, &CancelToken::new()).unwrap();
            let duration = doc.info().duration.unwrap();
            assert!((duration - 40.0).abs() < 0.05);
            assert_eq!(doc.video_end().is_some(), has_stream_duration);
            if let Some(end) = doc.video_end() {
                assert!((end - 2.0).abs() < 0.05);
            }
            drop(doc);
            let sink = || {
                Some(vec![
                    "dd".into(),
                    "of=/dev/null".into(),
                    "status=none".into(),
                ])
            };
            // Obtain the actual final PNG through ordinary playback, without
            // waiting for the much longer audio timeline to finish.
            let reference = Player::start_with_audio_sink(path, 64, sink());
            let final_frame = wait_until(&reference, |s| s.frame_id >= 24);
            assert!(final_frame.why.is_empty(), "{final_frame:?}");
            let last_png = final_frame.frame.unwrap().png.clone();
            drop(reference);

            let player = Player::start_with_audio_sink(path, 64, sink());
            player.set_paused(true);
            let first = wait_until(&player, |s| s.state == PlayState::Paused);
            assert_ne!(first.frame.unwrap().png, last_png);
            // This player has never seen the final frame: retaining its old
            // PNG on an empty seek would also fail these assertions.
            for seconds in [duration, duration * 10.0, duration - 0.05, 1.95] {
                let before = player.status().frame_id;
                player.seek(seconds);
                let state = if seconds >= duration {
                    PlayState::Ended
                } else {
                    PlayState::Paused
                };
                let status = wait_until(&player, |s| s.state == state && s.frame_id > before);
                assert!(status.why.is_empty(), "{status:?}");
                assert_eq!(status.duration.unwrap(), duration);
                assert!(
                    (status.pos - seconds.min(duration)).abs() < 0.01,
                    "{status:?}"
                );
                assert_eq!(status.frame.unwrap().png, last_png);
            }
            // Recovery must leave the worker usable for ordinary seeks,
            // running endpoint seeks and resuming the audio-only tail.
            let before = player.status().frame_id;
            player.seek(1.0);
            let ordinary = wait_until(&player, |s| {
                s.state == PlayState::Paused && s.frame_id > before
            });
            assert!((ordinary.pos - 1.0).abs() < 0.01);
            assert_ne!(ordinary.frame.unwrap().png, last_png);
            player.set_paused(false);
            let before = player.status().frame_id;
            player.seek(duration * 10.0);
            let ended = wait_until(&player, |s| {
                s.state == PlayState::Ended && s.frame_id > before
            });
            assert_eq!(ended.frame.unwrap().png, last_png);
            assert_eq!(ended.pos, duration);
            player.set_paused(true);
            let before = player.status().frame_id;
            player.seek(duration - 0.05);
            wait_until(&player, |s| {
                s.state == PlayState::Paused && s.frame_id > before
            });
            player.set_paused(false);
            wait_until(&player, |s| s.state == PlayState::Ended);
        }
    }

    #[test]
    fn paused_video_source_change_refuses_resume_and_invalidates_frame() {
        use std::io::Write;
        if !require_video() {
            return;
        }
        let d = Scratch::new().unwrap();
        let path = fixture(&d.0, 4.0, None);
        let player = Player::start_with_audio_sink(&path, 64, None);
        wait_until(&player, |s| s.frame_id >= 2);
        player.set_paused(true);
        let paused = wait_until(&player, |s| s.state == PlayState::Paused);
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"source changed")
            .unwrap();
        player.set_paused(false);
        let failed = wait_until(&player, |s| s.state == PlayState::Failed);
        assert!(failed.why.contains("changed"), "{failed:?}");
        assert!(failed.frame.is_none());
        assert_eq!(
            failed.frame_id,
            paused.frame_id + 1,
            "only invalidation, no changed-source frame"
        );
    }

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
