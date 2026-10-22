//! One viewer for the visible images, audio and video (DESIGN §6.18.1).

use crate::entry::{Entry, EntryKind};
use crate::mime::{category_for_name, Category};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// A snapshot of the displayed media order, independent of later model changes.
#[derive(Debug, PartialEq, Eq)]
pub struct Playlist {
    paths: Vec<PathBuf>,
    start: usize,
}

impl Playlist {
    /// Uses cached names/types only; no directory reads or metadata requests.
    pub fn from_entries<'a>(
        directory: &Path,
        entries: impl IntoIterator<Item = &'a Entry>,
        selected: &OsStr,
    ) -> Option<Self> {
        if !directory.is_absolute() {
            return None;
        }
        let mut paths = Vec::new();
        let mut start = None;
        for entry in entries {
            if entry.kind != EntryKind::File
                || !matches!(
                    category_for_name(&entry.name),
                    Category::Image | Category::Audio | Category::Video
                )
            {
                continue;
            }
            if entry.name == selected {
                start = Some(paths.len());
            }
            paths.push(directory.join(&entry.name));
        }
        Some(Self {
            paths,
            start: start?,
        })
    }

    fn command(&self, player: &Path) -> Command {
        let mut command = Command::new(player);
        command.args([
            "--no-terminal",
            "--force-window=yes",
            "--image-display-duration=inf",
            "--keep-open=always",
            // EOF must not carry an automatic pause into neighbouring media.
            "--keep-open-pause=no",
            "--loop-playlist=no",
            "--shuffle=no",
            "--autocreate-playlist=no",
            "--input-terminal=no",
            "--title=${filename} — Gust · ${playlist-pos-1}/${playlist-count}",
            "--osd-playing-msg=${filename} (${playlist-pos-1}/${playlist-count})",
            "--input-commands-append=keybind LEFT 'playlist-prev weak'",
            "--input-commands-append=keybind RIGHT 'playlist-next weak'",
            "--input-commands-append=keybind Shift+LEFT 'seek -5'",
            "--input-commands-append=keybind Shift+RIGHT 'seek 5'",
            "--input-commands-append=keybind ESC quit",
            "--playlist=-",
        ]);
        command.arg(format!("--playlist-start={}", self.start));
        command
    }

    /// Spawn and feed the player on a worker. The caller reaps the child.
    pub fn launch(&self, player: &Path) -> io::Result<Child> {
        let mut child = self
            .command(player)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // URI encoding keeps a newline, '#', '%' or non-UTF-8 byte inside
        // one filename, rather than creating another playlist entry.
        let result = self.write_m3u(child.stdin.take().expect("piped stdin"));
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    }

    fn write_m3u(&self, mut writer: impl Write) -> io::Result<()> {
        writer.write_all(b"#EXTM3U\n")?;
        for path in &self.paths {
            writeln!(writer, "{}", crate::clipboard::path_to_uri(path))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn mixed_media_keeps_display_order_and_selected_position() {
        let entries = [
            Entry::new("z.MP4".into(), EntryKind::File),
            Entry::new("skip.txt".into(), EntryKind::File),
            Entry::new("folder.png".into(), EntryKind::Dir),
            Entry::new("song.m4a".into(), EntryKind::File),
            Entry::new("app.AppImage".into(), EntryKind::File),
            Entry::new("photo.HEIC".into(), EntryKind::File),
            Entry::new("link.jpg".into(), EntryKind::Symlink),
            Entry::new("pipe.wav".into(), EntryKind::Special),
        ];
        let playlist =
            Playlist::from_entries(Path::new("/media"), &entries, OsStr::new("song.m4a")).unwrap();
        assert_eq!(playlist.start, 1);
        assert_eq!(
            playlist.paths,
            ["/media/z.MP4", "/media/song.m4a", "/media/photo.HEIC"].map(PathBuf::from)
        );
        assert!(
            Playlist::from_entries(Path::new("/media"), &entries, OsStr::new("skip.txt")).is_none()
        );
        assert!(
            Playlist::from_entries(Path::new("/media"), &entries, OsStr::new("absent.png"))
                .is_none()
        );
        assert!(
            Playlist::from_entries(Path::new("relative"), &entries, OsStr::new("z.MP4")).is_none()
        );
    }

    #[test]
    fn playlist_round_trips_hostile_and_raw_names_without_extra_entries() {
        let entries = [
            Entry::new("- image #1\n; $(touch nope) %.png".into(), EntryKind::File),
            Entry::new(
                OsString::from_vec(b"raw-\xff.jpg".to_vec()),
                EntryKind::File,
            ),
        ];
        let playlist =
            Playlist::from_entries(Path::new("/media"), &entries, &entries[1].name).unwrap();
        let mut bytes = Vec::new();
        playlist.write_m3u(&mut bytes).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().count(), 3);
        let decoded: Vec<_> = text
            .lines()
            .skip(1)
            .map(|line| crate::clipboard::uri_to_path(line).unwrap())
            .collect();
        assert_eq!(decoded, playlist.paths);
        let command = playlist.command(Path::new("mpv"));
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert!(args.contains(&"--playlist-start=1"));
        assert!(args.contains(&"--input-commands-append=keybind LEFT 'playlist-prev weak'"));
        assert!(args.contains(&"--input-commands-append=keybind RIGHT 'playlist-next weak'"));
        assert!(!args.iter().any(|a| a.contains("touch nope")));
    }
}
