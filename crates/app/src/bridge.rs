//! CXX-Qt bridge: Rust owns the data, QML owns the pixels.
//!
//! Lifetime rules (DESIGN.md §9, plan §9):
//! - the model is created on the GUI thread
//! - worker results are queued back with `CxxQtThread`, and a failed queue
//!   after destruction is HANDLED, never unwrapped
//! - every result carries a generation; stale results are dropped

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qvector.h");
        type QVector_i32 = cxx_qt_lib::QVector<i32>;

        // The base class must be declared through CXX before `#[base = ...]`
        // can name it.
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;
    }

    unsafe extern "RustQt" {
        /// The directory model backing every view.
        // 0.10 requires a bare identifier here, not a string literal.
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        #[qproperty(QString, location)]
        #[qproperty(QString, location_display)]
        #[qproperty(i32, count)]
        #[qproperty(bool, settled)]
        #[qproperty(bool, show_hidden)]
        #[qproperty(QString, status)]
        #[qproperty(QString, sort_field)]
        #[qproperty(bool, sort_ascending)]
        #[qproperty(QString, filter)]
        #[qproperty(i32, cursor)]
        /// Live selection summary, e.g. "3 selected · 2.3 GB". A property
        /// rather than a call so QML BINDS to it and cannot show a stale
        /// count after a selection change some handler forgot to announce.
        #[qproperty(QString, selection_text)]
        /// A persistent capability notice ("previews need bubblewrap").
        /// Separate from status, which describes the last action and is
        /// rewritten by every listing.
        #[qproperty(QString, notice)]
        /// True while a scoped find is running or its results are shown.
        #[qproperty(bool, finding)]
        #[qproperty(QString, find_needle, cxx_name = "findNeedle")]
        type DirModel = super::DirModelRust;

        #[qinvokable]
        fn navigate(self: Pin<&mut DirModel>, path: &QString);
        #[qinvokable]
        fn navigate_up(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn go_back(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn go_forward(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn can_go_back(self: &DirModel) -> bool;
        #[qinvokable]
        fn can_go_forward(self: &DirModel) -> bool;
        #[qinvokable]
        fn reload(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn toggle_hidden(self: Pin<&mut DirModel>);
        /// QML calls this when a view that shows thumbnails becomes active.
        /// The view lives in QML; the model does not duplicate that state.
        #[qinvokable]
        fn request_thumbnails(self: Pin<&mut DirModel>);
        /// Record an EXPLICIT view choice for the current location.
        #[qinvokable]
        fn remember_view(self: Pin<&mut DirModel>, view: &QString);
        /// The view to use here: a remembered choice, else a suggestion,
        /// else an empty string meaning "caller decides".
        #[qinvokable]
        fn view_for_here(self: &DirModel) -> QString;
        #[qinvokable]
        fn set_sort(self: Pin<&mut DirModel>, field: &QString);
        #[qinvokable]
        fn set_filter_text(self: Pin<&mut DirModel>, text: &QString);
        /// Type-ahead: SELECT a row, never start a search (Nautilus #1157).
        #[qinvokable]
        fn type_ahead(self: Pin<&mut DirModel>, prefix: &QString) -> i32;
        #[qinvokable]
        fn move_cursor(self: Pin<&mut DirModel>, delta: i32);
        #[qinvokable]
        fn focus_row(self: Pin<&mut DirModel>, row: i32);
        /// Open a row: returns "dir" (navigated), "file", or "blocked:<why>".
        #[qinvokable]
        fn activate(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Open a terminal in the current folder.
        #[qinvokable]
        fn open_terminal(self: &DirModel) -> QString;
        /// Launch `rclone config` in a terminal. Gust never asks for a cloud
        /// credential itself and never sees one.
        #[qinvokable]
        fn cloud_setup(self: &DirModel) -> QString;

        #[qinvokable]
        fn breadcrumbs(self: &DirModel) -> QStringList;
        #[qinvokable]
        fn entry_name(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn entry_path(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn is_dir(self: &DirModel, row: i32) -> bool;
        /// Label for the delete action at this location — "Move to Trash" or
        /// "Delete permanently…". Never the same string.
        #[qinvokable]
        fn delete_action_label(self: &DirModel) -> QString;
        #[qinvokable]
        fn delete_needs_hard_confirm(self: &DirModel) -> bool;
        /// Machine-readable state for the QA harness. Asserting on this is
        /// exact, unlike OCR of a screenshot.
        #[qinvokable]
        fn state_json(self: &DirModel) -> QString;
        /// Move the entry at `row` to Trash. Returns "" on success or a
        /// human-readable reason. Never falls back to deletion.
        #[qinvokable]
        fn trash_row(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Delete a row permanently. There is no undo, so this is only
        /// reachable behind a confirmation that says so and focuses Cancel.
        /// Returns "" on success or the reason it was refused.
        #[qinvokable]
        fn delete_row_permanently(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Delete every selected item permanently, or the row when nothing
        /// is selected. Returns "" on success or the reason it stopped.
        #[qinvokable]
        fn delete_selection_permanently(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Undo the last trash operation. Returns "" on success or a reason.
        #[qinvokable]
        fn undo_last(self: Pin<&mut DirModel>) -> QString;
        /// Description of what undo would do, or "" when nothing is undoable.
        #[qinvokable]
        fn undo_label(self: &DirModel) -> QString;

        // --- clipboard and operations -----------------------------------
        /// Mark the row for copying. Returns a status line for the UI.
        #[qinvokable]
        fn clip_copy(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Mark the row for moving (cut).
        #[qinvokable]
        fn clip_cut(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Paste into the current folder. Starts a background job and returns
        /// "" on success, or a reason why it could not start.
        #[qinvokable]
        fn clip_paste(self: Pin<&mut DirModel>) -> QString;
        /// What would a drop of the current selection onto `row` do?
        /// Returns the sentence to show the user, or "" for a no-op.
        /// Remember what a drag is carrying, at the moment it starts.
        /// Remote places the user's SSH config mentions, as JSON.
        /// Reading a config is NOT connecting: this opens no socket.
        #[qinvokable]
        fn remote_suggestions(self: &DirModel) -> QString;
        /// Connect a remote place and browse it. The first moment any
        /// network traffic happens, and always a deliberate action.
        #[qinvokable]
        fn remote_connect(self: Pin<&mut DirModel>, alias: QString) -> QString;
        /// Disconnect a remote place.
        #[qinvokable]
        fn remote_disconnect(self: Pin<&mut DirModel>, alias: QString) -> QString;
        /// Cloud remotes the INSTALLED rclone has configured, as JSON.
        /// Reading a config is not connecting: this opens no socket.
        #[qinvokable]
        fn cloud_suggestions(self: &DirModel) -> QString;
        /// Removable devices the kernel reports, as JSON. A local read:
        /// no daemon, no network, and Gust never auto-mounts (DESIGN §6.13).
        #[qinvokable]
        fn devices_json(self: &DirModel) -> QString;
        /// Compress the selection into a .tar.zst in this folder.
        /// Runs through the job machinery, so it is cancellable (DESIGN §6.15).
        #[qinvokable]
        fn compress_selection(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Extract the archive on `row` into this folder. Every entry is
        /// vetted before a byte is written; refusals are reported.
        #[qinvokable]
        fn extract_row(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Is the row something Gust would offer to extract?
        #[qinvokable]
        fn row_is_archive(self: &DirModel, row: i32) -> bool;
        /// Mount a network share. The password is handed to GIO on stdin and
        /// is never stored, logged or placed in argv (DESIGN §6.14).
        #[qinvokable]
        fn share_mount(
            self: Pin<&mut DirModel>,
            address: QString,
            user: QString,
            domain: QString,
            password: QString,
        ) -> QString;
        /// Check an address before asking for a password, so a typo is
        /// caught without the user typing a secret first.
        #[qinvokable]
        fn share_check(self: &DirModel, address: QString) -> QString;
        /// Mount a device and browse it. A visible, deliberate action.
        #[qinvokable]
        fn device_mount(self: Pin<&mut DirModel>, path: QString) -> QString;
        /// Unmount a device. Refuses while a transfer is running.
        #[qinvokable]
        fn device_unmount(self: Pin<&mut DirModel>, path: QString) -> QString;
        /// Mount a configured cloud remote and browse it.
        #[qinvokable]
        fn cloud_connect(self: Pin<&mut DirModel>, name: QString) -> QString;
        /// Unmount a cloud remote. Refuses while uploads are outstanding
        /// unless `force`; never lazy-unmounts a dirty mount.
        #[qinvokable]
        fn cloud_disconnect(self: Pin<&mut DirModel>, name: QString, force: bool) -> QString;
        /// A sentence about outstanding uploads on the current location, or
        /// "" when there is nothing to say. Never claims "uploaded".
        #[qinvokable]
        fn cloud_pending_notice(self: &DirModel) -> QString;
        /// Is a named file confirmed present on the backend at this size?
        /// The ONLY thing that may be shown as "uploaded".
        #[qinvokable]
        fn cloud_confirm_uploaded(self: &DirModel, name: QString) -> bool;
        /// Start a scoped, cancellable find. Results stream in.
        #[qinvokable]
        fn find_start(self: Pin<&mut DirModel>, needle: QString);
        /// Leave the results and go back to the folder being browsed.
        #[qinvokable]
        fn find_clear(self: Pin<&mut DirModel>);
        /// Start a scoped, cancellable CONTENT search. Results stream in as
        /// entries, exactly as a name find does (DESIGN §6.10).
        #[qinvokable]
        fn grep_start(self: Pin<&mut DirModel>, needle: QString);
        /// What Quick Look should show for a row, as JSON. Reads bytes and
        /// metadata only: nothing is executed and no link is followed
        /// (DESIGN §6.9).
        #[qinvokable]
        fn peek_row(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn drag_begin(self: Pin<&mut DirModel>, row: i32);
        #[qinvokable]
        fn drop_preview(self: &DirModel, row: i32, as_move: bool) -> QString;
        /// Perform a drop of the current selection onto `row`. Returns ""
        /// on success, or the reason it was refused.
        #[qinvokable]
        fn drop_on_row(self: Pin<&mut DirModel>, row: i32, as_move: bool) -> QString;
        /// Copy the selection into an absolute destination folder (the other
        /// pane). Returns "" on success or a reason it was refused.
        #[qinvokable]
        fn copy_selection_to(self: Pin<&mut DirModel>, dest: &QString) -> QString;
        /// Move the selection into an absolute destination folder.
        #[qinvokable]
        fn move_selection_to(self: Pin<&mut DirModel>, dest: &QString) -> QString;
        /// Accept a drop of URIs from another application onto `row`.
        #[qinvokable]
        fn drop_uris(self: Pin<&mut DirModel>, row: i32, uris: &QString, as_move: bool) -> QString;
        #[qinvokable]
        fn clip_summary(self: &DirModel) -> QString;
        #[qinvokable]
        fn clip_clear(self: Pin<&mut DirModel>);

        /// Rename the row. Returns "" on success or a readable reason.
        #[qinvokable]
        fn rename_row(self: Pin<&mut DirModel>, row: i32, new_name: &QString) -> QString;
        /// Create a folder in the current location.
        #[qinvokable]
        fn new_folder(self: Pin<&mut DirModel>, name: &QString) -> QString;
        /// Duplicate the row beside itself, e.g. `notes (copy).txt`.
        #[qinvokable]
        fn duplicate_row(self: Pin<&mut DirModel>, row: i32) -> QString;

        /// Poll the running job; refreshes the listing when it finishes.
        /// Returns a JSON snapshot, or "" when no job is running.
        #[qinvokable]
        fn poll_job(self: Pin<&mut DirModel>) -> QString;
        #[qinvokable]
        fn cancel_job(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn job_running(self: &DirModel) -> bool;

        // --- selection ---------------------------------------------------
        /// Toggle the row in the selection (Space, Ctrl+click).
        #[qinvokable]
        fn sel_toggle(self: Pin<&mut DirModel>, row: i32);
        /// Select exactly this row, dropping everything else (plain click).
        #[qinvokable]
        fn sel_only(self: Pin<&mut DirModel>, row: i32);
        /// Extend the selection from the anchor to this row (Shift).
        #[qinvokable]
        fn sel_range_to(self: Pin<&mut DirModel>, row: i32);
        /// Select every VISIBLE entry (Ctrl+A) — never what the filter hides.
        #[qinvokable]
        fn sel_all(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn sel_clear(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn sel_contains(self: &DirModel, row: i32) -> bool;
        #[qinvokable]
        fn sel_count(self: &DirModel) -> i32;
        /// Status-line summary, e.g. "3 selected · 2.3 GB".
        #[qinvokable]
        fn sel_summary(self: &DirModel) -> QString;
    }

    unsafe extern "RustQt" {
        /// # Safety
        /// Inherited beginResetModel. Caller must ensure `end_reset_model`
        /// follows on every path.
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut DirModel>);
        /// # Safety
        /// Inherited endResetModel; only valid after `begin_reset_model`.
        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut DirModel>);

        /// Index of a row, for targeted row updates.
        /// # Safety
        /// Qt-owned; must be called on the GUI thread.
        #[inherit]
        unsafe fn index(
            self: &DirModel,
            row: i32,
            column: i32,
            parent: &QModelIndex,
        ) -> QModelIndex;

        /// Repaint a row span without resetting the model (which would lose
        /// the scroll position and every delegate).
        /// # Safety
        /// Qt-owned; must be called on the GUI thread.
        #[inherit]
        #[cxx_name = "dataChanged"]
        unsafe fn data_changed(
            self: Pin<&mut DirModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QVector_i32,
        );
    }

    // QAbstractListModel overrides.
    unsafe extern "RustQt" {
        #[qinvokable]
        #[cxx_override]
        fn data(self: &DirModel, index: &QModelIndex, role: i32) -> QVariant;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &DirModel, parent: &QModelIndex) -> i32;
        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &DirModel) -> QHash_i32_QByteArray;
    }

    unsafe extern "RustQt" {
        /// QA-only state writer. Writes nothing unless the harness sets the
        /// environment variable naming a destination file, so a normal run has
        /// no extra behaviour at all.
        #[qobject]
        #[qml_element]
        type QaWriter = super::QaWriterRust;

        #[qinvokable]
        fn env_path(self: &QaWriter, var: &QString) -> QString;
        #[qinvokable]
        fn write(self: &QaWriter, path: &QString, contents: &QString) -> bool;
    }

    unsafe extern "RustQt" {
        /// Theme tokens exposed to QML. QML contains no literals.
        #[qobject]
        #[qml_element]
        #[qproperty(QString, name)]
        #[qproperty(bool, is_light)]
        type ThemeTokens = super::ThemeTokensRust;

        #[qinvokable]
        fn color(self: &ThemeTokens, key: &QString) -> QString;
        #[qinvokable]
        fn metric(self: &ThemeTokens, key: &QString) -> f64;
        #[qinvokable]
        fn reload(self: Pin<&mut ThemeTokens>);
        #[qinvokable]
        fn load_theme_dir(self: Pin<&mut ThemeTokens>, dir: &QString, name: &QString);
    }

    // Lets a worker thread queue work back onto the GUI thread.
    impl cxx_qt::Threading for DirModel {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QString, QStringList, QVariant};
use gust_core::entry::{Entry, EntryKind, SortField};
use gust_core::lister::{fill_metadata, list_names, CancelToken};
use gust_core::location::LocationKey;
use gust_core::search::{filter_entries, type_ahead_index};
use gust_core::theme::Tokens;
use std::path::PathBuf;

// Role ids exposed to QML.
const ROLE_NAME: i32 = 0x0100;
const ROLE_KIND: i32 = 0x0101;
const ROLE_SIZE: i32 = 0x0102;
const ROLE_MODIFIED: i32 = 0x0103;
const ROLE_IS_DIR: i32 = 0x0104;
const ROLE_IS_HIDDEN: i32 = 0x0105;
const ROLE_IS_SYMLINK: i32 = 0x0106;
const ROLE_LINK_OK: i32 = 0x0107;
const ROLE_PATH: i32 = 0x0108;
const ROLE_SELECTED: i32 = 0x0109;
const ROLE_THUMB: i32 = 0x010A;

pub struct DirModelRust {
    location: QString,
    location_display: QString,
    count: i32,
    settled: bool,
    show_hidden: bool,
    status: QString,
    selection_text: QString,
    notice: QString,
    sort_field: QString,
    sort_ascending: bool,
    filter: QString,
    cursor: i32,

    key: LocationKey,
    /// Everything listed, before hidden/filter narrowing.
    all: Vec<Entry>,
    /// Row order actually shown.
    visible: Vec<usize>,
    generation: u64,
    back: Vec<LocationKey>,
    forward: Vec<LocationKey>,
    /// Buffers type-ahead keystrokes so "Pro" searches for "Pro".
    type_ahead: gust_core::search::TypeAhead,
    /// The last trashed item plus its display label, for a one-step undo.
    last_trashed_batch: Vec<(gust_ops::trash::TrashedItem, String)>,
    /// What Ctrl+C / Ctrl+X marked, waiting for a paste.
    clipboard: Option<(Vec<std::path::PathBuf>, ClipMode)>,
    /// The job currently running, if any. Only one at a time in M2.
    job: Option<gust_ops::jobs::JobHandle>,
    /// Cancels the listing worker when the user navigates away.
    load_cancel: CancelToken,
    /// name -> cached thumbnail path, for the rows currently listed.
    thumbs: std::collections::HashMap<std::ffi::OsString, String>,
    /// Cancels the thumbnail pass when the location changes.
    thumb_cancel: CancelToken,
    /// What the in-flight drag carries, captured when it started.
    drag_payload: Vec<PathBuf>,
    /// True while a find is running or its results are shown.
    finding: bool,
    find_needle: QString,
    /// Cancels a running find. Escape, a new find, or navigating away.
    find_cancel: CancelToken,
    /// The folder the user was browsing before the find. Results are a
    /// VIEW, not a navigation: leaving them returns here.
    find_origin: Option<LocationKey>,
    /// True while a view that displays thumbnails (Grid) is active. Set by
    /// QML, which owns the view mode.
    thumbs_wanted: bool,
    /// Remembered explicit view choices, persisted between runs.
    views: gust_core::views::ViewMemory,
    /// Selected entry NAMES (DESIGN.md §4.1) — never row indices.
    selection: gust_core::selection::Selection,
    /// The last completed copy/move, kept so Ctrl+Z can ask for its inverse.
    /// Cleared once undone, or superseded by the next job.
    last_job: Option<gust_ops::journal::Job>,
}

/// Whether a clipboard entry will be copied or moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipMode {
    Copy,
    Cut,
}

/// The folder Gust should open, decided once at startup.
///
/// A file manager must honour `gust /some/path` — that is how a desktop
/// "Open containing folder" action invokes it. Anything that is not an
/// existing directory falls back to `$HOME` rather than opening nothing.
pub fn startup_location() -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    };

    // First non-flag argument wins. Flags are reserved for later options.
    let arg = std::env::args_os()
        .skip(1)
        .find(|a| !a.to_string_lossy().starts_with('-'));

    let Some(arg) = arg else { return home() };
    let path = PathBuf::from(&arg);

    // Resolve relative to the working directory the user launched from.
    let abs = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| home())
            .join(path)
    };

    // A stat on a stalled network mount blocks in the kernel, and this runs
    // before the window exists: Gust would never paint at all. Ask a
    // killable child first when the path is the kind that can stall
    // (plan §3 rule 3, DESIGN §6.12).
    {
        use gust_core::stall::{probe_if_risky, Reachability, DEFAULT_PROBE_MS};
        if let Reachability::Stalled { .. } =
            probe_if_risky(&abs, std::time::Duration::from_millis(DEFAULT_PROBE_MS))
        {
            // Start at home rather than not starting. The place is reported
            // once there is a window to report it in.
            return home();
        }
    }

    match std::fs::metadata(&abs) {
        // Pointed at a file: open its folder, which is what the user means.
        Ok(m) if m.is_file() => abs.parent().map(PathBuf::from).unwrap_or_else(home),
        Ok(m) if m.is_dir() => abs,
        _ => home(),
    }
}

impl Default for DirModelRust {
    fn default() -> Self {
        let home = startup_location();
        DirModelRust {
            location: QString::from(&home.to_string_lossy().to_string()),
            location_display: QString::from(&home.to_string_lossy().to_string()),
            count: 0,
            settled: false,
            show_hidden: false,
            status: QString::from("starting"),
            selection_text: QString::from(""),
            notice: QString::from(""),
            sort_field: QString::from("name"),
            sort_ascending: true,
            filter: QString::from(""),
            cursor: 0,
            key: LocationKey::local(&home),
            all: Vec::new(),
            visible: Vec::new(),
            generation: 0,
            back: Vec::new(),
            forward: Vec::new(),
            type_ahead: gust_core::search::TypeAhead::default(),
            last_trashed_batch: Vec::new(),
            clipboard: None,
            job: None,
            load_cancel: CancelToken::new(),
            thumbs: std::collections::HashMap::new(),
            thumb_cancel: CancelToken::new(),
            drag_payload: Vec::new(),
            finding: false,
            find_needle: QString::from(""),
            find_cancel: CancelToken::new(),
            find_origin: None,
            thumbs_wanted: false,
            views: gust_core::views::ViewMemory::default_path()
                .map(|p| gust_core::views::ViewMemory::load(&p))
                .unwrap_or_default(),
            selection: gust_core::selection::Selection::new(),
            last_job: None,
        }
    }
}

impl DirModelRust {
    fn sort_spec(&self) -> gust_core::entry::SortSpec {
        let field = match self.sort_field.to_string().as_str() {
            "size" => SortField::Size,
            "modified" => SortField::Modified,
            "kind" => SortField::Kind,
            _ => SortField::Name,
        };
        gust_core::entry::SortSpec {
            field,
            ascending: self.sort_ascending,
            dirs_first: true,
        }
    }

    /// Recompute which rows are shown, in O(n). Pure — no I/O.
    ///
    /// This used to build a `Vec` of kept pointers and call `contains` for
    /// every entry, which is O(n²): a 100k directory spent minutes here and
    /// the window never finished loading. Index arithmetic keeps it linear.
    fn recompute_visible(&mut self) {
        let needle = self.filter.to_string();
        let show_hidden = self.show_hidden;

        if needle.is_empty() {
            self.visible = self
                .all
                .iter()
                .enumerate()
                .filter(|(_, e)| show_hidden || !e.is_hidden())
                .map(|(i, _)| i)
                .collect();
            return;
        }

        // `filter_entries` returns borrows into `self.all`; turn each one back
        // into its index by pointer offset rather than searching for it.
        let base = self.all.as_ptr();
        let matched = filter_entries(&self.all, &needle);
        let mut idx: Vec<usize> = matched
            .into_iter()
            .filter(|e| show_hidden || !e.is_hidden())
            .map(|e| {
                // Safety: every element of `matched` is a reference into
                // `self.all`, so the offset is in bounds and well defined.
                unsafe { (e as *const Entry).offset_from(base) as usize }
            })
            .collect();
        idx.sort_unstable();
        self.visible = idx;
    }

    fn entry_at(&self, row: i32) -> Option<&Entry> {
        let idx = *self.visible.get(usize::try_from(row).ok()?)?;
        self.all.get(idx)
    }

    /// Local filesystem path of a visible row, if it has one.
    fn path_of_row(&self, row: i32) -> Option<std::path::PathBuf> {
        let name = self.entry_at(row).map(|e| e.name.clone())?;
        self.key.join(&name).as_path().map(|p| p.to_path_buf())
    }
}

/// Label for a clipboard or command target: one name, or a count.
fn clip_label(paths: &[std::path::PathBuf]) -> String {
    match paths.len() {
        0 => "nothing".to_string(),
        1 => display_file_name(&paths[0]),
        n => format!("{n} items"),
    }
}

/// Name shown to the user for a path. Never panics on non-UTF-8.
fn display_file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v >= 10.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_time(mtime: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let age = now - mtime;
    if age < 0 {
        return "future".into();
    }
    let day = 86400;
    if age < day {
        let secs_today = mtime % day;
        format!(
            "today {:02}:{:02}",
            secs_today / 3600,
            (secs_today % 3600) / 60
        )
    } else if age < 2 * day {
        "yesterday".into()
    } else if age < 365 * day {
        format!("{} days ago", age / day)
    } else {
        format!("{} years ago", age / (365 * day))
    }
}

impl qobject::DirModel {
    /// The Qt thread handle, or None before the object is fully constructed.
    fn qt_thread_handle(&self) -> Option<cxx_qt::CxxQtThread<qobject::DirModel>> {
        Some(<qobject::DirModel as cxx_qt::Threading>::qt_thread(self))
    }

    /// Start loading the current location on a WORKER thread.
    ///
    /// The UI thread never touches the filesystem (DESIGN.md §9). Names
    /// arrive first so the window paints immediately, then metadata fills
    /// the columns. Every result is tagged with a generation, so a late
    /// answer for a directory the user already left is dropped.
    fn load_current(mut self: Pin<&mut Self>) {
        let key = self.as_ref().key.clone();
        let gen = self.as_ref().generation + 1;
        let sort = self.as_ref().sort_spec();
        self.as_mut().rust_mut().generation = gen;

        // Cancel whatever the previous navigation was still doing.
        self.as_mut().rust_mut().load_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().load_cancel = cancel.clone();

        let display = key.display_path();
        self.as_mut().set_location(QString::from(&display));
        self.as_mut().set_location_display(QString::from(&display));
        self.as_mut().set_status(QString::from("reading…"));
        self.as_mut().set_settled(false);

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            // No Qt thread yet (constructed before the event loop). Fall back
            // to a synchronous load so startup still works.
            self.load_blocking(key, gen, sort, cancel);
            return;
        };

        std::thread::Builder::new()
            .name("gust-list".into())
            .spawn(move || {
                // A network mount can stop answering, and a readdir on one
                // blocks in uninterruptible sleep: the cancel token cannot
                // reach it and neither can SIGTERM. Ask a killable child
                // first, and only touch the path once it has answered
                // (plan §3 rule 3, DESIGN §6.12).
                if let Some(dir) = key.as_path() {
                    use gust_core::stall::{probe_if_risky, Reachability};
                    let reach = probe_if_risky(
                        dir,
                        std::time::Duration::from_millis(gust_core::stall::DEFAULT_PROBE_MS),
                    );
                    if !reach.is_responsive() {
                        let place = gust_core::stall::place_name_for(dir);
                        let msg = reach.message(&place);
                        let _ = thread.queue(move |mut model| {
                            // The window stays on the folder it could reach.
                            // Moving the user somewhere else because a remote
                            // hung would lose their place for them.
                            model.as_mut().set_settled(true);
                            model.as_mut().set_status(QString::from("Not responding"));
                            model.as_mut().set_notice(QString::from(&msg));
                        });
                        return;
                    }
                }

                // Clean up staging files left by a process that died mid-copy
                // before listing, so crash debris is never shown and never
                // accumulates. Only files whose owner is gone are removed.
                if let Some(dir) = key.as_path() {
                    let _ = gust_ops::engine::sweep_stale_staging(dir);
                }
                let mut snap = list_names(&key, gen, &cancel, |_| {});
                if cancel.is_cancelled() {
                    return;
                }
                snap.sort = sort;
                snap.sort_entries();

                // Publish names first: the window is usable at this point.
                let names = snap.clone();
                let _ = thread.queue(move |model| {
                    Self::apply_snapshot(model, names, false);
                });

                fill_metadata(&mut snap, &cancel);
                if cancel.is_cancelled() {
                    return;
                }
                // Metadata can change sort order (size, modified).
                snap.sort_entries();
                let _ = thread.queue(move |model| {
                    Self::apply_snapshot(model, snap, true);
                });
            })
            .ok();
    }

    /// Produce thumbnails for the current folder on a WORKER thread.
    ///
    /// Runs after the listing has settled, so it never competes with the
    /// work that makes the window usable. Each thumbnail is rendered by a
    /// sandboxed helper (gust-thumbs) and published one at a time, so tiles
    /// fill in progressively instead of arriving in one late batch.
    ///
    /// Leaving the folder cancels the pass: a directory of 500 photos must
    /// not keep a helper running after the user has moved on.
    fn start_thumbnails(mut self: Pin<&mut Self>) {
        if !self.as_ref().thumbs_wanted {
            return;
        }
        if !gust_thumbs::sandbox::sandbox_available() {
            // Say so once, rather than leaving the user to wonder why every
            // tile is a glyph. Untrusted image bytes are only ever decoded
            // inside the sandbox, so without it previews are genuinely
            // unavailable — never "decode them here instead".
            //
            // Only for a folder that actually has something previewable,
            // so browsing a folder of text files says nothing.
            let has_previewable = {
                let r = self.as_ref();
                r.visible.iter().filter_map(|i| r.all.get(*i)).any(|e| {
                    e.kind != EntryKind::Dir && gust_core::mime::thumbnail_mime(&e.name).is_some()
                })
            };
            if has_previewable {
                self.as_mut().set_notice(QString::from(
                    "Previews need bubblewrap — install bwrap to enable them",
                ));
            }
            return;
        }
        let key = self.as_ref().key.clone();
        let gen = self.as_ref().generation;
        let Some(dir) = key.as_path().map(|p| p.to_path_buf()) else {
            return;
        };

        // Candidates: visible, thumbnailable, not a directory.
        let want: Vec<(std::ffi::OsString, &'static str)> = {
            let r = self.as_ref();
            r.visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .filter(|e| e.kind != EntryKind::Dir)
                .filter_map(|e| {
                    gust_core::mime::thumbnail_mime(&e.name).map(|m| (e.name.clone(), m))
                })
                .collect()
        };
        let mut want = want;
        if want.is_empty() {
            return;
        }
        // Bounded: a folder with 50k images must not queue 50k renders.
        want.truncate(400);

        self.as_mut().rust_mut().thumb_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().thumb_cancel = cancel.clone();

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };

        std::thread::Builder::new()
            .name("gust-thumbs".into())
            .spawn(move || {
                for (name, mime) in want {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let src = dir.join(&name);
                    let Ok(path) = gust_thumbs::render::thumbnail(
                        &src,
                        mime,
                        gust_thumbs::cache::ThumbSize::Normal,
                    ) else {
                        // A file we cannot preview keeps its placeholder.
                        // Never a fallback that decodes it in this process.
                        continue;
                    };
                    if cancel.is_cancelled() {
                        return;
                    }
                    let shown = path.to_string_lossy().into_owned();
                    let n = name.clone();
                    let _ = thread.queue(move |model| {
                        Self::publish_thumbnail(model, gen, n, shown);
                    });
                }
            })
            .ok();
    }

    /// A JSON string literal. An alias is arbitrary text from a user's
    /// config file: quoting it by hand would break on the first backslash.
    fn json_str(v: &str) -> String {
        let mut out = String::with_capacity(v.len() + 2);
        out.push('"');
        for c in v.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// Remote places the user's SSH config mentions.
    ///
    /// Reading a config file is not connecting. `ssh -G` is never used
    /// because it EXECUTES `Match exec` blocks; this is a literal parse
    /// (docs/DESIGN.md §6.4).
    fn remote_suggestions(&self) -> QString {
        let Some(cfg) = gust_remote::ssh_config::user_config_path() else {
            return QString::from("[]");
        };
        let hosts = gust_remote::ssh_config::parse_ssh_config(&cfg);
        let mut out = String::from("[");
        for (i, h) in hosts.iter().take(50).enumerate() {
            if i > 0 {
                out.push(',');
            }
            let mounted = gust_remote::mount::mount_point_for(&h.alias)
                .map(|p| gust_remote::mount::is_mounted(&p))
                .unwrap_or(false);
            // Escaped: an alias is arbitrary text from a user's config.
            out.push_str(&format!(
                r#"{{"alias":{},"host":{},"proxy":{},"connected":{}}}"#,
                Self::json_str(&h.alias),
                Self::json_str(h.hostname.as_deref().unwrap_or(&h.alias)),
                h.has_proxy,
                mounted
            ));
        }
        out.push(']');
        QString::from(out)
    }

    // --- cloud remotes (DESIGN §6.8) -------------------------------------

    /// Where a cloud remote is mounted. Under the runtime dir, never in the
    /// user's home, and separate from SSH places so the two cannot collide.
    fn cloud_mount_point(name: &str) -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("TMPDIR").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe = safe.trim_matches('.').to_string();
        if safe.is_empty() {
            return None;
        }
        Some(base.join("gust").join("cloud").join(safe))
    }

    /// The RC address of the mount Gust started for this remote. Loopback
    /// only: the RC is a privileged control surface (plan §9).
    fn cloud_rc_addr(name: &str) -> String {
        // A stable per-remote port derived from the name, in the dynamic
        // range, so two remotes do not fight over one endpoint.
        let mut h: u32 = 2166136261;
        for b in name.as_bytes() {
            h = (h ^ *b as u32).wrapping_mul(16777619);
        }
        format!("http://127.0.0.1:{}", 45000 + (h % 2000) as u16)
    }

    // --- archives (DESIGN §6.15) -----------------------------------------

    fn row_is_archive(&self, row: i32) -> bool {
        self.path_of_row(row)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .map(|n| gust_ops::archive::io::looks_like_archive(&n))
            .unwrap_or(false)
    }

    /// Compress the selection into a .tar.zst beside it.
    ///
    /// Written to a staging name and renamed on success, exactly as a copy
    /// is: an interrupted compress must not leave a half-written archive
    /// that looks finished.
    fn compress_selection(mut self: Pin<&mut Self>, row: i32) -> QString {
        if !gust_ops::archive::io::tool_available() {
            return QString::from(gust_ops::archive::io::ArchiveError::NoTool.message());
        }
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be compressed from.");
        };

        self.as_mut().set_cursor_if_no_selection(row);
        let targets = self.as_ref().target_paths();
        if targets.is_empty() {
            return QString::from("nothing selected");
        }
        let names: Vec<String> = targets
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        if names.is_empty() {
            return QString::from("nothing selected");
        }

        // Named after the single item, or the folder when there are several.
        let stem = if names.len() == 1 {
            names[0].clone()
        } else {
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "archive".into())
        };
        let final_path = gust_ops::archive::io::unique_name(&dir, &format!("{stem}.tar.zst"));
        let staging = dir.join(format!(".gust-compress-{}.tar.zst", std::process::id()));

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return QString::default();
        };
        let gen = self.as_ref().generation;
        let args = gust_ops::archive::io::compress_args(&dir, &staging, &names);

        std::thread::Builder::new()
            .name("gust-compress".into())
            .spawn(move || {
                let out = std::process::Command::new("bsdtar")
                    .args(&args)
                    .stdin(std::process::Stdio::null())
                    .output();
                let message = match out {
                    Ok(o) if o.status.success() => match std::fs::rename(&staging, &final_path) {
                        Ok(()) => format!(
                            "Compressed to {}",
                            final_path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        ),
                        Err(e) => {
                            let _ = std::fs::remove_file(&staging);
                            format!("Could not finish the archive: {e}")
                        }
                    },
                    Ok(o) => {
                        // A failed compress leaves nothing behind.
                        let _ = std::fs::remove_file(&staging);
                        format!(
                            "Could not compress: {}",
                            String::from_utf8_lossy(&o.stderr)
                                .lines()
                                .next_back()
                                .unwrap_or("bsdtar did not say why")
                                .trim()
                        )
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&staging);
                        format!("Could not compress: {e}")
                    }
                };
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().generation == gen {
                        model.as_mut().set_status(QString::from(&message));
                        model.as_mut().reload();
                    }
                });
            })
            .ok();

        self.as_mut().set_status(QString::from("Compressing…"));
        QString::default()
    }

    /// Extract the archive on `row` into the current folder.
    fn extract_row(mut self: Pin<&mut Self>, row: i32) -> QString {
        if !gust_ops::archive::io::tool_available() {
            return QString::from(gust_ops::archive::io::ArchiveError::NoTool.message());
        }
        let Some(archive) = self.as_ref().path_of_row(row) else {
            return QString::from("That item is no longer there.");
        };
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be extracted into.");
        };

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return QString::default();
        };
        let gen = self.as_ref().generation;

        std::thread::Builder::new()
            .name("gust-extract".into())
            .spawn(move || {
                let policy = gust_ops::archive::ExtractPolicy::default();
                let (message, refusals) =
                    match gust_ops::archive::io::extract(&archive, &dir, &policy) {
                        Ok(done) => {
                            let refused = if done.refused.is_empty() {
                                String::new()
                            } else {
                                // Named, not counted: "something was refused"
                                // leaves the user hunting for what.
                                format!("Refused unsafe entries: {}", done.refused.join(", "))
                            };
                            (done.summary(), refused)
                        }
                        Err(e) => (e.message(), String::new()),
                    };
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().generation != gen {
                        return;
                    }
                    model.as_mut().set_status(QString::from(&message));
                    if !refusals.is_empty() {
                        model.as_mut().set_notice(QString::from(&refusals));
                    }
                    model.as_mut().reload();
                });
            })
            .ok();

        self.as_mut().set_status(QString::from("Extracting…"));
        QString::default()
    }

    // --- network shares (DESIGN §6.14) -----------------------------------

    /// Validate an address without asking for a secret.
    ///
    /// Returns "" when it parses, or a sentence about what is wrong. A typo
    /// should be caught before the user types a password, not after.
    fn share_check(&self, address: QString) -> QString {
        use gust_remote::shares::{gvfs_available, parse_address, ShareError, ShareKind};
        if !gvfs_available() {
            return QString::from(ShareError::NoGvfs.message());
        }
        let text = address.to_string();
        match parse_address(&text, Self::share_kind_for(&text)) {
            Ok(_) => QString::default(),
            Err(e) => QString::from(e.message()),
        }
    }

    /// Does this address ask for NFS or SMB?
    ///
    /// Only an explicit `nfs://` means NFS. Guessing from anything else
    /// would send a password to a protocol that has no use for one.
    fn share_kind_for(text: &str) -> gust_remote::shares::ShareKind {
        if text.trim_start().to_lowercase().starts_with("nfs://") {
            gust_remote::shares::ShareKind::Nfs
        } else {
            gust_remote::shares::ShareKind::Smb
        }
    }

    /// Mount a share and browse it.
    ///
    /// The password arrives here, goes straight into GIO's stdin, and is
    /// dropped. It is never stored, never logged, never put in argv, and
    /// never written to Gust's places file — which records the server and
    /// share only.
    fn share_mount(
        mut self: Pin<&mut Self>,
        address: QString,
        user: QString,
        domain: QString,
        password: QString,
    ) -> QString {
        use gust_remote::shares::{mount, parse_address};

        let text = address.to_string();
        let mut share = match parse_address(&text, Self::share_kind_for(&text)) {
            Ok(s) => s,
            Err(e) => return QString::from(e.message()),
        };
        let u = user.to_string();
        if !u.trim().is_empty() {
            share.user = Some(u);
        }
        let d = domain.to_string();
        if !d.trim().is_empty() {
            share.domain = Some(d);
        }

        let runtime =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());

        // Bounded: a server that never answers must not hang the window.
        // This runs on the UI thread only because gio is given a deadline;
        // the listing that follows goes through the normal worker.
        let result = mount(
            &share,
            password.to_string(),
            &runtime,
            std::time::Duration::from_secs(25),
        );

        match result {
            Ok(mp) => {
                let q = QString::from(mp.to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
                QString::default()
            }
            Err(e) => QString::from(e.message()),
        }
    }

    // --- removable devices (DESIGN §6.13) --------------------------------

    fn devices_json(&self) -> QString {
        let found = gust_core::devices::list_devices();
        let can_act = gust_core::devices::udisks_available();
        let mut out = String::from("[");
        for (i, d) in found.iter().take(30).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"name":{},"path":{},"label":{},"state":{},"mountPoint":{},"mounted":{},"empty":{},"canAct":{}}}"#,
                Self::json_str(&d.name),
                Self::json_str(&d.path),
                Self::json_str(&d.display()),
                Self::json_str(&d.state_text()),
                Self::json_str(
                    &d.mount_point
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
                d.is_mounted(),
                d.is_empty_drive(),
                can_act,
            ));
        }
        out.push(']');
        QString::from(out)
    }

    fn device_mount(mut self: Pin<&mut Self>, path: QString) -> QString {
        use gust_core::devices::{mount_args, udisks_available, DeviceError};
        if !udisks_available() {
            return QString::from(DeviceError::NoUdisks.message());
        }
        let dev = path.to_string();
        // The device must be one the kernel actually reports, so a crafted
        // path cannot be handed to udisks.
        if !gust_core::devices::list_devices()
            .iter()
            .any(|d| d.path == dev)
        {
            return QString::from("That device is no longer there.");
        }

        let out = std::process::Command::new("udisksctl")
            .args(mount_args(&dev))
            .stdin(std::process::Stdio::null())
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) => return QString::from(format!("Could not mount: {e}")),
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return QString::from(format!(
                "Could not mount that device: {}",
                err.lines()
                    .last()
                    .unwrap_or("udisks did not say why")
                    .trim()
            ));
        }
        // udisksctl prints "Mounted /dev/sdb1 at /run/media/...".
        let said = String::from_utf8_lossy(&out.stdout);
        if let Some(at) = said.rsplit_once(" at ") {
            let mount = at.1.trim().trim_end_matches('.').to_string();
            if std::path::Path::new(&mount).is_dir() {
                let q = QString::from(mount.as_str());
                self.as_mut().navigate(&q);
                return QString::default();
            }
        }
        // Mounted, but the path could not be read back: say so rather than
        // claiming to have gone somewhere.
        QString::from("Mounted, but Gust could not tell where. Check the sidebar.")
    }

    fn device_unmount(mut self: Pin<&mut Self>, path: QString) -> QString {
        use gust_core::devices::{udisks_available, unmount_args, DeviceError};
        if !udisks_available() {
            return QString::from(DeviceError::NoUdisks.message());
        }
        let dev = path.to_string();

        // Never pull a filesystem out from under a running transfer.
        if self.as_ref().job.is_some() {
            return QString::from(DeviceError::Busy("A transfer".into()).message());
        }

        // If the user is standing on the device, leave before ejecting it.
        let here = std::path::PathBuf::from(self.as_ref().location.to_string());
        let mount = gust_core::devices::list_devices()
            .into_iter()
            .find(|d| d.path == dev)
            .and_then(|d| d.mount_point);
        if let Some(mp) = &mount {
            if here.starts_with(mp) {
                if let Some(home) = std::env::var_os("HOME") {
                    let q =
                        QString::from(std::path::PathBuf::from(home).to_string_lossy().as_ref());
                    self.as_mut().navigate(&q);
                }
            }
        }

        let out = std::process::Command::new("udisksctl")
            .args(unmount_args(&dev))
            .stdin(std::process::Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() => QString::default(),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                QString::from(format!(
                    "Could not eject: {}",
                    err.lines()
                        .last()
                        .unwrap_or("udisks did not say why")
                        .trim()
                ))
            }
            Err(e) => QString::from(format!("Could not eject: {e}")),
        }
    }

    fn cloud_suggestions(&self) -> QString {
        let remotes = match gust_remote::cloud::list_remotes(None) {
            Ok(r) => r,
            // Cloud storage is optional, so a missing or locked rclone is not
            // an error to shout about — but it must still SAY SO. Returning
            // an empty list made "rclone is not installed" look exactly like
            // "you have no cloud storage", which is how a user ends up
            // wondering why a documented feature does nothing (plan §8).
            Err(e) => {
                return QString::from(format!(
                    r#"[{{"unavailable":true,"why":{}}}]"#,
                    Self::json_str(&e.message())
                ))
            }
        };
        let mut out = String::from("[");
        for (i, r) in remotes.iter().take(50).enumerate() {
            if i > 0 {
                out.push(',');
            }
            let connected = Self::cloud_mount_point(&r.name)
                .map(|p| gust_remote::mount::is_mounted(&p))
                .unwrap_or(false);
            out.push_str(&format!(
                r#"{{"name":{},"backend":{},"connected":{}}}"#,
                Self::json_str(&r.name),
                Self::json_str(&r.backend),
                connected
            ));
        }
        out.push(']');
        QString::from(out)
    }

    fn cloud_connect(mut self: Pin<&mut Self>, name: QString) -> QString {
        let name = name.to_string();
        if !gust_remote::cloud::rclone_available() {
            return QString::from(gust_remote::cloud::CloudError::NotInstalled.message());
        }
        // The remote must be one the installed rclone actually has.
        match gust_remote::cloud::list_remotes(None) {
            Ok(remotes) => {
                if !remotes.iter().any(|r| r.name == name) {
                    return QString::from(
                        gust_remote::cloud::CloudError::NoSuchRemote(name).message(),
                    );
                }
            }
            Err(e) => return QString::from(e.message()),
        }

        let Some(mp) = Self::cloud_mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        if gust_remote::mount::is_mounted(&mp) {
            let q = QString::from(mp.to_string_lossy().as_ref());
            self.as_mut().navigate(&q);
            return QString::from("");
        }
        if std::fs::create_dir_all(&mp).is_err() {
            return QString::from("Could not prepare a place for that remote.");
        }

        // --vfs-cache-mode writes is what makes "cached" and "uploaded"
        // different moments; Gust reports the difference rather than hiding
        // it. No credential is passed here — rclone reads its own config.
        let rc = Self::cloud_rc_addr(&name);
        let rc_bind = rc.trim_start_matches("http://").to_string();
        let cache = mp.parent().map(|p| p.join(".cache")).unwrap_or_default();
        let _ = std::fs::create_dir_all(&cache);

        let spawned = std::process::Command::new("rclone")
            .arg("mount")
            .arg(format!("{name}:"))
            .arg(&mp)
            .arg("--vfs-cache-mode")
            .arg("writes")
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--rc")
            .arg("--rc-addr")
            .arg(&rc_bind)
            .arg("--rc-no-auth")
            // NOT --daemon: its readiness handshake reports "daemon exited
            // with error code 1" over a mount that came up perfectly, and
            // would report success over one that did not. The mount point
            // itself is the only trustworthy signal.
            .env("RCLONE_ASK_PASSWORD", "false")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_dir(&mp);
                return QString::from(format!("Could not start rclone: {e}"));
            }
        };

        // A mount appears asynchronously. Wait a bounded moment, watching
        // both the mount point and the child: if rclone gives up first, say
        // so rather than waiting out the whole timeout.
        for _ in 0..60 {
            if gust_remote::mount::is_mounted(&mp) {
                let q = QString::from(mp.to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
                return QString::from("");
            }
            if let Ok(Some(status)) = child.try_wait() {
                let _ = std::fs::remove_dir(&mp);
                return QString::from(format!(
                    "Could not connect to {name}: rclone stopped ({status}). Check it with: rclone lsd {name}:"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        // Still nothing: leave nothing running behind.
        let _ = child.kill();
        let _ = std::fs::remove_dir(&mp);
        QString::from(format!("{name} did not finish connecting."))
    }

    fn cloud_disconnect(mut self: Pin<&mut Self>, name: QString, force: bool) -> QString {
        let name = name.to_string();
        let Some(mp) = Self::cloud_mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        if !gust_remote::mount::is_mounted(&mp) {
            return QString::from(format!("{name} is not connected."));
        }

        // Never lazy-unmount a dirty mount (plan §9). Ask first, and say how
        // much work is outstanding.
        if !force {
            let rc = Self::cloud_rc_addr(&name);
            if let Ok(p) = gust_remote::cloud::vfs_pending(&rc, std::time::Duration::from_secs(3)) {
                if p.has_pending() {
                    let n = p.queued + p.in_progress;
                    // "1 file is" / "3 files are": the verb agrees with the
                    // count, and the whole clause is built rather than
                    // stitched from fragments that can disagree.
                    let subject = if n == 1 {
                        "1 file is".to_string()
                    } else {
                        format!("{n} files are")
                    };
                    return QString::from(format!(
                        "ASK:{subject} still waiting to upload. \
                         Disconnecting now would lose that work."
                    ));
                }
            }
        }

        // If the user is inside the mount, leave before pulling it away.
        let here = std::path::PathBuf::from(self.as_ref().location.to_string());
        if here.starts_with(&mp) {
            if let Some(home) = std::env::var_os("HOME") {
                let q = QString::from(std::path::PathBuf::from(home).to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
            }
        }

        // Navigating away is not instant: the listing worker can still hold
        // the directory open for a moment, and fusermount3 then reports
        // "Device or resource busy" for a mount nobody is really using.
        let mut last = String::new();
        for attempt in 0..8 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            match std::process::Command::new("fusermount3")
                .arg("-u")
                .arg(&mp)
                .output()
            {
                Ok(o) if o.status.success() => {
                    // Only a directory Gust created, under the runtime dir.
                    let _ = std::fs::remove_dir(&mp);
                    return QString::from("");
                }
                Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
                Err(e) => last = e.to_string(),
            }
        }
        // Still busy after several tries. Never lazy-unmount to paper over
        // this (plan §9): something is genuinely using those files, and
        // pulling the mount away underneath it would lose data.
        QString::from(format!(
            "Could not disconnect {name}: {last}. Close anything using those files and try again."
        ))
    }

    fn cloud_pending_notice(&self) -> QString {
        // Which cloud mount, if any, is this location inside?
        let Some(name) = self.cloud_remote_here() else {
            return QString::default();
        };
        let rc = Self::cloud_rc_addr(&name);
        let Ok(p) = gust_remote::cloud::vfs_pending(&rc, std::time::Duration::from_secs(2)) else {
            return QString::default();
        };
        if p.errored > 0 {
            return QString::from(format!(
                "{} file{} failed to upload and {} still only on this computer.",
                p.errored,
                if p.errored == 1 { "" } else { "s" },
                if p.errored == 1 { "is" } else { "are" }
            ));
        }
        if p.has_pending() {
            let n = p.queued + p.in_progress;
            return QString::from(format!(
                "{n} file{} not uploaded yet.",
                if n == 1 { "" } else { "s" }
            ));
        }
        // Deliberately says nothing here. An empty queue is not a receipt,
        // so it must never be turned into "everything is uploaded".
        QString::default()
    }

    fn cloud_confirm_uploaded(&self, name: QString) -> bool {
        let file = name.to_string();
        let Some(remote) = self.cloud_remote_here() else {
            return false;
        };
        let Some(mp) = Self::cloud_mount_point(&remote) else {
            return false;
        };
        let here = std::path::PathBuf::from(self.location.to_string());
        let local = here.join(&file);
        let Ok(size) = std::fs::metadata(&local).map(|m| m.len()) else {
            return false;
        };
        let Ok(rel) = gust_remote::cloud::relative_to_mount(&mp, &here) else {
            return false;
        };
        let subdir = rel.to_string_lossy().to_string();
        match gust_remote::cloud::backend_listing(&remote, &subdir, None) {
            Ok(entries) => gust_remote::cloud::confirmed_on_backend(&entries, &file, size),
            Err(_) => false,
        }
    }

    /// The cloud remote whose mount contains the current location, if any.
    fn cloud_remote_here(&self) -> Option<String> {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("TMPDIR").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("gust")
            .join("cloud");
        let here = std::path::PathBuf::from(self.location.to_string());
        let rest = here.strip_prefix(&base).ok()?;
        rest.components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
    }

    /// Connect a remote place, then browse it.
    ///
    /// Returns "" on success or a sentence explaining what went wrong.
    fn remote_connect(mut self: Pin<&mut Self>, alias: QString) -> QString {
        let alias = alias.to_string();
        let Some(cfg) = gust_remote::ssh_config::user_config_path() else {
            return QString::from("No SSH configuration found.");
        };
        let hosts = gust_remote::ssh_config::parse_ssh_config(&cfg);
        let Some(h) = hosts.iter().find(|h| h.alias == alias) else {
            return QString::from(format!("No saved computer called \"{alias}\"."));
        };
        let Some(mp) = gust_remote::mount::mount_point_for(&alias) else {
            return QString::from("That name cannot be used for a place.");
        };

        if gust_remote::mount::is_mounted(&mp) {
            let q = QString::from(mp.to_string_lossy().as_ref());
            self.as_mut().navigate(&q);
            return QString::from("");
        }

        let mut spec = gust_remote::mount::RemoteSpec::new(
            &alias,
            h.hostname.clone().unwrap_or_else(|| alias.clone()),
        );
        spec.user = h.user.clone();
        spec.port = h.port;
        spec.config_file = Some(cfg);
        // The alias names a whole Host block: identity, known_hosts, port,
        // proxy. Use it verbatim, or all of that is silently dropped.
        spec.use_alias_as_target = true;
        // Where the place opens. Without an explicit folder a connection
        // lands in the login directory, which is that account'"'"'s whole home.
        spec.remote_path = h.remote_path.clone();

        // Blocking, but bounded: sshfs is given a connect timeout and the
        // call cannot outlive it. A stuck server fails instead of freezing.
        match gust_remote::mount::mount(&spec, &mp, std::time::Duration::from_secs(20)) {
            Ok(()) => {
                let q = QString::from(mp.to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
                self.as_mut()
                    .set_status(QString::from(format!("Connected to {alias}")));
                QString::from("")
            }
            Err(e) => QString::from(e.message()),
        }
    }

    fn remote_disconnect(mut self: Pin<&mut Self>, alias: QString) -> QString {
        let alias = alias.to_string();
        let Some(mp) = gust_remote::mount::mount_point_for(&alias) else {
            return QString::from("That name cannot be used for a place.");
        };
        // Leaving the folder first: unmounting what is on screen is how a
        // listing worker ends up reading a filesystem that just vanished.
        if self.as_ref().key.as_path() == Some(mp.as_path()) {
            if let Some(home) = std::env::var_os("HOME") {
                let q = QString::from(std::path::Path::new(&home).to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
            }
        }
        match gust_remote::mount::unmount(&mp) {
            Ok(()) => {
                self.as_mut()
                    .set_status(QString::from(format!("Disconnected from {alias}")));
                QString::from("")
            }
            Err(e) => QString::from(e.message()),
        }
    }

    /// Start a scoped, cancellable find over the current folder and below.
    ///
    /// Results are a VIEW over the model, not a navigation: `find_clear`
    /// returns to the folder the user was browsing. The walk never crosses a
    /// device boundary and never touches a remote place implicitly
    /// (docs/DESIGN.md §6.3).
    fn find_start(mut self: Pin<&mut Self>, needle: QString) {
        let needle = needle.to_string();
        if needle.trim().is_empty() {
            self.as_mut().find_clear();
            return;
        }

        // A second find supersedes the first.
        self.as_ref().find_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().find_cancel = cancel.clone();

        let key = self.as_ref().key.clone();
        // Remember where we came from, but only on the FIRST find: a find
        // started from within results must still return to the real folder.
        if self.as_ref().find_origin.is_none() {
            self.as_mut().rust_mut().find_origin = Some(key.clone());
        }
        let gen = self.as_ref().generation;

        self.as_mut().set_finding(true);
        self.as_mut()
            .set_find_needle(QString::from(needle.as_str()));
        self.as_mut().set_status(QString::from(format!(
            "Searching for \"{needle}\"… Escape to stop"
        )));

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        // 8 levels is deep enough for a project tree and shallow enough that
        // a mistake cannot walk an entire disk.
        let scope = gust_core::search::SearchScope::recursive(key, 8);

        std::thread::Builder::new()
            .name("gust-find".into())
            .spawn(move || {
                let hits = gust_core::search::find_by_name(&scope, &needle, &cancel, |_| {});
                if cancel.is_cancelled() {
                    return;
                }
                let paths: Vec<PathBuf> = hits.into_iter().map(|h| h.path).collect();
                let _ = thread.queue(move |model| {
                    Self::publish_find(model, gen, paths);
                });
            })
            .ok();
    }

    /// Start a content search. Same scope, cancellation and streaming as a
    /// name find; a different question asked of each file.
    fn grep_start(mut self: Pin<&mut Self>, needle: QString) {
        let needle = needle.to_string();
        if needle.trim().is_empty() {
            self.as_mut().find_clear();
            return;
        }

        self.as_ref().find_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().find_cancel = cancel.clone();

        let key = self.as_ref().key.clone();
        if self.as_ref().find_origin.is_none() {
            self.as_mut().rust_mut().find_origin = Some(key.clone());
        }
        let gen = self.as_ref().generation;

        self.as_mut().set_finding(true);
        self.as_mut()
            .set_find_needle(QString::from(needle.as_str()));
        self.as_mut().set_status(QString::from(format!(
            "Searching inside files for \"{needle}\"… Escape to stop"
        )));

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let scope = gust_core::search::SearchScope::recursive(key, 8);

        std::thread::Builder::new()
            .name("gust-grep".into())
            .spawn(move || {
                let (hits, outcome) =
                    gust_core::grep::grep_contents(&scope, &needle, &cancel, |_| {});
                if cancel.is_cancelled() {
                    return;
                }
                let n = hits.len();
                let paths: Vec<PathBuf> = hits.into_iter().map(|h| h.path).collect();
                let _ = thread.queue(move |mut model| {
                    Self::publish_find(model.as_mut(), gen, paths);
                    // Stopping early must be visible: a truncated result that
                    // looks complete is how someone concludes a file is not
                    // there when it is (DESIGN §6.10 rule 3).
                    if outcome == gust_core::grep::GrepOutcome::HitLimit {
                        model.as_mut().set_notice(QString::from(format!(
                            "Stopped after {n} files — narrow the search to see the rest"
                        )));
                    }
                });
            })
            .ok();
    }

    /// What Quick Look shows for a row.
    ///
    /// Bytes and metadata only. Nothing is executed, no preview script runs,
    /// no symlink is followed, and untrusted image bytes are described
    /// rather than decoded here (DESIGN §6.9).
    fn peek_row(&self, row: i32) -> QString {
        let Some(path) = self.path_of_row(row) else {
            return QString::from(r#"{"ok":false}"#);
        };
        let p = gust_core::peek::peek(&path);
        let mut lines = String::from("[");
        for (i, l) in p.text.iter().enumerate() {
            if i > 0 {
                lines.push(',');
            }
            lines.push_str(&Self::json_str(l).to_string());
        }
        lines.push(']');
        QString::from(format!(
            r#"{{"ok":true,"name":{},"kind":{},"kindWords":{},"size":{},"sizeText":{},"mode":{},"lines":{},"truncated":{},"noPreview":{}}}"#,
            Self::json_str(&p.name),
            Self::json_str(p.kind.label()),
            Self::json_str(&p.kind_words),
            p.size,
            Self::json_str(&human_size(p.size)),
            Self::json_str(&p.mode_line),
            lines,
            p.truncated,
            Self::json_str(p.no_preview_reason.as_deref().unwrap_or("")),
        ))
    }

    /// Show what a finished find produced.
    fn publish_find(mut self: Pin<&mut Self>, gen: u64, paths: Vec<PathBuf>) {
        // A find that finished after the user moved on is not shown.
        if self.as_ref().generation != gen || !self.as_ref().finding {
            return;
        }
        // A result is labelled by its path relative to where the search
        // started: two files called "notes.txt" in different folders must
        // not look identical in the list.
        let root = self
            .as_ref()
            .find_origin
            .as_ref()
            .and_then(|k| k.as_path().map(|p| p.to_path_buf()));
        let entries: Vec<Entry> = paths
            .iter()
            .map(|p| {
                let label = root
                    .as_ref()
                    .and_then(|r| p.strip_prefix(r).ok())
                    .unwrap_or(p.as_path())
                    .as_os_str()
                    .to_os_string();
                let kind = if p.is_dir() {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                };
                Entry::new(label, kind)
            })
            .collect();
        let n = entries.len();

        // SAFETY: paired begin/end around the mutation, on the GUI thread —
        // the same contract every other reset in this file follows.
        unsafe {
            self.as_mut().begin_reset_model();
            {
                let mut m = self.as_mut().rust_mut();
                m.all = entries;
                m.visible = (0..n).collect();
                m.cursor = 0;
                m.selection.clear();
            }
            self.as_mut().end_reset_model();
        }
        self.as_mut().set_count(n as i32);
        self.as_mut().set_settled(true);
        self.as_mut().refresh_selection_flags();

        let needle = self.as_ref().find_needle.to_string();
        let msg = if n == 0 {
            format!("Nothing named \"{needle}\" here")
        } else {
            format!(
                "{n} result{} for \"{needle}\" — Escape to go back",
                if n == 1 { "" } else { "s" }
            )
        };
        self.as_mut().set_status(QString::from(msg));
    }

    /// Leave the results and return to the folder being browsed.
    fn find_clear(mut self: Pin<&mut Self>) {
        if !self.as_ref().finding {
            return;
        }
        self.as_ref().find_cancel.cancel();
        self.as_mut().set_finding(false);
        self.as_mut().set_find_needle(QString::from(""));
        // The find never changed `key` — results are a VIEW over the model,
        // not a navigation — so `navigate` to the same path is a no-op and
        // the results would stay on screen. Re-list the current key instead.
        let origin = self.as_mut().rust_mut().find_origin.take();
        match origin.as_ref().and_then(|k| k.as_path()) {
            // If a result was opened, the key DID move; go back deliberately.
            Some(p) if Some(p) != self.as_ref().key.as_path() => {
                let q = QString::from(p.to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
            }
            _ => self.as_mut().reload(),
        }
        self.as_mut().set_status(QString::from("Search cleared"));
    }

    /// Record an explicit view choice and persist it.
    ///
    /// Only ever called for a choice the user actually made. A suggestion
    /// must not reach this, or it would harden into a stored decision.
    fn remember_view(mut self: Pin<&mut Self>, view: &QString) {
        let Some(mode) = gust_core::views::ViewMode::parse(&view.to_string()) else {
            return;
        };
        let key = self.as_ref().key.clone();
        self.as_mut().rust_mut().views.remember(&key, mode);
        // Persist immediately: a preference the user set must survive a
        // crash, and the file is tiny.
        if let Some(path) = gust_core::views::ViewMemory::default_path() {
            let _ = self.as_mut().rust_mut().views.save(&path);
        }
    }

    /// What view should this location use?
    ///
    /// A remembered explicit choice always wins. Otherwise a suggestion, if
    /// this is a location we recognise. An empty string means Gust has no
    /// opinion and the caller keeps whatever it is showing.
    fn view_for_here(&self) -> QString {
        if let Some(v) = self.views.get(&self.key) {
            return QString::from(v.as_str());
        }
        if gust_core::views::suggests_grid(&self.key) {
            return QString::from("grid");
        }
        QString::default()
    }

    /// QML: a thumbnail-showing view became active.
    ///
    /// Safe to call repeatedly — an already-cached folder finishes almost
    /// instantly, and a pass already running is cancelled and restarted
    /// rather than duplicated.
    fn request_thumbnails(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().thumbs_wanted = true;
        if self.as_ref().settled {
            self.as_mut().start_thumbnails();
        }
    }

    /// Hand one finished thumbnail to the model, on the GUI thread.
    ///
    /// Tagged with the generation it was requested for, so a thumbnail that
    /// finishes after the user has navigated away is dropped rather than
    /// painted over the new folder's tiles.
    fn publish_thumbnail(
        mut self: Pin<&mut Self>,
        gen: u64,
        name: std::ffi::OsString,
        path: String,
    ) {
        if self.as_ref().generation != gen {
            return;
        }
        self.as_mut().rust_mut().thumbs.insert(name.clone(), path);

        // Repaint only the row that changed.
        let Some(row) = self
            .as_ref()
            .visible
            .iter()
            .position(|i| self.as_ref().all.get(*i).map(|e| &e.name) == Some(&name))
        else {
            return;
        };
        let mut roles = cxx_qt_lib::QVector::<i32>::default();
        roles.append(ROLE_THUMB);
        unsafe {
            let idx = self
                .as_ref()
                .index(row as i32, 0, &cxx_qt_lib::QModelIndex::default());
            self.as_mut().data_changed(&idx, &idx, &roles);
        }
    }

    /// Synchronous load, used only before the Qt event loop exists.
    fn load_blocking(
        mut self: Pin<&mut Self>,
        key: LocationKey,
        gen: u64,
        sort: gust_core::entry::SortSpec,
        cancel: CancelToken,
    ) {
        let mut snap = list_names(&key, gen, &cancel, |_| {});
        snap.sort = sort;
        snap.sort_entries();
        fill_metadata(&mut snap, &cancel);
        snap.sort_entries();
        Self::apply_snapshot(self.as_mut(), snap, true);
    }

    /// Install a finished snapshot. Runs on the GUI thread only.
    fn apply_snapshot(
        mut self: Pin<&mut Self>,
        snap: gust_core::entry::DirectorySnapshot,
        settled: bool,
    ) {
        // A result from a superseded navigation is discarded.
        if snap.generation != self.as_ref().generation {
            return;
        }

        unsafe { self.as_mut().begin_reset_model() };
        {
            let mut r = self.as_mut().rust_mut();
            r.all = snap.entries;
            r.settled = settled && snap.settled;
        }
        self.as_mut().rust_mut().recompute_visible();
        {
            // A file deleted by another program must not stay selected and
            // become the silent target of the next command.
            let mut r = self.as_mut().rust_mut();
            let names: Vec<std::ffi::OsString> = r
                .visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .map(|e| e.name.clone())
                .collect();
            r.selection.retain_names(&names);
        }
        let n = self.as_ref().visible.len() as i32;
        unsafe { self.as_mut().end_reset_model() };

        let status = match &snap.complete {
            gust_core::entry::Completeness::Failed { error } => format!("cannot open: {error}"),
            _ if !settled => format!("{n} items — reading details…"),
            _ => format!("{n} items"),
        };
        // Order matters: settled and count are set BEFORE status, because a
        // QML observer reacting to any one of these reads all of them. The
        // status string always differs between the two phases, so it is the
        // signal guaranteed to fire last and see a consistent state.
        self.as_mut().set_settled(settled && snap.settled);
        self.as_mut().set_count(n);
        if self.as_ref().cursor >= n {
            self.as_mut().set_cursor(if n > 0 { n - 1 } else { 0 });
        }
        self.as_mut().set_status(QString::from(&status));

        if settled && snap.settled {
            self.as_mut().start_thumbnails();
            self.as_mut().start_cloud_notice();
        }
    }

    /// Ask rclone, on a worker, whether this cloud location has uploads
    /// outstanding, and publish the answer as a notice.
    ///
    /// Never on the UI thread: this shells out to rclone, which talks to a
    /// socket. A stuck remote must not freeze the window (DESIGN §9).
    fn start_cloud_notice(mut self: Pin<&mut Self>) {
        let Some(remote) = self.as_ref().cloud_remote_here() else {
            // Leaving a cloud location clears a notice that no longer
            // applies; a stale "not uploaded yet" is worse than silence.
            if self.as_ref().notice.to_string().contains("upload") {
                self.as_mut().set_notice(QString::default());
            }
            return;
        };
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let rc = Self::cloud_rc_addr(&remote);
        std::thread::spawn(move || {
            let text = match gust_remote::cloud::vfs_pending(&rc, std::time::Duration::from_secs(3))
            {
                Ok(p) if p.errored > 0 => format!(
                    "{} file{} failed to upload and {} still only on this computer",
                    p.errored,
                    if p.errored == 1 { "" } else { "s" },
                    if p.errored == 1 { "is" } else { "are" }
                ),
                Ok(p) if p.has_pending() => {
                    let n = p.queued + p.in_progress;
                    format!("{n} file{} not uploaded yet", if n == 1 { "" } else { "s" })
                }
                // An empty queue is NOT a receipt, so it says nothing at all
                // rather than "everything is uploaded" (DESIGN §6.8).
                Ok(_) => String::new(),
                Err(_) => String::new(),
            };
            let _ = thread.queue(move |mut model| {
                let had = model.as_ref().notice.to_string();
                // Only ever replace an upload notice or an empty one, so
                // this cannot stamp on the bubblewrap capability notice.
                if text.is_empty() {
                    if had.contains("upload") {
                        model.as_mut().set_notice(QString::default());
                    }
                } else {
                    model.as_mut().set_notice(QString::from(&text));
                }
            });
        });
    }

    fn navigate(mut self: Pin<&mut Self>, path: &QString) {
        let p = PathBuf::from(path.to_string());
        // `is_dir()` is a stat, and a stat on a stalled mount blocks the UI
        // thread with no way back. A killable child answers first.
        {
            use gust_core::stall::{
                place_name_for, probe_if_risky, Reachability, DEFAULT_PROBE_MS,
            };
            let reach = probe_if_risky(&p, std::time::Duration::from_millis(DEFAULT_PROBE_MS));
            if let Reachability::Stalled { .. } = reach {
                let msg = reach.message(&place_name_for(&p));
                self.as_mut().set_notice(QString::from(&msg));
                self.as_mut().set_status(QString::from("Not responding"));
                return;
            }
        }
        if !p.is_dir() {
            self.as_mut()
                .set_status(QString::from(&format!("not a directory: {}", p.display())));
            return;
        }
        let old = self.as_ref().key.clone();
        self.as_mut().rust_mut().thumb_cancel.cancel();
        self.as_mut().rust_mut().thumbs.clear();
        {
            let mut r = self.as_mut().rust_mut();
            r.back.push(old);
            r.forward.clear();
            r.key = LocationKey::local(&p);
            r.filter = QString::from("");
            r.cursor = 0;
            // A new folder starts a new type-ahead search.
            r.type_ahead.clear();
            // A stale selection over a different directory is dangerous.
            r.selection.clear();
        }
        self.load_current();
    }

    fn navigate_up(mut self: Pin<&mut Self>) {
        let Some(parent) = self.as_ref().key.parent() else {
            return;
        };
        let old = self.as_ref().key.clone();
        self.as_mut().rust_mut().thumb_cancel.cancel();
        self.as_mut().rust_mut().thumbs.clear();
        {
            let mut r = self.as_mut().rust_mut();
            r.back.push(old);
            r.forward.clear();
            r.key = parent;
            r.cursor = 0;
            r.type_ahead.clear();
            // A stale selection over a different directory is dangerous.
            r.selection.clear();
        }
        self.load_current();
    }

    fn go_back(mut self: Pin<&mut Self>) {
        let Some(prev) = self.as_mut().rust_mut().back.pop() else {
            return;
        };
        let cur = self.as_ref().key.clone();
        {
            let mut r = self.as_mut().rust_mut();
            r.forward.push(cur);
            r.key = prev;
            r.cursor = 0;
            r.type_ahead.clear();
            // A stale selection over a different directory is dangerous.
            r.selection.clear();
        }
        self.load_current();
    }

    fn go_forward(mut self: Pin<&mut Self>) {
        let Some(next) = self.as_mut().rust_mut().forward.pop() else {
            return;
        };
        let cur = self.as_ref().key.clone();
        {
            let mut r = self.as_mut().rust_mut();
            r.back.push(cur);
            r.key = next;
            r.cursor = 0;
            r.type_ahead.clear();
            // A stale selection over a different directory is dangerous.
            r.selection.clear();
        }
        self.load_current();
    }

    fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    fn reload(self: Pin<&mut Self>) {
        self.load_current();
    }

    fn toggle_hidden(mut self: Pin<&mut Self>) {
        let v = !self.as_ref().show_hidden;
        self.as_mut().set_show_hidden(v);
        unsafe { self.as_mut().begin_reset_model() };
        self.as_mut().rust_mut().recompute_visible();
        {
            // A file deleted by another program must not stay selected and
            // become the silent target of the next command.
            let mut r = self.as_mut().rust_mut();
            let names: Vec<std::ffi::OsString> = r
                .visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .map(|e| e.name.clone())
                .collect();
            r.selection.retain_names(&names);
        }
        let n = self.as_ref().visible.len() as i32;
        unsafe { self.as_mut().end_reset_model() };
        self.as_mut().set_count(n);
        self.as_mut()
            .set_status(QString::from(&format!("{n} items")));
    }

    fn set_sort(mut self: Pin<&mut Self>, field: &QString) {
        let f = field.to_string();
        // Clicking the active column flips direction.
        if self.as_ref().sort_field.to_string() == f {
            let asc = !self.as_ref().sort_ascending;
            self.as_mut().set_sort_ascending(asc);
        } else {
            self.as_mut().set_sort_field(QString::from(&f));
            self.as_mut().set_sort_ascending(true);
        }
        let spec = self.as_ref().sort_spec();
        unsafe { self.as_mut().begin_reset_model() };
        {
            let mut r = self.as_mut().rust_mut();
            gust_core::entry::sort_entries(&mut r.all, spec);
        }
        self.as_mut().rust_mut().recompute_visible();
        unsafe { self.as_mut().end_reset_model() };
    }

    fn set_filter_text(mut self: Pin<&mut Self>, text: &QString) {
        self.as_mut().set_filter(QString::from(&text.to_string()));
        unsafe { self.as_mut().begin_reset_model() };
        self.as_mut().rust_mut().recompute_visible();
        {
            // A file deleted by another program must not stay selected and
            // become the silent target of the next command.
            let mut r = self.as_mut().rust_mut();
            let names: Vec<std::ffi::OsString> = r
                .visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .map(|e| e.name.clone())
                .collect();
            r.selection.retain_names(&names);
        }
        let n = self.as_ref().visible.len() as i32;
        unsafe { self.as_mut().end_reset_model() };
        self.as_mut().set_count(n);
        self.as_mut().set_cursor(0);
        let t = text.to_string();
        let status = if t.is_empty() {
            format!("{n} items")
        } else {
            format!("{n} of {} match \"{}\"", self.as_ref().all.len(), t)
        };
        self.as_mut().set_status(QString::from(&status));
    }

    fn type_ahead(mut self: Pin<&mut Self>, prefix: &QString) -> i32 {
        let typed = prefix.to_string();
        let mut ch_iter = typed.chars();
        let (Some(ch), None) = (ch_iter.next(), ch_iter.next()) else {
            // A multi-character string is treated as an explicit prefix and
            // does not go through the keystroke buffer.
            return self.type_ahead_prefix(&typed, false);
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Buffer keystrokes: typing "Pro" searches for "Pro", not for "o".
        let (search, cycle) = self.as_mut().rust_mut().type_ahead.push(ch, now);
        self.type_ahead_prefix(&search, cycle)
    }

    /// Shared implementation: select the row matching `prefix`.
    fn type_ahead_prefix(mut self: Pin<&mut Self>, prefix: &str, cycle: bool) -> i32 {
        let entries: Vec<Entry> = self
            .as_ref()
            .visible
            .iter()
            .filter_map(|i| self.as_ref().all.get(*i).cloned())
            .collect();
        // Only a repeated key continues from the current row; a new prefix
        // always searches from the top so the first match wins.
        let from = if cycle {
            usize::try_from(self.as_ref().cursor).ok()
        } else {
            None
        };
        match type_ahead_index(&entries, prefix, from) {
            Some(i) => {
                let row = i as i32;
                self.as_mut().set_cursor(row);
                row
            }
            None => -1,
        }
    }

    fn move_cursor(mut self: Pin<&mut Self>, delta: i32) {
        let n = self.as_ref().visible.len() as i32;
        if n == 0 {
            return;
        }
        let next = (self.as_ref().cursor + delta).clamp(0, n - 1);
        self.as_mut().set_cursor(next);
    }

    fn focus_row(mut self: Pin<&mut Self>, row: i32) {
        let n = self.as_ref().visible.len() as i32;
        let clamped = if n == 0 { 0 } else { row.clamp(0, n - 1) };
        self.as_mut().set_cursor(clamped);
    }

    fn activate(mut self: Pin<&mut Self>, row: i32) -> QString {
        // Copy out everything needed before borrowing `self` mutably.
        let Some((name, kind, mode)) = self
            .as_ref()
            .entry_at(row)
            .map(|e| (e.name.clone(), e.kind, e.meta.map(|m| m.mode)))
        else {
            return QString::from("blocked:no such row");
        };
        let target = self.as_ref().key.join(&name);

        if kind == EntryKind::Dir {
            self.navigate(&QString::from(&target.display_path()));
            return QString::from("dir");
        }
        // A symlink to a directory still navigates, but only after resolving.
        if kind == EntryKind::Symlink {
            if let Some(p) = target.as_path() {
                if p.is_dir() {
                    self.navigate(&QString::from(&target.display_path()));
                    return QString::from("dir");
                }
            }
        }
        // Executables never launch from a plain activation.
        if gust_core::mime::needs_trusted_launch(&name, mode) {
            return QString::from("blocked:executable needs a deliberate Run action");
        }

        // Hand the file to the desktop's own opener. Gust does not decide
        // what the file is by running it, and does not run it itself.
        let Some(path) = target.as_path() else {
            return QString::from("blocked:this location is not a local file");
        };
        match gust_core::open::open_with_desktop(path, mode) {
            Ok(_opener) => QString::from("file"),
            Err(e) => QString::from(&format!("blocked:{e}")),
        }
    }

    /// Launch rclone's own configuration tool. Every provider's setup
    /// differs and Drive's OAuth is not a template for Proton or Filen
    /// (plan §3), so Gust delegates rather than reimplementing any of it.
    fn cloud_setup(&self) -> QString {
        if !gust_remote::cloud::rclone_available() {
            return QString::from(gust_remote::cloud::CloudError::NotInstalled.message());
        }
        let (bin, args) = gust_remote::cloud::config_command();
        match gust_core::open::run_in_terminal(bin, &args) {
            Ok(_) => QString::default(),
            Err(_) => QString::from(
                "No terminal found to run rclone config in. Run it yourself, then use :cloud again.",
            ),
        }
    }

    /// Open a terminal in the current folder.
    fn open_terminal(&self) -> QString {
        let Some(dir) = self.key.as_path() else {
            return QString::from("this location is not a local folder");
        };
        match gust_core::open::open_terminal_here(dir) {
            Ok(term) => QString::from(&format!("Opened {term}")),
            Err(e) => QString::from(&format!("{e}")),
        }
    }

    fn breadcrumbs(&self) -> QStringList {
        let mut list = QStringList::default();
        let Some(path) = self.key.as_path() else {
            return list;
        };
        let mut acc = PathBuf::from("/");
        list.append(QString::from("/"));
        for c in path.components().skip(1) {
            acc.push(c);
            list.append(QString::from(&acc.to_string_lossy().to_string()));
        }
        list
    }

    fn entry_name(&self, row: i32) -> QString {
        match self.entry_at(row) {
            Some(e) => QString::from(&e.display_name()),
            None => QString::default(),
        }
    }

    fn entry_path(&self, row: i32) -> QString {
        match self.entry_at(row) {
            Some(e) => QString::from(&self.key.join(&e.name).display_path()),
            None => QString::default(),
        }
    }

    fn is_dir(&self, row: i32) -> bool {
        self.entry_at(row)
            .map(|e| e.kind == EntryKind::Dir)
            .unwrap_or(false)
    }

    /// Delete everything the confirmation described.
    ///
    /// Trash acts on the whole selection, so permanent delete must too —
    /// otherwise confirming "4 selected items" removes one and leaves three,
    /// which is the opposite of the promise the dialog just made.
    fn delete_selection_permanently(mut self: Pin<&mut Self>, row: i32) -> QString {
        if !self.as_ref().delete_needs_hard_confirm() {
            return QString::from("Trash is available here — nothing was deleted.");
        }
        let Some(folder) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be deleted from.");
        };

        self.as_mut().set_cursor_if_no_selection(row);
        let targets = self.as_ref().target_paths();
        if targets.is_empty() {
            return QString::from("That item is already gone.");
        }

        // Stop at the first refusal but KEEP what already went, and say how
        // far it got: silently continuing past a refusal would delete things
        // the guard just rejected.
        let mut total = gust_ops::permanent::Removed::default();
        let mut refusal: Option<String> = None;
        for t in &targets {
            match gust_ops::permanent::delete_permanently(&folder, t) {
                Ok(r) => {
                    total.files += r.files;
                    total.dirs += r.dirs;
                }
                Err(e) => {
                    refusal = Some(e.message());
                    break;
                }
            }
        }

        self.as_mut().rust_mut().selection.clear();
        self.as_mut().reload();

        match refusal {
            Some(why) if total.total() == 0 => QString::from(why),
            Some(why) => QString::from(format!("{} {why}", total.summary())),
            None => {
                let msg = total.summary();
                self.as_mut().set_status(QString::from(msg));
                QString::from("")
            }
        }
    }

    /// Delete permanently. The one operation with no journal entry.
    fn delete_row_permanently(mut self: Pin<&mut Self>, row: i32) -> QString {
        // Never on the strength of a row index alone: Trash must genuinely
        // be unavailable here, or this path should not have been reachable.
        if !self.as_ref().delete_needs_hard_confirm() {
            return QString::from("Trash is available here — nothing was deleted.");
        }
        let Some(folder) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be deleted from.");
        };
        let Some(target) = self.as_ref().path_of_row(row) else {
            return QString::from("That item is already gone.");
        };

        match gust_ops::permanent::delete_permanently(&folder, &target) {
            Ok(removed) => {
                let msg = removed.summary();
                self.as_mut().reload();
                self.as_mut().set_status(QString::from(msg));
                QString::from("")
            }
            Err(e) => QString::from(e.message()),
        }
    }

    fn delete_action_label(&self) -> QString {
        let cap = match self.key.as_path() {
            Some(p) => gust_ops::trash::TrashDir::home()
                .map(|t| gust_ops::trash::trash_capability(p, &t))
                .unwrap_or(gust_core::location::Cap::No),
            None => gust_core::location::Cap::Unknown,
        };
        QString::from(gust_ops::trash::action_label(cap))
    }

    fn delete_needs_hard_confirm(&self) -> bool {
        let cap = match self.key.as_path() {
            Some(p) => gust_ops::trash::TrashDir::home()
                .map(|t| gust_ops::trash::trash_capability(p, &t))
                .unwrap_or(gust_core::location::Cap::No),
            None => gust_core::location::Cap::Unknown,
        };
        gust_ops::trash::is_destructive_without_recovery(cap)
    }

    /// Exact state for QA assertions — no OCR guesswork.
    fn state_json(&self) -> QString {
        let names: Vec<String> = self
            .visible
            .iter()
            .filter_map(|i| self.all.get(*i))
            .take(40)
            .map(|e| e.display_name())
            .collect();
        let cursor_name = self
            .visible
            .get(usize::try_from(self.cursor).unwrap_or(0))
            .and_then(|i| self.all.get(*i))
            .map(|e| e.display_name())
            .unwrap_or_default();
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let list = names
            .iter()
            .map(|n| format!("\"{}\"", esc(n)))
            .collect::<Vec<_>>()
            .join(",");
        QString::from(&format!(
            concat!(
                "{{\"location\":\"{}\",\"count\":{},\"cursor\":{},",
                "\"cursor_name\":\"{}\",\"filter\":\"{}\",\"show_hidden\":{},",
                "\"sort_field\":\"{}\",\"sort_ascending\":{},\"settled\":{},",
                "\"status\":\"{}\",\"visible\":[{}]}}"
            ),
            esc(&self.location.to_string()),
            self.count,
            self.cursor,
            esc(&cursor_name),
            esc(&self.filter.to_string()),
            self.show_hidden,
            esc(&self.sort_field.to_string()),
            self.sort_ascending,
            self.settled,
            esc(&self.status.to_string()),
            list
        ))
    }

    /// Move one entry to Trash, recording it so undo can restore it.
    ///
    /// Refuses (with a reason) when trash is unsupported for the location —
    /// a permanent delete is never silently substituted.
    fn trash_row(mut self: Pin<&mut Self>, row: i32) -> QString {
        self.as_mut().set_cursor_if_no_selection(row);
        let paths = self.as_ref().target_paths();
        if paths.is_empty() {
            return QString::from("nothing to move to Trash");
        }
        let Some(trash) = gust_ops::trash::TrashDir::home() else {
            return QString::from("no trash directory available");
        };

        // Trash each target, stopping at the first failure but KEEPING what
        // already succeeded — and recording it, so undo restores all of it.
        let mut done: Vec<(gust_ops::trash::TrashedItem, String)> = Vec::new();
        let mut failure: Option<String> = None;
        for p in &paths {
            let label = display_file_name(p);
            match gust_ops::trash::trash_path(p, &trash) {
                Ok(item) => done.push((
                    item,
                    gust_core::location::escape_for_display(std::ffi::OsStr::new(&label)),
                )),
                Err(e) => {
                    failure = Some(format!("{label}: {e}"));
                    break;
                }
            }
        }

        let moved = done.len();
        if moved > 0 {
            let mut r = self.as_mut().rust_mut();
            r.last_trashed_batch = done;
            // One level of undo: the newest action owns it.
            r.last_job = None;
        }
        self.as_mut().rust_mut().selection.clear();
        self.as_mut().reload();

        match failure {
            // Partial work is reported, never silently swallowed.
            Some(why) if moved > 0 => {
                QString::from(&format!("Moved {moved}, then stopped — {why}"))
            }
            Some(why) => QString::from(&why),
            None => QString::default(),
        }
    }

    fn undo_last(mut self: Pin<&mut Self>) -> QString {
        // A completed copy/move is more recent than any trash batch it
        // followed, so it is undone first.
        if let Some(job) = self.as_ref().last_job.clone() {
            return self.undo_job(&job);
        }

        let batch = self.as_ref().last_trashed_batch.clone();
        if batch.is_empty() {
            return QString::from("nothing to undo");
        }
        let Some(trash) = gust_ops::trash::TrashDir::home() else {
            return QString::from("no trash directory available");
        };

        // Restore every item. A refusal for one must not abandon the rest,
        // and must not be hidden.
        let mut restored = 0usize;
        let mut refusals: Vec<String> = Vec::new();
        let mut keep: Vec<(gust_ops::trash::TrashedItem, String)> = Vec::new();
        for (item, label) in batch {
            match gust_ops::trash::restore(&item, &trash) {
                Ok(()) => restored += 1,
                Err(e) => {
                    refusals.push(format!("{label}: {e}"));
                    keep.push((item, label));
                }
            }
        }
        // Anything still in the trash stays undoable.
        self.as_mut().rust_mut().last_trashed_batch = keep;
        self.as_mut().reload();

        if refusals.is_empty() {
            QString::default()
        } else if restored > 0 {
            QString::from(&format!(
                "Restored {restored}; {} could not be restored: {}",
                refusals.len(),
                refusals.join("; ")
            ))
        } else {
            QString::from(&refusals.join("; "))
        }
    }

    /// Reverse a completed copy/move, re-verifying before anything is removed.
    fn undo_job(mut self: Pin<&mut Self>, job: &gust_ops::journal::Job) -> QString {
        let plan = gust_ops::undo::plan_undo(job);
        if let gust_ops::undo::UndoPlan::Refused(reason) = &plan {
            // The record stays: the user may still undo it once whatever
            // changed is dealt with. Refusing is not forgetting.
            return QString::from(&format!("{reason}"));
        }

        let outcome = gust_ops::undo::apply_undo(&plan);
        let verb = match job.kind {
            gust_ops::journal::JobKind::Move => "move",
            _ => "copy",
        };
        // Only forget the job once it is fully undone; a partial undo stays
        // available so the rest can be retried.
        if outcome.is_complete() {
            self.as_mut().rust_mut().last_job = None;
        }
        self.as_mut().reload();

        if outcome.is_complete() {
            QString::default()
        } else if outcome.steps_done > 0 {
            QString::from(&format!(
                "Undid part of the {verb} ({}/{}) — {}",
                outcome.steps_done,
                outcome.steps_total,
                outcome.problems.join("; ")
            ))
        } else {
            QString::from(&format!(
                "Could not undo the {verb}: {}",
                outcome.problems.join("; ")
            ))
        }
    }

    fn undo_label(&self) -> QString {
        if let Some(job) = &self.last_job {
            let n = job.entries.iter().filter(|e| e.done).count();
            let noun = if n == 1 { "item" } else { "items" };
            return QString::from(&match job.kind {
                gust_ops::journal::JobKind::Move => format!("Undo: move {n} {noun} back"),
                _ => format!("Undo: remove the {n} copied {noun}"),
            });
        }
        match self.last_trashed_batch.len() {
            0 => QString::default(),
            1 => QString::from(&format!("Undo: restore {}", self.last_trashed_batch[0].1)),
            n => QString::from(&format!("Undo: restore {n} items")),
        }
    }

    // --- clipboard ------------------------------------------------------

    fn clip_copy(mut self: Pin<&mut Self>, row: i32) -> QString {
        self.as_mut().set_cursor_if_no_selection(row);
        let paths = self.as_ref().target_paths();
        if paths.is_empty() {
            return QString::from("nothing to copy");
        }
        let label = clip_label(&paths);
        self.as_mut().rust_mut().clipboard = Some((paths, ClipMode::Copy));
        QString::from(&format!("Copied {label} — paste with Ctrl+V"))
    }

    fn clip_cut(mut self: Pin<&mut Self>, row: i32) -> QString {
        self.as_mut().set_cursor_if_no_selection(row);
        let paths = self.as_ref().target_paths();
        if paths.is_empty() {
            return QString::from("nothing to cut");
        }
        let label = clip_label(&paths);
        self.as_mut().rust_mut().clipboard = Some((paths, ClipMode::Cut));
        QString::from(&format!("Cut {label} — paste with Ctrl+V"))
    }

    /// A command invoked on a row acts on that row only when nothing is
    /// selected; an explicit selection always wins.
    fn set_cursor_if_no_selection(mut self: Pin<&mut Self>, row: i32) {
        if self.as_ref().selection.is_empty() {
            self.as_mut().set_cursor(row);
        }
    }

    fn clip_summary(&self) -> QString {
        match &self.clipboard {
            Some((paths, mode)) => QString::from(&format!(
                "{} {}",
                match mode {
                    ClipMode::Copy => "Copy",
                    ClipMode::Cut => "Move",
                },
                clip_label(paths)
            )),
            None => QString::default(),
        }
    }

    fn clip_clear(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().clipboard = None;
    }

    fn clip_paste(mut self: Pin<&mut Self>) -> QString {
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let Some((srcs, mode)) = self.as_ref().clipboard.clone() else {
            return QString::from("nothing to paste");
        };
        let Some(dest) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot paste here");
        };
        // Drop anything that vanished since it was copied.
        let sources: Vec<PathBuf> = srcs.into_iter().filter(|p| p.exists()).collect();
        if sources.is_empty() {
            self.as_mut().rust_mut().clipboard = None;
            return QString::from("the copied items no longer exist");
        }
        // Pasting a folder into itself would build an infinite tree.
        if sources.iter().any(|s| dest.starts_with(s)) {
            return QString::from("cannot paste a folder into itself");
        }

        let req = match mode {
            ClipMode::Copy => gust_ops::jobs::Request::Copy {
                sources,
                dest_dir: dest,
            },
            ClipMode::Cut => gust_ops::jobs::Request::Move {
                sources,
                dest_dir: dest,
            },
        };
        // Ask policy: an existing name surfaces a conflict instead of a
        // silent overwrite.
        let id = format!("job-{}", gust_ops::journal::now_unix());
        let handle = gust_ops::jobs::start(id, req, gust_ops::journal::ConflictPolicy::Ask);
        {
            let mut r = self.as_mut().rust_mut();
            r.job = Some(handle);
            // A new operation supersedes the previous undo.
            r.last_job = None;
            r.last_trashed_batch.clear();
            if mode == ClipMode::Cut {
                // A cut is consumed by its paste; a copy stays available.
                r.clipboard = None;
            }
        }
        QString::default()
    }

    // --- drag and drop ---------------------------------------------------

    /// Remember what this drag carries, at the moment it starts.
    ///
    /// Resolving the sources later would read whatever is under the pointer
    /// — which by then is the DROP TARGET, so every drop refused itself as
    /// "cannot be dropped into itself".
    fn drag_begin(mut self: Pin<&mut Self>, row: i32) {
        let dragged = self.as_ref().path_of_row(row);
        // The selection travels ONLY if the row being dragged is part of it.
        // Dragging an unselected row carries that row, not a stale selection
        // from an earlier click — which would move something the user never
        // touched, and (when the stale item was the drop target) refuse the
        // drop as "cannot be dropped into itself".
        let sel = self.as_ref().target_paths();
        let sources = match &dragged {
            Some(d) if sel.iter().any(|s| s == d) => sel,
            Some(d) => vec![d.clone()],
            None => Vec::new(),
        };
        self.as_mut().rust_mut().drag_payload = sources;
    }

    /// What the in-flight drag is carrying.
    fn drag_sources(&self, _row: i32) -> Vec<PathBuf> {
        self.drag_payload.clone()
    }

    /// Where a drop onto `row` lands, and whether that is a directory.
    fn drop_target(&self, row: i32) -> Option<(PathBuf, bool)> {
        if row < 0 {
            // Empty space in the view: the folder being shown.
            return self.key.as_path().map(|p| (p.to_path_buf(), true));
        }
        let idx = *self.visible.get(row as usize)?;
        let e = self.all.get(idx)?;
        let path = self.key.join(&e.name).as_path()?.to_path_buf();
        Some((path, e.kind == EntryKind::Dir))
    }

    fn plan_for(&self, row: i32, as_move: bool) -> gust_core::drop::DropPlan {
        use gust_core::drop::{plan_drop, DropAction};
        let Some((target, is_dir)) = self.drop_target(row) else {
            return gust_core::drop::DropPlan::Refused("cannot drop here".into());
        };
        let action = if as_move {
            DropAction::Move
        } else {
            DropAction::Copy
        };
        plan_drop(&self.drag_sources(row), &target, is_dir, action)
    }

    /// Describe the drop without doing it, so the user is told the
    /// operation and the destination BEFORE releasing the button.
    fn drop_preview(&self, row: i32, as_move: bool) -> QString {
        use gust_core::drop::DropPlan;
        match self.plan_for(row, as_move) {
            DropPlan::Perform { description, .. } => QString::from(&description),
            DropPlan::Refused(why) => QString::from(&why),
            DropPlan::Nothing => QString::default(),
        }
    }

    fn drop_on_row(mut self: Pin<&mut Self>, row: i32, as_move: bool) -> QString {
        use gust_core::drop::{DropAction, DropPlan};
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let plan = self.as_ref().plan_for(row, as_move);
        let (action, sources, destination) = match plan {
            DropPlan::Perform {
                action,
                sources,
                destination,
                ..
            } => (action, sources, destination),
            DropPlan::Refused(why) => return QString::from(&why),
            DropPlan::Nothing => return QString::default(),
        };

        // Exactly the same engine, journal and undo as a paste. A drop is
        // never a shortcut around the safe path.
        let req = match action {
            DropAction::Copy => gust_ops::jobs::Request::Copy {
                sources,
                dest_dir: destination,
            },
            DropAction::Move => gust_ops::jobs::Request::Move {
                sources,
                dest_dir: destination,
            },
        };
        let id = format!("job-{}", gust_ops::journal::now_unix());
        let handle = gust_ops::jobs::start(id, req, gust_ops::journal::ConflictPolicy::Ask);
        {
            let mut r = self.as_mut().rust_mut();
            r.job = Some(handle);
            r.last_job = None;
            r.last_trashed_batch.clear();
        }
        QString::default()
    }

    /// The shared half of "send this selection over there". The destination
    /// comes from the other pane, so it is a real folder the user is looking
    /// at — but it is still checked, because a pane can be showing a place
    /// that has gone away underneath it.
    fn send_selection_to(mut self: Pin<&mut Self>, dest: &QString, as_move: bool) -> QString {
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let sources = self.as_ref().target_paths();
        if sources.is_empty() {
            return QString::from("nothing selected");
        }
        let dest_dir = std::path::PathBuf::from(dest.to_string());
        if !dest_dir.is_dir() {
            return QString::from("that folder is no longer there");
        }
        // Refuse a no-op and refuse copying a folder into itself; the same
        // questions a drop asks, asked the same way (DESIGN §6.1).
        for src in &sources {
            if src == &dest_dir {
                return QString::from("that is the destination");
            }
            if let Some(parent) = src.parent() {
                if parent == dest_dir.as_path() {
                    return QString::from("already there");
                }
            }
            if dest_dir.starts_with(src) {
                return QString::from("a folder cannot be copied into itself");
            }
        }

        let req = if as_move {
            gust_ops::jobs::Request::Move { sources, dest_dir }
        } else {
            gust_ops::jobs::Request::Copy { sources, dest_dir }
        };
        let id = format!("job-{}", gust_ops::journal::now_unix());
        let handle = gust_ops::jobs::start(id, req, gust_ops::journal::ConflictPolicy::Ask);
        {
            let mut r = self.as_mut().rust_mut();
            r.job = Some(handle);
            r.last_job = None;
            r.last_trashed_batch.clear();
        }
        QString::default()
    }

    fn copy_selection_to(self: Pin<&mut Self>, dest: &QString) -> QString {
        self.send_selection_to(dest, false)
    }

    fn move_selection_to(self: Pin<&mut Self>, dest: &QString) -> QString {
        self.send_selection_to(dest, true)
    }

    fn drop_uris(mut self: Pin<&mut Self>, row: i32, uris: &QString, as_move: bool) -> QString {
        use gust_core::drop::{paths_from_uri_list, plan_drop, DropAction, DropPlan};
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        // Only URIs we can resolve to real local paths. Dragged text from
        // another application is not a file operation.
        let sources = paths_from_uri_list(&uris.to_string());
        if sources.is_empty() {
            return QString::from("nothing droppable there");
        }
        let Some((target, is_dir)) = self.as_ref().drop_target(row) else {
            return QString::from("cannot drop here");
        };
        let action = if as_move {
            DropAction::Move
        } else {
            DropAction::Copy
        };
        match plan_drop(&sources, &target, is_dir, action) {
            DropPlan::Perform {
                action,
                sources,
                destination,
                ..
            } => {
                let req = match action {
                    DropAction::Copy => gust_ops::jobs::Request::Copy {
                        sources,
                        dest_dir: destination,
                    },
                    DropAction::Move => gust_ops::jobs::Request::Move {
                        sources,
                        dest_dir: destination,
                    },
                };
                let id = format!("job-{}", gust_ops::journal::now_unix());
                let handle = gust_ops::jobs::start(id, req, gust_ops::journal::ConflictPolicy::Ask);
                let mut r = self.as_mut().rust_mut();
                r.job = Some(handle);
                r.last_job = None;
                r.last_trashed_batch.clear();
                QString::default()
            }
            DropPlan::Refused(why) => QString::from(&why),
            DropPlan::Nothing => QString::default(),
        }
    }

    // --- direct operations ----------------------------------------------

    fn rename_row(mut self: Pin<&mut Self>, row: i32, new_name: &QString) -> QString {
        let Some(path) = self.as_ref().path_of_row(row) else {
            return QString::from("no such row");
        };
        let name = std::ffi::OsString::from(new_name.to_string());
        match gust_ops::recursive::rename_in_place(&path, &name, false) {
            Ok(_) => {
                self.as_mut().reload();
                QString::default()
            }
            Err(gust_ops::engine::OpError::Conflict(_)) => {
                QString::from("a file with that name already exists")
            }
            Err(e) => QString::from(&format!("{e}")),
        }
    }

    fn new_folder(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot create a folder here");
        };
        let raw = std::ffi::OsString::from(name.to_string());
        if let Err(e) = gust_ops::recursive::validate_name(&raw) {
            return QString::from(&format!("{e}"));
        }
        let target = dir.join(&raw);
        if target.exists() {
            return QString::from("a file with that name already exists");
        }
        match std::fs::create_dir(&target) {
            Ok(()) => {
                self.as_mut().reload();
                QString::default()
            }
            Err(e) => QString::from(&format!("{e}")),
        }
    }

    fn duplicate_row(mut self: Pin<&mut Self>, row: i32) -> QString {
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let Some(src) = self.as_ref().path_of_row(row) else {
            return QString::from("no such row");
        };
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot duplicate here");
        };
        let Some(base) = src.file_name().map(|n| n.to_os_string()) else {
            return QString::from("no file name");
        };
        // Resolve a free "(copy)" name through the parent's descriptor.
        let Ok((parent, _)) = gust_ops::anchor::resolve_parent(&dir.join("probe")) else {
            return QString::from("cannot read this folder");
        };
        let Ok(new_name) = gust_ops::recursive::unique_copy_name(&parent, &base) else {
            return QString::from("cannot find a free name");
        };
        let dest_path = dir.join(&new_name);

        // CopyAs lands the copy directly under its "(copy)" name — no
        // temporary name, so a crash cannot leave a half-named duplicate.
        let id = format!("dup-{}", gust_ops::journal::now_unix());
        let handle = gust_ops::jobs::start(
            id,
            gust_ops::jobs::Request::CopyAs {
                source: src,
                dest_path,
            },
            gust_ops::journal::ConflictPolicy::Ask,
        );
        self.as_mut().rust_mut().job = Some(handle);
        QString::default()
    }

    // --- job lifecycle ---------------------------------------------------

    fn job_running(&self) -> bool {
        self.job.is_some()
    }

    // --- selection --------------------------------------------------------

    /// The entries the user can currently see, in display order.
    fn visible_entries(&self) -> Vec<&Entry> {
        self.visible
            .iter()
            .filter_map(|i| self.all.get(*i))
            .collect()
    }

    /// Publish the selection summary and repaint the selected flags.
    ///
    /// Every selection mutation funnels through here, so the displayed count
    /// cannot drift from the real one — three separate handlers forgot to
    /// announce a change before this was centralised.
    fn refresh_selection_flags(mut self: Pin<&mut Self>) {
        let summary = self.as_ref().sel_summary();
        self.as_mut().set_selection_text(summary);

        let n = self.as_ref().visible.len() as i32;
        if n == 0 {
            return;
        }
        let root = cxx_qt_lib::QModelIndex::default();
        // Safety: all four calls run on the GUI thread, inside an invokable.
        unsafe {
            let top = self.as_ref().index(0, 0, &root);
            let bottom = self.as_ref().index(n - 1, 0, &root);
            let mut roles = cxx_qt_lib::QVector::<i32>::default();
            roles.append(ROLE_SELECTED);
            self.as_mut().data_changed(&top, &bottom, &roles);
        }
    }

    fn sel_toggle(mut self: Pin<&mut Self>, row: i32) {
        let Some(name) = self.as_ref().entry_at(row).map(|e| e.name.clone()) else {
            return;
        };
        self.as_mut().rust_mut().selection.toggle(&name);
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_cursor(row);
    }

    fn sel_only(mut self: Pin<&mut Self>, row: i32) {
        let Some(name) = self.as_ref().entry_at(row).map(|e| e.name.clone()) else {
            return;
        };
        self.as_mut().rust_mut().selection.select_only(&name);
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_cursor(row);
    }

    fn sel_range_to(mut self: Pin<&mut Self>, row: i32) {
        // Collect the names first so nothing borrows `self` while the
        // selection is mutated. Cloning names is cheap next to a range op.
        let names: Vec<std::ffi::OsString> = self
            .as_ref()
            .visible_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        self.as_mut().rust_mut().selection.select_range_by_name(
            &names,
            row.max(0) as usize,
            gust_core::selection::RangeMode::Replace,
        );
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_cursor(row);
    }

    fn sel_all(mut self: Pin<&mut Self>) {
        let names: Vec<std::ffi::OsString> = self
            .as_ref()
            .visible_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        self.as_mut()
            .rust_mut()
            .selection
            .select_all_by_name(&names);
        self.as_mut().refresh_selection_flags();
    }

    fn sel_clear(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().selection.clear();
        self.as_mut().refresh_selection_flags();
    }

    fn sel_contains(&self, row: i32) -> bool {
        self.entry_at(row)
            .map(|e| self.selection.contains(&e.name))
            .unwrap_or(false)
    }

    fn sel_count(&self) -> i32 {
        self.selection.len() as i32
    }

    fn sel_summary(&self) -> QString {
        let bytes: u64 = self
            .visible_entries()
            .iter()
            .filter(|e| self.selection.contains(&e.name))
            .filter_map(|e| e.meta.map(|m| m.size))
            .sum();
        QString::from(&self.selection.summary(bytes))
    }

    /// The paths a command should act on.
    ///
    /// The selection when there is one, otherwise the cursor row. Resolved
    /// ONCE, when the command is issued (DESIGN.md §4.1).
    fn target_paths(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            return self.path_of_row(self.cursor).into_iter().collect();
        }
        let visible = self.visible_entries();
        self.selection
            .resolve(&visible)
            .iter()
            .filter_map(|e| self.key.join(&e.name).as_path().map(|p| p.to_path_buf()))
            .collect()
    }

    fn cancel_job(self: Pin<&mut Self>) {
        if let Some(job) = self.job.as_ref() {
            job.cancel();
        }
    }

    fn poll_job(mut self: Pin<&mut Self>) -> QString {
        let Some(snap) = self.as_ref().job.as_ref().map(|j| j.snapshot()) else {
            return QString::default();
        };

        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let json = format!(
            concat!(
                "{{\"id\":\"{}\",\"state\":\"{:?}\",\"done\":{},\"total\":{},",
                "\"bytes_done\":{},\"bytes_total\":{},\"message\":\"{}\",\"refused\":{}}}"
            ),
            esc(&snap.id),
            snap.state,
            snap.entries_done,
            snap.entries_total,
            snap.bytes_done,
            snap.bytes_total,
            esc(&snap.message),
            snap.refused.len()
        );

        if snap.is_finished() || snap.state == gust_ops::journal::JobState::Conflict {
            // Keep the record of a CLEANLY finished job so Ctrl+Z can ask for
            // its inverse. A cancelled or failed job is not offered an undo:
            // half an operation has no well-defined opposite.
            let record = self.as_ref().job.as_ref().map(|j| j.record());
            {
                let mut r = self.as_mut().rust_mut();
                r.job = None;
                r.last_job = match record {
                    Some(rec) if rec.offers_undo() => Some(rec),
                    _ => None,
                };
            }
            self.as_mut().reload();
        }
        QString::from(&json)
    }

    fn data(&self, index: &cxx_qt_lib::QModelIndex, role: i32) -> QVariant {
        let Some(e) = self.entry_at(index.row()) else {
            return QVariant::default();
        };
        match role {
            ROLE_NAME => QVariant::from(&QString::from(&e.display_name())),
            ROLE_KIND => QVariant::from(&QString::from(e.kind.as_str())),
            ROLE_SIZE => {
                let s = match e.meta {
                    Some(m) if e.kind != EntryKind::Dir => human_size(m.size),
                    Some(_) => String::new(),
                    None => "…".into(),
                };
                QVariant::from(&QString::from(&s))
            }
            ROLE_MODIFIED => {
                let s = match e.meta {
                    Some(m) => human_time(m.mtime),
                    None => "…".into(),
                };
                QVariant::from(&QString::from(&s))
            }
            ROLE_IS_DIR => QVariant::from(&(e.kind == EntryKind::Dir)),
            ROLE_IS_HIDDEN => QVariant::from(&e.is_hidden()),
            ROLE_IS_SYMLINK => QVariant::from(&(e.kind == EntryKind::Symlink)),
            ROLE_LINK_OK => QVariant::from(&e.meta.and_then(|m| m.link_ok).unwrap_or(true)),
            ROLE_PATH => QVariant::from(&QString::from(&self.key.join(&e.name).display_path())),
            ROLE_SELECTED => QVariant::from(&self.selection.contains(&e.name)),
            ROLE_THUMB => {
                // Only ever a CACHED thumbnail. This runs on the GUI thread,
                // so it must not render, and must not even stat the source
                // beyond what `cached` already does.
                let url = self
                    .thumbs
                    .get(&e.name)
                    .map(|p| QString::from(&format!("file://{p}")))
                    .unwrap_or_default();
                QVariant::from(&url)
            }
            _ => QVariant::default(),
        }
    }

    fn row_count(&self, _parent: &cxx_qt_lib::QModelIndex) -> i32 {
        self.visible.len() as i32
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut h = QHash::<QHashPair_i32_QByteArray>::default();
        h.insert(ROLE_NAME, QByteArray::from("name"));
        h.insert(ROLE_KIND, QByteArray::from("kind"));
        h.insert(ROLE_SIZE, QByteArray::from("sizeText"));
        h.insert(ROLE_MODIFIED, QByteArray::from("modifiedText"));
        h.insert(ROLE_IS_DIR, QByteArray::from("isDir"));
        h.insert(ROLE_IS_HIDDEN, QByteArray::from("isHidden"));
        h.insert(ROLE_IS_SYMLINK, QByteArray::from("isSymlink"));
        h.insert(ROLE_LINK_OK, QByteArray::from("linkOk"));
        h.insert(ROLE_PATH, QByteArray::from("fullPath"));
        h.insert(ROLE_SELECTED, QByteArray::from("isSelected"));
        h.insert(ROLE_THUMB, QByteArray::from("thumbUrl"));
        h
    }
}

/// QA-only state writer (see the bridge declaration).
#[derive(Default)]
pub struct QaWriterRust;

impl qobject::QaWriter {
    /// Read an environment variable, returning "" when unset.
    fn env_path(&self, var: &QString) -> QString {
        match std::env::var(var.to_string()) {
            Ok(v) => QString::from(&v),
            Err(_) => QString::default(),
        }
    }

    /// Write a file atomically. Refuses anything outside a temp directory so
    /// this probe can never be pointed at the user's real files.
    fn write(&self, path: &QString, contents: &QString) -> bool {
        let p = std::path::PathBuf::from(path.to_string());
        if p.as_os_str().is_empty() {
            return false;
        }
        let allowed = p.starts_with("/tmp/") || p.starts_with("/var/tmp/");
        if !allowed {
            eprintln!(
                "gust: refusing QA state write outside /tmp: {}",
                p.display()
            );
            return false;
        }
        let tmp = p.with_extension("partial");
        if std::fs::write(&tmp, contents.to_string()).is_err() {
            return false;
        }
        std::fs::rename(&tmp, &p).is_ok()
    }
}

pub struct ThemeTokensRust {
    name: QString,
    is_light: bool,
    tokens: Tokens,
}

impl Default for ThemeTokensRust {
    fn default() -> Self {
        let t = gust_core::theme::load_current();
        ThemeTokensRust {
            name: QString::from(&t.name),
            is_light: t.is_light(),
            tokens: t,
        }
    }
}

impl qobject::ThemeTokens {
    fn color(&self, key: &QString) -> QString {
        QString::from(self.tokens.color(&key.to_string()))
    }

    fn metric(&self, key: &QString) -> f64 {
        self.tokens.metric(&key.to_string())
    }

    fn reload(mut self: Pin<&mut Self>) {
        let t = gust_core::theme::load_current();
        let name = QString::from(&t.name);
        let light = t.is_light();
        self.as_mut().rust_mut().tokens = t;
        self.as_mut().set_name(name);
        self.as_mut().set_is_light(light);
    }

    fn load_theme_dir(mut self: Pin<&mut Self>, dir: &QString, name: &QString) {
        let t = gust_core::theme::load_from_theme_dir(
            std::path::Path::new(&dir.to_string()),
            &name.to_string(),
        );
        let n = QString::from(&t.name);
        let light = t.is_light();
        self.as_mut().rust_mut().tokens = t;
        self.as_mut().set_name(n);
        self.as_mut().set_is_light(light);
    }
}
