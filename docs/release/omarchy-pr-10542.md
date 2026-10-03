# Draft: PR text for basecamp/omarchy #10542 (not sent)

**Title:** Add Gust to the file manager choices

**Body:**

This adds Gust to the list of file managers next to Nautilus, Dolphin,
Thunar, Nemo and PCManFM.

Gust is a Qt Quick file manager written in Rust, built for Omarchy:

- It reads `~/.local/state/omarchy/current/theme/{colors,shell}.toml`, so it
  follows theme switches live. It has no hard-coded colours, and every
  bundled theme passes a 4.5:1 / 3:1 contrast test.
- It never touches the file association at install time.
  `gust --default on|off` switches it and restores exactly what was there
  before, so the existing default-app flow keeps working.
- On Flea's field bench, a 100k-file folder settles in 410 ms against
  2.9–11.7 s for Dolphin, Nautilus and Thunar on the same VM. Data is in
  `bench/RESULTS.md` in the Gust repository.
- It is MIT licensed, a single binary, and depends only on `qt6-base` and
  `qt6-declarative`.

Package: `gust` (AUR). Upstream: <repository URL>.

Change: one entry in the file-manager list, with the package name `gust`
and the desktop id `gust.desktop`. No other Omarchy files change.

Tested on Omarchy with the default theme and the white theme. Install, open
a folder, `gust --default on`, then Super+Shift+F opens Gust.
`gust --default off` restores the previous choice.
