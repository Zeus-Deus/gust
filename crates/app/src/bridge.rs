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
        /// Fixed personal Home root, independent of the folder being browsed.
        #[qproperty(QString, home_path)]
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
        /// Why the list has no rows when it is not simply an empty folder
        /// ("This folder no longer exists", "Permission denied"), or "".
        /// The views show it instead of "This folder is empty", which would
        /// be a lie about a folder Gust could not read.
        #[qproperty(QString, empty_reason)]
        /// Columns view side panes, as JSON: the parent folder's entries
        /// (current one marked) and a peek at the item under the cursor.
        /// Filled on a worker after request_columns().
        #[qproperty(QString, columns_side)]
        #[qproperty(QString, peek_visual)]
        #[qproperty(QString, delete_total)]
        /// True while a scoped find is running or its results are shown.
        #[qproperty(bool, finding)]
        #[qproperty(QString, find_needle, cxx_name = "findNeedle")]
        /// True while a mount, connect, eject or disconnect is running on a
        /// worker. The window stays responsive; this only drives a spinner.
        #[qproperty(bool, place_busy)]
        /// Bumped whenever details (size, date, mode) arrive for rows that
        /// scrolled into view, so anything showing the cursor row's details
        /// (status line, Properties, the QA probe) can refresh.
        #[qproperty(i32, meta_rev)]
        /// SHA-256 of the file Quick Look shows: "" before asking,
        /// "…" while reading, the hex digest, or why not.
        #[qproperty(QString, peek_hash)]
        /// Bumped when folder-jump candidates have been gathered.
        #[qproperty(i32, jump_rev)]
        /// Tab completion result: "<request>\n<completed text>", or "" .
        #[qproperty(QString, jump_completion)]
        /// Every place Gust could connect to, as JSON, built on a worker by
        /// request_places(): SSH config hosts, Tailscale devices and rclone
        /// remotes, each with whether it is connected. Reading these opens
        /// no connection to any of them (DESIGN §6.4.1).
        #[qproperty(QString, places_json)]
        #[qproperty(QString, sharing_json)]
        #[qproperty(bool, sharing_busy)]
        #[qproperty(bool, sharing_checking)]
        type DirModel = super::DirModelRust;

        #[qsignal]
        fn sharing_done(
            self: Pin<&mut DirModel>,
            action: QString,
            success: bool,
            message: QString,
            path: QString,
        );
        #[qinvokable]
        fn request_sharing(self: Pin<&mut DirModel>);
        /// Capture native selection paths before opening the receiver picker.
        #[qinvokable]
        fn prepare_device_send(self: Pin<&mut DirModel>, row: i32) -> QString;
        #[qinvokable]
        fn device_folder(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn device_share(self: Pin<&mut DirModel>, action: QString, value: QString) -> QString;
        #[qinvokable]
        fn cancel_device_share(self: Pin<&mut DirModel>);

        /// A place operation finished on its worker. `message` is "" on
        /// success, "ASK:<question>" when the user must decide, or a
        /// sentence saying what went wrong.
        #[qsignal]
        fn place_done(self: Pin<&mut DirModel>, op: QString, name: QString, message: QString);

        /// A worker finished file-clipboard admission for this exact model.
        #[qsignal]
        fn paste_ready(self: Pin<&mut DirModel>, message: QString, allow_image: bool);
        #[qinvokable]
        fn clipboard_busy(self: &DirModel) -> bool;
        #[qinvokable]
        fn cancel_clip_paste(self: Pin<&mut DirModel>);

        /// A removable device came, went, or had its media or mount change.
        /// The UI asks `devices_json` again.
        #[qsignal]
        fn devices_changed(self: Pin<&mut DirModel>);
        /// Start following device changes (udev's /dev/disk links). Called
        /// once by the window; the watch lives as long as the model.
        #[qinvokable]
        fn watch_devices(self: Pin<&mut DirModel>);

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
        /// Ctrl+R: reload and re-measure folder sizes.
        #[qinvokable]
        fn reload_all(self: Pin<&mut DirModel>);
        /// While true, changes made by other programs wait instead of moving
        /// rows. QML holds this while a rename prompt, menu or dialog refers
        /// to a row by its index; releasing it applies anything that waited.
        #[qinvokable]
        fn hold_live_updates(self: Pin<&mut DirModel>, hold: bool);
        /// The view shows rows `first .. first + count`. Details (size,
        /// date, mode) are read for those rows on a worker if they are not
        /// known yet: a listing reads only the first screenful up front.
        #[qinvokable]
        fn want_rows(self: Pin<&mut DirModel>, first: i32, count: i32);
        /// Whether this folder updates by itself (false for network mounts,
        /// where Ctrl+R is the only way to see changes made elsewhere).
        #[qinvokable]
        fn is_live(self: &DirModel) -> bool;
        /// Measure what a delete would remove (the targets, folders
        /// counted recursively) on a worker; the answer arrives in
        /// `delete_total` as a sentence, e.g. "2 folders, 1,204 files, 3.2 GB".
        #[qinvokable]
        fn request_delete_total(self: Pin<&mut DirModel>);
        /// Recent files from the desktop's list, as JSON (name, folder,
        /// path, when, exists). Local files only.
        #[qinvokable]
        fn recent_list(self: &DirModel) -> QString;
        /// Open a file by path with its default app, under the same rules
        /// as activating a row (executables never launch). "" or why not.
        #[qinvokable]
        fn open_path(self: &DirModel, path: &QString) -> QString;
        /// Go to a file's folder with the cursor on it.
        #[qinvokable]
        fn reveal(self: Pin<&mut DirModel>, path: &QString);
        /// The Trash: JSON list of name, original path and deletion date.
        #[qinvokable]
        fn trash_list(self: &DirModel) -> QString;
        /// Put a Trash item back where it came from. "" or why not.
        #[qinvokable]
        fn trash_restore(self: Pin<&mut DirModel>, name: &QString) -> QString;
        /// Columns view: fill `columns_side` for the cursor row, on a worker.
        #[qinvokable]
        fn request_columns(self: Pin<&mut DirModel>);
        /// Settings as JSON, plus whether Gust is the default file manager.
        #[qinvokable]
        fn settings_json(self: &DirModel) -> QString;
        /// Change one setting and save. Returns "" or why it failed.
        #[qinvokable]
        fn settings_set(self: &DirModel, key: &QString, value: &QString) -> QString;
        /// Make Gust the default file manager, or give it back, through the
        /// same tested path as the command line.
        #[qinvokable]
        fn set_default_file_manager(self: &DirModel, on: bool) -> QString;
        /// Chooser mode when another app's Open/Save dialog started this
        /// window: "", "open", "open-many", "open-folder" or "save".
        #[qinvokable]
        fn chooser_mode(self: &DirModel) -> QString;
        /// The dialog's title and, for Save, the suggested name.
        #[qinvokable]
        fn chooser_title(self: &DirModel) -> QString;
        #[qinvokable]
        fn chooser_name(self: &DirModel) -> QString;
        /// Hand the choice back and let the window close. Open modes send
        /// the selection (or the cursor row); Save sends folder + name.
        /// Returns "" or why it cannot be accepted.
        #[qinvokable]
        fn chooser_accept(self: &DirModel, save_name: &QString) -> QString;
        /// Make Super+Shift+F open Gust (Omarchy), or give it back.
        #[qinvokable]
        fn set_omarchy_keys(self: &DirModel, on: bool) -> QString;
        /// Answer other apps' Open/Save dialogs, or give them back.
        #[qinvokable]
        fn set_file_dialogs(self: &DirModel, on: bool) -> QString;
        /// Bookmarks shared with the desktop (GTK format), as a JSON list of
        /// label, path and exists. Only local folders; other entries in the
        /// file are kept but not listed.
        #[qinvokable]
        fn bookmarks_json(self: &DirModel) -> QString;
        /// Pin a folder. Returns "" or why it could not be saved.
        #[qinvokable]
        fn bookmark_add(self: &DirModel, path: &QString) -> QString;
        /// Unpin a folder. Returns "" or why it could not be saved.
        #[qinvokable]
        fn bookmark_remove(self: &DirModel, path: &QString) -> QString;
        /// Operations a crash interrupted, as JSON `[{id, text, dest}]`.
        /// Read once at start-up.
        #[qinvokable]
        fn interrupted_jobs(self: &DirModel) -> QString;
        /// The user has seen the interrupted-operations notice.
        #[qinvokable]
        fn acknowledge_interrupted(self: &DirModel);
        #[qinvokable]
        fn toggle_hidden(self: Pin<&mut DirModel>);
        /// QML reports whether the current view displays thumbnails.
        #[qinvokable]
        fn request_thumbnails(self: Pin<&mut DirModel>, enabled: bool);
        /// Record an EXPLICIT view choice for the current location.
        #[qinvokable]
        fn remember_view(self: Pin<&mut DirModel>, view: &QString);
        /// The view to use here: a remembered choice, else a suggestion,
        /// else an empty string meaning "caller decides".
        #[qinvokable]
        fn view_for_here(self: &DirModel) -> QString;
        #[qinvokable]
        fn set_sort(self: Pin<&mut DirModel>, field: &QString);
        /// Set the sort exactly (the toolbar's sort menu), rather than
        /// flipping direction when the field is already active.
        #[qinvokable]
        fn set_sort_to(self: Pin<&mut DirModel>, field: &QString, ascending: bool);
        /// The icon key and emblems of a row (QA probe; views use roles).
        #[qinvokable]
        fn entry_icon_key(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn entry_emblems(self: &DirModel, row: i32) -> QString;
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
        /// Install an allowlisted capability on a worker through polkit.
        /// No device is connected, trusted or browsed by installation.
        #[qinvokable]
        fn install_support(self: Pin<&mut DirModel>, key: QString) -> QString;

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
        /// Is there something Paste could use — Gust's own clipboard or files
        /// another application offered?
        #[qinvokable]
        fn can_paste(self: &DirModel) -> bool;
        /// Put the absolute path(s) of the selection — or of the folder when
        /// `row` is negative — on the desktop clipboard as text.
        #[qinvokable]
        fn copy_path(self: Pin<&mut DirModel>, row: i32) -> QString;
        /// Paste into the current folder. Starts a background job and returns
        /// "" on success, or a reason why it could not start.
        #[qinvokable]
        fn clip_paste(self: Pin<&mut DirModel>) -> QString;
        /// Paste into the folder at `row` ("Paste into folder").
        #[qinvokable]
        fn clip_paste_into(self: Pin<&mut DirModel>, row: i32) -> QString;
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
        /// Rebuild places_json on a worker.
        #[qinvokable]
        fn request_places(self: Pin<&mut DirModel>);
        /// Connect a Tailscale device as `user` over SSH and browse it. A
        /// host key not on record comes back as "ASKKEY:{json}" through
        /// place_done; calling again with `trust` records it and connects.
        #[qinvokable]
        fn tailnet_connect(
            self: Pin<&mut DirModel>,
            name: QString,
            target: QString,
            user: QString,
            trust: bool,
        ) -> QString;
        #[qinvokable]
        fn tailnet_disconnect(self: Pin<&mut DirModel>, name: QString) -> QString;
        /// Cloud remotes the INSTALLED rclone has configured, as JSON.
        /// Reading a config is not connecting: this opens no socket.
        #[qinvokable]
        fn cloud_suggestions(self: &DirModel) -> QString;
        /// Removable devices the kernel reports, as JSON. A local read:
        /// no daemon, no network, and Gust never auto-mounts (DESIGN §6.13).
        #[qinvokable]
        fn devices_json(self: &DirModel) -> QString;
        /// Read the device list again on a worker; devices_changed fires
        /// when it is ready. The UI thread never runs lsblk.
        #[qinvokable]
        fn request_devices(self: Pin<&mut DirModel>);
        /// Open a phone or camera (DESIGN §6.13.1): mount it through GVfs on
        /// a worker, answering its Trust/unlock question until the person has
        /// done it on the phone, then browse it. `apps` opens an iPhone's
        /// shared app files instead of its photos.
        #[qinvokable]
        fn phone_open(self: Pin<&mut DirModel>, id: QString, apps: bool) -> QString;
        /// Stop waiting for a phone (Cancel while it asks for Trust).
        #[qinvokable]
        fn phone_cancel(self: Pin<&mut DirModel>, id: QString);
        /// Eject a phone. Never forced; refused while a transfer runs.
        #[qinvokable]
        fn phone_eject(self: Pin<&mut DirModel>, id: QString) -> QString;
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
        /// Unlock an encrypted drive with its passphrase, then mount and
        /// browse it. The passphrase goes to udisks on a pipe: never argv,
        /// env, a file or a log, and it is not kept (DESIGN §6.13.2).
        #[qinvokable]
        fn device_unlock(self: Pin<&mut DirModel>, path: QString, passphrase: QString) -> QString;
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
        /// Adjacent previewable file in the visible order, without filesystem I/O.
        #[qinvokable]
        fn preview_neighbor(self: &DirModel, row: i32, delta: i32) -> i32;
        /// Hash a row's file on a worker; the answer lands in peek_hash.
        /// A second call (or hash_cancel) stops the first.
        #[qinvokable]
        fn hash_row(self: Pin<&mut DirModel>, row: i32);
        #[qinvokable]
        fn hash_cancel(self: Pin<&mut DirModel>);
        /// Make a link to the cursor row (or the one selected item) in this
        /// folder, named "name (link)". Ctrl+Z removes it if it is still
        /// that link. "" or why not.
        /// Gather folder-jump candidates (bookmarks, zoxide, recent) on a
        /// worker; jump_rev bumps when they are ready.
        #[qinvokable]
        fn jump_prepare(self: Pin<&mut DirModel>);
        /// The best folders for a query, as JSON [{path, shown, source}].
        /// Pure ranking over the gathered list: no filesystem access.
        #[qinvokable]
        fn jump_query(self: &DirModel, query: &QString, limit: i32) -> QString;
        /// Tab-complete a typed path on a worker; the answer lands in
        /// jump_completion as "<typed>\n<completed>".
        #[qinvokable]
        fn jump_complete(self: Pin<&mut DirModel>, typed: &QString);
        /// The user's custom actions that fit the current targets, as JSON
        /// {"actions":[{id,name}], "problems":[...]}. Reads the user's own
        /// config folder only.
        #[qinvokable]
        fn custom_actions_json(self: &DirModel) -> QString;
        /// Run a custom action on the targets. argv only, never a shell.
        /// "" or why not.
        #[qinvokable]
        fn run_custom_action(self: Pin<&mut DirModel>, id: &QString) -> QString;
        /// Can the row be run? "" when yes, otherwise why not.
        #[qinvokable]
        fn run_check(self: &DirModel, row: i32) -> QString;
        /// Run the row's executable in the default terminal. Only ever called
        /// after the user confirmed a dialog naming the file. "" or why not.
        #[qinvokable]
        fn run_row(self: Pin<&mut DirModel>, row: i32) -> QString;
        #[qinvokable]
        fn make_link(self: Pin<&mut DirModel>) -> QString;
        /// Change permissions of the targets ("644", "u+x", "go-w").
        /// Ctrl+Z puts every mode back where the file is unchanged since.
        #[qinvokable]
        fn set_mode(self: Pin<&mut DirModel>, spec: &QString) -> QString;
        /// The cursor row's rwx and octal, e.g. "rw-r--r--  644".
        #[qinvokable]
        fn mode_text(self: &DirModel) -> QString;
        /// Change owner and/or group of the targets ("bob", ":media",
        /// "bob:media"). Ctrl+Z puts every owner back where unchanged since.
        #[qinvokable]
        fn set_owner(self: Pin<&mut DirModel>, spec: &QString) -> QString;
        /// Colour tags on the selection (else the cursor row): "1"–"7"
        /// toggles that colour, "clear" removes every colour. One Ctrl+Z
        /// puts every file's tags back (DESIGN §6.25).
        #[qinvokable]
        fn set_tags(self: Pin<&mut DirModel>, op: &QString) -> QString;
        /// The colour keys on a row ("red,blue"), for qa and the views.
        #[qinvokable]
        fn entry_tags(self: &DirModel, row: i32) -> QString;
        /// The cursor row's owner, "alice : alice".
        #[qinvokable]
        fn owner_text(self: &DirModel) -> QString;
        /// The name of what an action will act on: the one selected item,
        /// or the cursor row when nothing is selected. Dialogs name this,
        /// never the cursor row behind a selection.
        #[qinvokable]
        fn target_name(self: &DirModel) -> QString;
        /// Paste an image from the clipboard as a new file here. Runs on a
        /// worker; the result arrives as place_done("paste-image", ...).
        #[qinvokable]
        fn paste_image(self: Pin<&mut DirModel>) -> QString;
        /// "Link here": a symbolic link to each dragged item, in the drop
        /// target. Ctrl+Z removes exactly those links.
        #[qinvokable]
        fn drop_link_on_row(self: Pin<&mut DirModel>, row: i32) -> QString;
        #[qinvokable]
        fn drop_link_preview(self: &DirModel, row: i32) -> QString;
        #[qinvokable]
        fn pause_job(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn resume_job(self: Pin<&mut DirModel>);
        /// Is this the administrator window?
        #[qinvokable]
        fn is_admin(self: &DirModel) -> bool;
        /// Open the cursor folder (or this one) in an administrator window,
        /// through pkexec. "" when the request was made, else why not.
        #[qinvokable]
        fn open_as_admin(self: &DirModel) -> QString;
        /// The tabs to reopen at start, as JSON, or "" when there are none
        /// (restoring is off, or Gust was asked for a folder).
        #[qinvokable]
        fn session_json(self: &DirModel) -> QString;
        /// Remember the open tabs: one "token view" line per tab.
        #[qinvokable]
        fn session_save(self: &DirModel, lines: &QString, active: i32);
        /// This folder as a location token (hex bytes, any name survives).
        #[qinvokable]
        fn location_token(self: &DirModel) -> QString;
        #[qinvokable]
        fn navigate_token(self: Pin<&mut DirModel>, token: &QString);
        /// Open a restored tab's folder as its starting point (no history).
        #[qinvokable]
        fn start_at_token(self: Pin<&mut DirModel>, token: &QString);
        /// A typed path as a location token (for tabs not yet opened).
        #[qinvokable]
        fn token_for(self: &DirModel, path: &QString) -> QString;
        /// Quick Look's visual: render a large preview for the row on a
        /// worker (sandboxed helpers only) and publish it in `peek_visual`
        /// as JSON — an image path, or an archive's entry list.
        #[qinvokable]
        fn request_peek_visual(self: Pin<&mut DirModel>, row: i32);
        /// Quick Look on a PDF: render page `page` (1-based) in the
        /// sandbox and publish it in `peek_visual` like the first page.
        #[qinvokable]
        fn request_peek_page(self: Pin<&mut DirModel>, row: i32, page: i32);
        /// Quick Look on audio: start playing the row (decoded in the
        /// sandbox, played through the desktop's PCM player).
        #[qinvokable]
        fn audio_play(self: Pin<&mut DirModel>, row: i32);
        /// Pause or resume; at the end, play again from the start.
        #[qinvokable]
        fn audio_toggle(self: Pin<&mut DirModel>);
        /// Jump to `secs` from the start.
        #[qinvokable]
        fn audio_seek(self: Pin<&mut DirModel>, secs: f64);
        /// Stop and forget the sound (Quick Look closed).
        #[qinvokable]
        fn audio_stop(self: Pin<&mut DirModel>);
        /// The player as JSON {state,pos,duration,title,artist,album,why},
        /// or "" when nothing is loaded. Quick Look polls it.
        #[qinvokable]
        fn audio_status(self: &DirModel) -> QString;
        #[qinvokable]
        fn drag_begin(self: Pin<&mut DirModel>, row: i32);
        /// How many items the in-flight drag carries (the drag preview).
        #[qinvokable]
        fn drag_count(self: &DirModel) -> i32;
        #[qinvokable]
        fn drop_preview(self: &DirModel, row: i32, as_move: bool) -> QString;
        /// A drag released by hand: `mode` 0 = the default for where it
        /// lands (Move on the same drive, Copy onto another), 1 = Copy
        /// (Ctrl held), 2 = Move (Shift held). DESIGN §6.1.
        #[qinvokable]
        fn drag_preview(self: &DirModel, row: i32, mode: i32) -> QString;
        #[qinvokable]
        fn drag_drop_on_row(self: Pin<&mut DirModel>, row: i32, mode: i32) -> QString;
        /// The same, onto an absolute folder outside this view: a sidebar
        /// place, a path-bar crumb, or a folder in the other pane.
        #[qinvokable]
        fn drag_preview_into(self: &DirModel, dest: &QString, mode: i32) -> QString;
        #[qinvokable]
        fn drag_drop_into(self: Pin<&mut DirModel>, dest: &QString, mode: i32) -> QString;
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
        /// What the in-flight drag carries, as text/uri-list, for handing
        /// to another application when the drag leaves the window.
        #[qinvokable]
        fn drag_uri_list(self: &DirModel) -> QString;
        /// Accept a drop of URIs from another application onto `row`.
        #[qinvokable]
        fn drop_uris(self: Pin<&mut DirModel>, row: i32, uris: &QString, as_move: bool) -> QString;
        /// Everything a drag hands to other applications, as JSON
        /// {uris, text, gnome}: built here so no name is decoded in JS.
        #[qinvokable]
        fn drag_mime_json(self: &DirModel) -> QString;
        /// A drag from another window or application entered this one:
        /// read the mount table once for planning (DESIGN §6.1).
        #[qinvokable]
        fn drop_enter(self: Pin<&mut DirModel>);
        /// What dropping `uris` onto `row` (or into the absolute folder
        /// `dest`, when not empty) would do. `gust` is true for a drag from
        /// another Gust window, which follows D60 (Move on one drive, Copy
        /// across); any other application's drag copies. `mode` 1 = Copy,
        /// 2 = Move, 0 = that default.
        #[qinvokable]
        fn uri_preview(
            self: &DirModel,
            row: i32,
            dest: &QString,
            uris: &QString,
            mode: i32,
            gust: bool,
        ) -> QString;
        /// Perform that drop through the transfer engine.
        #[qinvokable]
        fn uri_drop(
            self: Pin<&mut DirModel>,
            row: i32,
            dest: &QString,
            uris: &QString,
            mode: i32,
            gust: bool,
        ) -> QString;
        /// Forget what the last drag carried.
        #[qinvokable]
        fn drag_end(self: Pin<&mut DirModel>);
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
        /// Open this location in a NEW Gust window (a separate process, so
        /// Hyprland sees an independent client). Returns "" on success or
        /// the reason it was refused (DESIGN §6.20).
        #[qinvokable]
        fn detach_here(self: &DirModel, view: &QString) -> QString;
        /// After detach_here: "wait" while the new window is starting,
        /// "ready" once it is on screen (the tab may close now), or a
        /// reason it failed (the tab stays).
        #[qinvokable]
        fn poll_detach(self: &DirModel) -> QString;
        /// For a window started by a detach: the view it should show, and
        /// the ready signal back to the window it came from.
        #[qinvokable]
        fn handoff_view(self: &DirModel) -> QString;
        #[qinvokable]
        fn signal_ready(self: &DirModel);
        /// Open every selected file with its default handler. One launch
        /// when the handler takes a list, one per file when it does not
        /// (DESIGN §6.18). Returns "" when there is nothing multi-file to
        /// do, so the caller falls back to ordinary activation.
        #[qinvokable]
        fn activate_selection(self: Pin<&mut DirModel>) -> QString;
        /// Applications that can open the row, as JSON. Read from the
        /// desktop database, never from a localized command's output
        /// (DESIGN §6.17).
        #[qinvokable]
        fn open_with_list(self: &DirModel, row: i32) -> QString;
        /// Open the row with a specific desktop entry id.
        #[qinvokable]
        fn open_with_id(self: &DirModel, row: i32, id: &QString) -> QString;
        /// Preview a bulk rename over the selection, as JSON. Touches
        /// nothing: the user sees every old → new before anything happens
        /// (DESIGN §6.16).
        #[qinvokable]
        fn bulk_preview(self: &DirModel, row: i32, pattern: &QString) -> QString;
        /// Run a bulk rename the user has already seen. All or nothing.
        #[qinvokable]
        fn bulk_apply(self: Pin<&mut DirModel>, row: i32, pattern: &QString) -> QString;
        /// The rename card's starting text: the selected names, one per
        /// line, in screen order. "" when a name holds a line break (the
        /// card cannot show it as one line; F2 renames it alone).
        #[qinvokable]
        fn bulk_names(self: &DirModel, row: i32) -> QString;
        /// Check an edited list (one new name per line, same order) as
        /// JSON: ok, summary, changing, and each line's problem or null.
        /// Touches nothing.
        #[qinvokable]
        fn bulk_check(self: &DirModel, row: i32, names: &QString) -> QString;
        /// Rename each file to its line of `names`. All or nothing, one
        /// Ctrl+Z puts every name back.
        #[qinvokable]
        fn bulk_apply_names(self: Pin<&mut DirModel>, row: i32, names: &QString) -> QString;

        /// The clash the conflict dialog should show now, as JSON, or "".
        #[qinvokable]
        fn conflict_json(self: &DirModel) -> QString;
        /// Answer the current clash: "replace", "keep-both" or "skip", for
        /// this item or (`all`) every clash still unanswered. Returns
        /// "?conflict" while more questions remain, "" once the job has
        /// started, or a reason it could not.
        #[qinvokable]
        fn conflict_answer(self: Pin<&mut DirModel>, choice: &QString, all: bool) -> QString;
        /// Drop the held transfer. Nothing was written.
        #[qinvokable]
        fn conflict_cancel(self: Pin<&mut DirModel>);

        /// Poll the running job; refreshes the listing when it finishes.
        /// Returns a JSON snapshot, or "" when no job is running.
        #[qinvokable]
        fn poll_job(self: Pin<&mut DirModel>) -> QString;
        #[qinvokable]
        fn cancel_job(self: Pin<&mut DirModel>);
        #[qinvokable]
        fn job_running(self: &DirModel) -> bool;
        /// Cancel was asked for and the worker has not stopped yet. It may be
        /// waiting on a slow drive or a remote call that has not returned.
        #[qinvokable]
        fn job_stopping(self: &DirModel) -> bool;
        /// Compress or extract work this tab started that is still running.
        #[qinvokable]
        fn archive_running(self: &DirModel) -> bool;
        /// What this tab is still doing inside a connected place (`kind` is
        /// "ssh", "tailnet" or "cloud"): "transfer", "stopping", "archive",
        /// or "" for nothing. In memory only; never touches the place.
        #[qinvokable]
        fn work_on_place(self: &DirModel, kind: QString, name: QString) -> QString;

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
        /// Invert the selection over the rows shown (never what the filter
        /// hides).
        #[qinvokable]
        fn sel_invert(self: Pin<&mut DirModel>);
        /// Start a rubber band. `additive` (Ctrl held) keeps the current
        /// selection as the base; otherwise the band starts from nothing.
        #[qinvokable]
        fn band_begin(self: Pin<&mut DirModel>, additive: bool);
        /// The band covers rows r1..=r2 and columns c1..=c2 of a layout with
        /// `cols` items per row (1 for Rows). Index = row * cols + col.
        #[qinvokable]
        fn band_update(self: Pin<&mut DirModel>, r1: i32, r2: i32, c1: i32, c2: i32, cols: i32);
        /// What a key press means under a preset ("standard", "vim", "mac",
        /// "windows"): an action name, "tab-N", or "" when the window's own
        /// handler decides. One table drives this and the `?` key map.
        #[qinvokable]
        fn key_action(self: &DirModel, preset: &QString, key: i32, mods: i32) -> QString;
        /// The `?` key map for a preset, as JSON rows {group, keys, does}.
        #[qinvokable]
        fn keymap_json(self: &DirModel, preset: &QString) -> QString;
        /// Create an empty file here, never replacing one. "" or why not.
        #[qinvokable]
        fn new_file(self: Pin<&mut DirModel>, name: &QString) -> QString;
        /// The removable drive this folder is on, as its device path, or ""
        /// (Ctrl+E ejects it).
        #[qinvokable]
        fn device_here(self: &DirModel) -> QString;
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

        /// # Safety
        /// Inherited beginInsertRows; `end_insert_rows` must follow.
        #[inherit]
        #[cxx_name = "beginInsertRows"]
        unsafe fn begin_insert_rows(
            self: Pin<&mut DirModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        /// # Safety
        /// Inherited endInsertRows; only valid after `begin_insert_rows`.
        #[inherit]
        #[cxx_name = "endInsertRows"]
        unsafe fn end_insert_rows(self: Pin<&mut DirModel>);
        /// # Safety
        /// Inherited beginRemoveRows; `end_remove_rows` must follow.
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        unsafe fn begin_remove_rows(
            self: Pin<&mut DirModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        /// # Safety
        /// Inherited endRemoveRows; only valid after `begin_remove_rows`.
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        unsafe fn end_remove_rows(self: Pin<&mut DirModel>);

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
        /// Bumped whenever the token values change. QML bindings read it so
        /// they re-evaluate: `color()` is a call, which QML cannot track.
        #[qproperty(i32, revision)]
        /// Bumped when the file-type data (MIME database, special folders)
        /// has been (re)loaded, so bindings that ask for a look re-evaluate.
        #[qproperty(i32, icons_rev)]
        type ThemeTokens = super::ThemeTokensRust;

        /// Follow Omarchy theme switches from now on (live reload).
        #[qinvokable]
        fn watch_theme(self: Pin<&mut ThemeTokens>);

        #[qinvokable]
        fn color(self: &ThemeTokens, key: &QString) -> QString;
        #[qinvokable]
        fn metric(self: &ThemeTokens, key: &QString) -> f64;
        #[qinvokable]
        fn reload(self: Pin<&mut ThemeTokens>);
        #[qinvokable]
        fn load_theme_dir(self: Pin<&mut ThemeTokens>, dir: &QString, name: &QString);
        /// How an entry is drawn, for its `iconKey` role:
        /// `shape\tfamily\tlabel\tglyph` (DESIGN §6.21). A lookup in memory:
        /// the MIME database was read on a worker.
        #[qinvokable]
        fn icon_look(self: &ThemeTokens, key: &QString) -> QString;
    }

    // Lets a worker thread queue work back onto the GUI thread.
    impl cxx_qt::Threading for DirModel {}
    impl cxx_qt::Threading for ThemeTokens {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QString, QStringList, QVariant};
use gust_core::entry::{Entry, EntryKind, SortField};
use gust_core::lister::{
    fill_metadata, fill_metadata_for, first_window, list_names, stat_entry, CancelToken,
    FIRST_WINDOW,
};
use gust_core::location::LocationKey;
use gust_core::search::{filter_entries, type_ahead_index};
use gust_core::theme::Tokens;
use std::path::{Path, PathBuf};

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
const ROLE_TYPE_TEXT: i32 = 0x010B;
const ROLE_CLIP: i32 = 0x010C;
const ROLE_ICON_KEY: i32 = 0x010D;
const ROLE_EMBLEMS: i32 = 0x010E;
const ROLE_TAGS: i32 = 0x010F;

pub struct DirModelRust {
    empty_reason: QString,
    columns_side: QString,
    columns_gen: u64,
    peek_visual: QString,
    peek_gen: u64,
    /// The sound Quick Look is playing, if any. Dropping it stops it.
    peek_player: Option<gust_thumbs::audio::Player>,
    delete_total: QString,
    delete_gen: u64,
    location: QString,
    location_display: QString,
    home_path: QString,
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
    /// Every (from, to) of the last bulk rename, so one Ctrl+Z puts them
    /// all back — one user action, one undo entry (DESIGN §6.16).
    last_bulk_rename: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    /// The newest link Gust made, for Ctrl+Z.
    last_link: Option<gust_ops::attrs::LinkMade>,
    /// The newest permissions change, for Ctrl+Z.
    last_modes: Vec<gust_ops::attrs::ModeChanged>,
    /// Undo for links dropped, owners changed, images pasted.
    last_extra: UndoExtra,
    /// What Ctrl+C / Ctrl+X marked, as paths, so rows can show it.
    clip_set: std::collections::HashSet<PathBuf>,
    /// What Ctrl+C / Ctrl+X marked, waiting for a paste.
    clipboard: Option<(Vec<std::path::PathBuf>, ClipMode)>,
    clipboard_read: crate::clipboard::ReadGate,
    /// The job currently running, if any. Only one at a time in M2.
    job: Option<gust_ops::jobs::JobHandle>,
    /// Compress and extract work still running on its worker.
    archive_work: crate::place_work::ArchiveWork,
    /// A copy/move waiting for the user to answer its name clashes.
    pending: Option<PendingTransfer>,
    /// Speed of the running job, sampled on each poll.
    job_rate: gust_ops::rate::RateMeter,
    job_clock: Option<std::time::Instant>,
    /// Cancels the listing worker when the user navigates away.
    load_cancel: CancelToken,
    /// Live updates for the folder on screen. Replaced on every navigation,
    /// dropped (thread stopped) when the folder is left.
    watch: Option<gust_core::watch::DirWatch>,
    /// Watches on udev's /dev/disk folders, so a stick plugged in or
    /// pulled out shows up without a restart.
    device_watches: Vec<gust_core::watch::DirWatch>,
    /// Which folder `watch` is on, so a same-folder reload keeps it.
    watch_key: Option<PathBuf>,
    /// Increments per watch, so a late change from a replaced watch is
    /// ignored.
    watch_gen: u64,
    /// Rows are referred to by index while a prompt/menu is up.
    live_hold: bool,
    /// After a rename or new folder, the cursor goes to this name once it
    /// appears in the listing.
    focus_name: Option<std::ffi::OsString>,
    /// A change arrived during the hold; apply it when the hold ends.
    live_dirty: bool,
    /// A live relist is running; another change waits for it.
    live_busy: bool,
    /// name -> cached thumbnail path, for the rows currently listed.
    thumbs: std::collections::HashMap<std::ffi::OsString, String>,
    /// Cancels the thumbnail pass when the location changes.
    thumb_cancel: CancelToken,
    /// Repeated viewport reports must not restart an identical in-flight pass.
    thumb_work: crate::thumbnails::Work,
    /// name -> total size of that sub-folder, filled in the background.
    dir_sizes: std::collections::HashMap<std::ffi::OsString, gust_core::dirsize::Sized>,
    /// Cancels the folder-size pass when the location changes.
    size_cancel: CancelToken,
    /// What the in-flight drag carries, captured when it started.
    drag_payload: Vec<PathBuf>,
    /// The mount table as it was when the drag started: decides whether a
    /// plain drop moves (same drive) or copies (another one).
    drag_mounts: Vec<PathBuf>,
    /// True while a find is running or its results are shown.
    finding: bool,
    find_needle: QString,
    place_busy: bool,
    peek_hash: QString,
    /// What was selected when the current rubber band started.
    band_base: Vec<std::ffi::OsString>,
    jump_rev: i32,
    jump_completion: QString,
    places_json: QString,
    sharing_json: QString,
    sharing_busy: bool,
    sharing_checking: bool,
    sharing_work: SharingWork,
    /// Coalesce periodic and explicit refreshes; never publish overlapping snapshots.
    places_refreshing: bool,
    places_again: bool,
    jump_cands: Vec<gust_core::jump::Candidate>,
    /// Cancels a running hash (Quick Look closed, another file asked).
    hash_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Cancels a running find. Escape, a new find, or navigating away.
    find_cancel: CancelToken,
    /// The folder the user was browsing before the find. Results are a
    /// VIEW, not a navigation: leaving them returns here.
    find_origin: Option<LocationKey>,
    /// The row the cursor was on when the search started.
    find_return: Option<std::ffi::OsString>,
    /// True while a view that displays thumbnails (Grid) is active. Set by
    /// QML, which owns the view mode.
    thumbs_wanted: bool,
    /// Remembered explicit view choices, persisted between runs.
    views: gust_core::views::ViewMemory,
    /// Remembered per-folder sort choices, persisted between runs.
    sorts: gust_core::sorts::SortMemory,
    /// Selected entry NAMES (DESIGN.md §4.1) — never row indices.
    selection: gust_core::selection::Selection,
    /// The last completed copy/move, kept so Ctrl+Z can ask for its inverse.
    /// Cleared once undone, or superseded by the next job.
    last_job: Option<gust_ops::journal::Job>,
    /// A worker is reading details for rows that scrolled into view.
    meta_busy: bool,
    meta_rev: i32,
    /// The newest request that arrived while it was busy (first, count).
    meta_again: Option<(i32, i32)>,
    /// The rows the view last reported as on screen (first, count).
    view_rows: (i32, i32),
}

/// A copy or move held back until every name clash has an answer.
struct PendingTransfer {
    req: gust_ops::jobs::Request,
    clashes: Vec<gust_ops::jobs::Clash>,
    answers: std::collections::HashMap<PathBuf, gust_ops::journal::ConflictPolicy>,
    /// The paste came from a cut: the clipboard is consumed only when the
    /// job really starts, so a cancelled dialog leaves it pasteable.
    consumes_cut: bool,
}

#[derive(Default)]
struct SharingWork {
    cancel: CancelToken,
    prepared: Vec<PathBuf>,
    active_paths: Vec<PathBuf>,
}

impl Drop for SharingWork {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What a transfer funnel returns when it must ask first. QML opens the
/// conflict dialog on it; anything else non-empty is a refusal to show.
const NEEDS_ANSWERS: &str = "?conflict";

/// What a place operation's worker found.
enum PlaceOutcome {
    /// Browse this folder (unless the user moved on), then report success.
    Open {
        path: std::path::PathBuf,
        status: String,
    },
    /// Report success with a status line.
    Done(String),
    /// Browse this folder, and tell the user something they must not miss
    /// (files that were set aside to make the connection possible).
    OpenNotice {
        path: std::path::PathBuf,
        status: String,
        notice: String,
    },
    /// Report a failure the user must read.
    Failed(String),
    /// Continue on the GUI thread (e.g. leave a mount, then unmount it).
    Then(Box<dyn FnOnce(Pin<&mut qobject::DirModel>) + Send>),
}

/// A worker panic is an unknown outcome, not permission to retry its effects.
fn run_place_work<F>(work: F) -> std::thread::Result<PlaceOutcome>
where
    F: FnOnce() -> PlaceOutcome,
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
}

/// Is this the administrator window (root, started through pkexec)? Set
/// only by main.rs after it checked it is really running as root.
pub fn admin_session() -> bool {
    std::env::var_os("GUST_ADMIN").is_some_and(|v| v == "1")
}

/// What the administrator window refuses, in one sentence.
const ADMIN_REFUSAL: &str = "Administrator window: files are never opened or run here";

/// Undo for the actions added in v1 (each newest action owns Ctrl+Z).
#[derive(Debug, Clone, Default)]
pub enum UndoExtra {
    #[default]
    None,
    Links(Vec<gust_ops::attrs::LinkMade>),
    Owners(Vec<gust_ops::attrs::OwnerChanged>),
    Created(gust_ops::create::Created),
    Renamed(gust_ops::attrs::RenameMade),
    Tags(Vec<gust_ops::tags::TagChanged>),
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

    // The administrator window was handed its folder by its parent, which
    // already checked it (main.rs, prepare_admin_session).
    if let Some(d) = std::env::var_os("GUST_ADMIN_FOLDER") {
        if admin_session() {
            return PathBuf::from(d);
        }
    }

    // First non-flag argument wins. Flags are reserved for later options.
    let Some(path) = crate::cli::startup_argument(std::env::args_os().skip(1)) else {
        return home();
    };

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
        let mut model = Self::fresh(home.clone());
        // A window started by a tab detach picks up where the tab was:
        // history, selection, cursor, hidden files, view. Taken once (the
        // file is deleted), and only if it names this same folder.
        if let Some(h) = std::env::var_os("GUST_HANDOFF")
            .inspect(|_| std::env::remove_var("GUST_HANDOFF"))
            .and_then(|p| gust_core::handoff::Handoff::take(std::path::Path::new(&p)))
            .filter(|h| h.location == home)
        {
            model.back = h.back.iter().map(LocationKey::local).collect();
            model.show_hidden = h.show_hidden;
            if model.focus_name.is_none() {
                model.focus_name = h.cursor_name.clone();
            }
            model.selection.select_all_by_name(&h.selected);
            if let Ok(mut v) = HANDOFF_VIEW.lock() {
                *v = h.view.clone();
            }
        }
        model
    }
}

impl DirModelRust {
    fn fresh(home: PathBuf) -> Self {
        DirModelRust {
            location: QString::from(&home.to_string_lossy().to_string()),
            location_display: QString::from(&home.to_string_lossy().to_string()),
            home_path: QString::from(&std::env::var("HOME").unwrap_or_default()),
            count: 0,
            settled: false,
            show_hidden: false,
            status: QString::from("starting"),
            selection_text: QString::from(""),
            notice: QString::from(""),
            empty_reason: QString::from(""),
            columns_side: QString::from(""),
            columns_gen: 0,
            peek_visual: QString::from(""),
            peek_gen: 0,
            peek_player: None,
            delete_total: QString::from(""),
            delete_gen: 0,
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
            last_bulk_rename: Vec::new(),
            last_link: None,
            last_modes: Vec::new(),
            last_extra: UndoExtra::None,
            clip_set: Default::default(),
            clipboard: None,
            clipboard_read: Default::default(),
            pending: None,
            job_rate: gust_ops::rate::RateMeter::new(),
            job_clock: None,
            job: None,
            archive_work: Default::default(),
            load_cancel: CancelToken::new(),
            watch: None,
            device_watches: Vec::new(),
            watch_key: None,
            watch_gen: 0,
            live_hold: false,
            // "Show in folder" from another app: the file to put the cursor
            // on once the first listing is in. Read once; never inherited by
            // windows this one opens.
            focus_name: std::env::var_os("GUST_SELECT")
                .filter(|v| !v.is_empty())
                .inspect(|_| std::env::remove_var("GUST_SELECT")),
            live_dirty: false,
            live_busy: false,
            thumbs: std::collections::HashMap::new(),
            thumb_cancel: CancelToken::new(),
            thumb_work: crate::thumbnails::Work::default(),
            dir_sizes: std::collections::HashMap::new(),
            size_cancel: CancelToken::new(),
            drag_payload: Vec::new(),
            drag_mounts: Vec::new(),
            finding: false,
            find_needle: QString::from(""),
            place_busy: false,
            peek_hash: QString::default(),
            band_base: Vec::new(),
            jump_rev: 0,
            jump_completion: QString::default(),
            places_json: QString::from("{}"),
            sharing_json: QString::from("{}"),
            sharing_busy: false,
            sharing_checking: false,
            sharing_work: SharingWork::default(),
            places_refreshing: false,
            places_again: false,
            jump_cands: Vec::new(),
            hash_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            find_cancel: CancelToken::new(),
            find_origin: None,
            find_return: None,
            thumbs_wanted: false,
            views: gust_core::views::ViewMemory::default_path()
                .map(|p| gust_core::views::ViewMemory::load(&p))
                .unwrap_or_default(),
            sorts: gust_core::sorts::SortMemory::default_path()
                .map(|p| gust_core::sorts::SortMemory::load(&p))
                .unwrap_or_default(),
            selection: gust_core::selection::Selection::new(),
            last_job: None,
            meta_busy: false,
            meta_rev: 0,
            meta_again: None,
            view_rows: (0, 0),
        }
    }

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
        self.thumb_cancel.cancel();
        self.thumb_work.invalidate();
        self.visible = visible_indices(&self.all, &self.filter.to_string(), self.show_hidden);
    }

    fn entry_at(&self, row: i32) -> Option<&Entry> {
        let idx = *self.visible.get(usize::try_from(row).ok()?)?;
        self.all.get(idx)
    }

    /// What an entry looks like, as a key `icon_look` resolves.
    fn icon_key_of(&self, e: &Entry) -> String {
        let full = self
            .key
            .as_path()
            .map(|d| d.join(&e.name))
            .unwrap_or_default();
        gust_core::icons::key_for(&e.name, e.kind, e.meta.as_ref(), &full)
    }

    /// Local filesystem path of a visible row, if it has one.
    fn path_of_row(&self, row: i32) -> Option<std::path::PathBuf> {
        let name = self.entry_at(row).map(|e| e.name.clone())?;
        self.key.join(&name).as_path().map(|p| p.to_path_buf())
    }
}

/// Names for mounted places Gust knows better than their mount path does:
/// an opened phone is "Alex's iPhone", not `afc:host=<udid>`.
fn place_labels() -> &'static std::sync::Mutex<Vec<(std::path::PathBuf, String)>> {
    static S: std::sync::OnceLock<std::sync::Mutex<Vec<(std::path::PathBuf, String)>>> =
        std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// The place a path is in and its name: a phone Gust opened first, then the
/// general rule (Gust's own mounts, GVfs shares, drives).
fn place_of(path: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
    let named = place_labels().lock().ok().and_then(|g| {
        g.iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.as_os_str().len())
            .cloned()
    });
    named.or_else(|| gust_core::location::place_root(path))
}

/// Where each plugged-in phone stands, by its id (DESIGN §6.13.1).
fn phone_states(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, gust_remote::phones::PhoneState>> {
    static S: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, gust_remote::phones::PhoneState>>,
    > = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Phones the user stopped waiting for.
fn phone_cancels() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// The device list, shared by every tab's model (one machine, one list).
/// Overwrite a secret's bytes before they are freed. Volatile writes, so
/// the compiler cannot decide the stores are dead and drop them.
fn wipe(bytes: &mut Vec<u8>) {
    for b in bytes.iter_mut() {
        // SAFETY: `b` is a valid, aligned &mut u8 for the whole loop body.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    bytes.clear();
}

fn devices_cache() -> &'static std::sync::Mutex<String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new("[]".to_string()))
}

/// "1 item", "12 items".
fn count_items<N: std::fmt::Display + PartialEq + From<u8>>(n: N) -> String {
    if n == N::from(1) {
        "1 item".to_string()
    } else {
        format!("{n} items")
    }
}

/// Label for a clipboard or command target: one name, or a count.
fn clip_label(paths: &[std::path::PathBuf]) -> String {
    match paths.len() {
        0 => "nothing".to_string(),
        1 => display_file_name(&paths[0]),
        n => count_items(n),
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
    use std::cell::Cell;
    thread_local! {
        // (utc offset, when it was read). data() runs per painted row, so the
        // zone is not re-read every call — once a minute follows DST.
        static OFFSET: Cell<(i64, i64)> = const { Cell::new((0, i64::MIN)) };
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let offset = OFFSET.with(|c| {
        let (off, at) = c.get();
        if now.saturating_sub(at) < 60 {
            off
        } else {
            let off = gust_core::properties::local_offset_seconds();
            c.set((off, now));
            off
        }
    });
    gust_core::properties::relative_time(mtime, now, offset)
}

impl qobject::DirModel {
    /// The Qt thread handle, or None before the object is fully constructed.
    fn qt_thread_handle(&self) -> Option<cxx_qt::CxxQtThread<qobject::DirModel>> {
        Some(<qobject::DirModel as cxx_qt::Threading>::qt_thread(self))
    }

    /// Run a place operation (mount, connect, eject, disconnect) on a worker.
    ///
    /// These call sshfs, gio, udisksctl, rclone and fusermount3, each of
    /// which can take many seconds against a slow or dead server. Blocking
    /// the GUI thread for that long froze the whole window (the m41 audit);
    /// here the call returns at once and `place_done` reports the outcome.
    /// A result that navigates is dropped if the user has already gone
    /// somewhere else in the meantime.
    fn place_op<F>(mut self: Pin<&mut Self>, op: &'static str, name: String, busy: String, work: F)
    where
        F: FnOnce() -> PlaceOutcome + Send + 'static,
    {
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        if self.as_ref().place_busy {
            self.as_mut().place_done(
                QString::from(op),
                QString::from(&name),
                QString::from("Another connection is still in progress."),
            );
            return;
        }
        self.as_mut().set_place_busy(true);
        self.as_mut().set_status(QString::from(&busy));
        let gen = self.as_ref().generation;
        let spawned = std::thread::Builder::new()
            .name(format!("gust-{op}"))
            .spawn(move || {
                let outcome = match run_place_work(work) {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let message = "The connection worker stopped unexpectedly. Its outcome is unknown. Choose Check again before retrying.".to_string();
                        if op == "phone-open" {
                            Self::set_phone_state(
                                &name,
                                gust_remote::phones::PhoneState::Failed(message.clone()),
                            );
                        }
                        PlaceOutcome::Failed(message)
                    }
                };
                let _ = thread.queue(move |mut model| {
                    model.as_mut().set_place_busy(false);
                    let (opq, nameq) = (QString::from(op), QString::from(&name));
                    match outcome {
                        PlaceOutcome::Open { path, status } => {
                            if model.as_ref().generation == gen {
                                model
                                    .as_mut()
                                    .navigate(&QString::from(path.to_string_lossy().as_ref()));
                            }
                            if !status.is_empty() {
                                model.as_mut().set_status(QString::from(&status));
                            }
                            model.as_mut().place_done(opq, nameq, QString::default());
                        }
                        PlaceOutcome::Done(status) => {
                            model.as_mut().set_status(QString::from(&status));
                            model.as_mut().place_done(opq, nameq, QString::default());
                        }
                        PlaceOutcome::OpenNotice {
                            path,
                            status,
                            notice,
                        } => {
                            if model.as_ref().generation == gen {
                                model
                                    .as_mut()
                                    .navigate(&QString::from(path.to_string_lossy().as_ref()));
                            }
                            model.as_mut().set_status(QString::from(&status));
                            model
                                .as_mut()
                                .place_done(opq, nameq.clone(), QString::default());
                            model.as_mut().place_done(
                                QString::from("recovered"),
                                nameq,
                                QString::from(&notice),
                            );
                        }
                        PlaceOutcome::Failed(msg) => {
                            model.as_mut().set_status(QString::from(""));
                            model.as_mut().place_done(opq, nameq, QString::from(&msg));
                        }
                        PlaceOutcome::Then(next) => next(model),
                    }
                });
            });
        if spawned.is_err() {
            self.as_mut().set_place_busy(false);
            self.as_mut().place_done(
                QString::from(op),
                QString::from(""),
                QString::from("Could not start a worker thread."),
            );
        }
    }

    /// Leave `mount` for the home folder if this pane is inside it, so an
    /// unmount never pulls a filesystem out from under the open listing.
    fn leave_if_inside(mut self: Pin<&mut Self>, mount: &std::path::Path) {
        let here = std::path::PathBuf::from(self.as_ref().location.to_string());
        if here.starts_with(mount) {
            if let Some(home) = std::env::var_os("HOME") {
                let q = QString::from(std::path::PathBuf::from(home).to_string_lossy().as_ref());
                self.as_mut().navigate(&q);
            }
        }
    }

    /// Start loading the current location on a WORKER thread.
    ///
    /// The UI thread never touches the filesystem (DESIGN.md §9). Names
    /// arrive first so the window paints immediately, then metadata fills
    /// the columns. Every result is tagged with a generation, so a late
    /// answer for a directory the user already left is dropped.
    fn load_current(self: Pin<&mut Self>) {
        self.load_current_cached(true);
    }

    fn load_current_cached(mut self: Pin<&mut Self>, reuse: bool) {
        let key = self.as_ref().key.clone();
        let gen = self.as_ref().generation + 1;
        // Per-folder sort: this folder's remembered choice, or by name.
        // set_sort records every choice before it reloads, so for the
        // current folder this always agrees with what the user picked.
        {
            let spec = gust_core::sorts::spec_for(&self.as_ref().sorts, &key);
            let f = gust_core::sorts::sort_text(spec.field, true);
            let f = f.trim_end_matches("-asc").to_string();
            if self.as_ref().sort_field.to_string() != f {
                self.as_mut().set_sort_field(QString::from(&f));
            }
            if self.as_ref().sort_ascending != spec.ascending {
                self.as_mut().set_sort_ascending(spec.ascending);
            }
        }
        let sort = self.as_ref().sort_spec();
        self.as_mut().rust_mut().generation = gen;

        // Cancel whatever the previous navigation was still doing.
        self.as_mut().rust_mut().load_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().load_cancel = cancel.clone();
        self.as_mut().rust_mut().thumb_cancel.cancel();
        self.as_mut().rust_mut().thumb_work.invalidate();
        self.as_mut().rust_mut().thumbs.clear();

        // Leaving the watched folder ends its watch now, not when the next
        // listing settles: its changes no longer describe what is on screen.
        if self.as_ref().watch_key.as_deref() != key.as_path() {
            self.as_mut().stop_watch();
            // Sizes belong to the folder being left, whichever way it was
            // left (back, forward, up, a path typed in).
            self.as_mut().rust_mut().size_cancel.cancel();
            self.as_mut().rust_mut().dir_sizes.clear();
        }

        // What the worker needs to know which rows land on screen first.
        let filter_now = self.as_ref().filter.to_string();
        let hidden_now = self.as_ref().show_hidden;
        let focus_now = self.as_ref().focus_name.clone();
        let grid_now = self.as_ref().thumbs_wanted
            || self
                .as_ref()
                .views
                .get(&key)
                .is_some_and(|view| view.as_str() == "grid");

        let display = key.display_path();
        self.as_mut().set_location(QString::from(&display));
        // Inside a mounted place, name it as the user knows it ("nas/Shared"),
        // not by Gust's mount path under /run (title bar, tab label).
        let shown = key
            .as_path()
            .and_then(|p| {
                let (root, label) = place_of(p)?;
                let rest = p.strip_prefix(&root).ok()?;
                Some(if rest.as_os_str().is_empty() {
                    label
                } else {
                    format!(
                        "{label}/{}",
                        gust_core::location::escape_for_display(rest.as_os_str())
                    )
                })
            })
            .unwrap_or_else(|| display.clone());
        self.as_mut().set_location_display(QString::from(&shown));
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
                let mut snap = crate::directory_cache::list(&key, gen, &cancel, reuse);
                if cancel.is_cancelled() {
                    return;
                }
                if snap.sort != sort {
                    snap.sort = sort;
                    snap.sort_entries();
                }

                if snap.sort_needs_metadata() {
                    // Size or date order needs every entry's details. Show
                    // names at once, then the settled order.
                    let names = snap.clone();
                    let _ = thread.queue(move |model| {
                        Self::apply_snapshot(model, names, false);
                    });
                    fill_metadata(&mut snap, &cancel);
                    if cancel.is_cancelled() {
                        return;
                    }
                    let hits =
                        Self::initial_thumbnails(&snap, grid_now, &filter_now, hidden_now, &cancel);
                    let _ = thread.queue(move |mut model| {
                        if model.as_ref().generation != gen {
                            return;
                        }
                        model.as_mut().rust_mut().thumbs.extend(hits);
                        Self::apply_snapshot(model, snap, true);
                    });
                    return;
                }

                // Name or kind order is final on names alone. Read details
                // only for the first screenful (and a row the cursor must
                // land on); rows further down are read when they scroll into
                // view (`want_rows`). A 100k folder then settles on 256
                // stats instead of 100 000.
                let slow = key.as_path().is_some_and(gust_core::stall::needs_probe);
                if slow {
                    // One stat per row is a network round trip: names first.
                    let names = snap.clone();
                    let _ = thread.queue(move |model| {
                        Self::apply_snapshot(model, names, false);
                    });
                }
                let shown = visible_indices(&snap.entries, &filter_now, hidden_now);
                let focus_row = focus_now
                    .as_ref()
                    .and_then(|f| shown.iter().position(|&i| snap.entries[i].name == *f));
                let window = first_window(&shown, focus_row, FIRST_WINDOW);
                if !fill_metadata_for(&mut snap, &window, &cancel) {
                    return;
                }
                let hits =
                    Self::initial_thumbnails(&snap, grid_now, &filter_now, hidden_now, &cancel);
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().generation != gen {
                        return;
                    }
                    model.as_mut().rust_mut().thumbs.extend(hits);
                    Self::apply_snapshot(model, snap, true);
                });
            })
            .ok();
    }

    /// Cache I/O happens before queuing the model reset, on its listing worker.
    fn initial_thumbnails(
        snap: &gust_core::DirectorySnapshot,
        wanted: bool,
        filter: &str,
        hidden: bool,
        cancel: &CancelToken,
    ) -> Vec<(std::ffi::OsString, String)> {
        if !wanted || admin_session() {
            return Vec::new();
        }
        let Some(dir) = snap.key.as_path() else {
            return Vec::new();
        };
        let shown = visible_indices(&snap.entries, filter, hidden);
        let window = first_window(&shown, None, FIRST_WINDOW);
        let names = window
            .into_iter()
            .filter_map(|i| snap.entries.get(i))
            .filter(|e| crate::thumbnails::mime(e).is_some())
            .map(|e| e.name.clone());
        crate::thumbnails::cached_rows(dir, names, cancel)
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
        // The administrator window never feeds file contents to helpers.
        if admin_session() {
            return;
        }
        if !self.as_ref().thumbs_wanted || !self.as_ref().settled {
            return;
        }
        let key = self.as_ref().key.clone();
        let gen = self.as_ref().generation;
        let Some(dir) = key.as_path().map(|p| p.to_path_buf()) else {
            return;
        };

        let (first, count) = self.as_ref().view_rows;
        let window = gust_core::dirsize::rows_in_view(first, count, self.as_ref().visible.len());
        let pass = (gen, window.start, window.end);
        match self.as_mut().rust_mut().thumb_work.request(pass) {
            crate::thumbnails::Admission::Unchanged => return,
            crate::thumbnails::Admission::Pending => {
                self.as_mut().rust_mut().thumb_cancel.cancel();
                return;
            }
            crate::thumbnails::Admission::Start => {}
        }

        // Only the viewport and its prefetch margin, with on-screen rows
        // first. A late photo must not wait for the first 400 files.
        let want: Vec<(std::ffi::OsString, &'static str)> = {
            let r = self.as_ref();
            let first = (first.max(0) as usize).clamp(window.start, window.end);
            r.visible[first..window.end]
                .iter()
                .chain(r.visible[window.start..first].iter())
                .filter_map(|i| r.all.get(*i))
                .filter_map(|e| crate::thumbnails::mime(e).map(|m| (e.name.clone(), m)))
                .collect()
        };
        self.as_mut().rust_mut().thumb_cancel.cancel();
        if want.is_empty() {
            self.as_mut().rust_mut().thumb_work.finish();
            return;
        }

        let cancel = CancelToken::new();
        self.as_mut().rust_mut().thumb_cancel = cancel.clone();

        let Some(thread) = self.as_ref().qt_thread_handle() else {
            self.as_mut().rust_mut().thumb_work.finish();
            self.as_mut().rust_mut().thumb_work.invalidate();
            return;
        };

        let spawned = std::thread::Builder::new()
            .name("gust-thumbs".into())
            .spawn(move || {
                if !gust_thumbs::sandbox::sandbox_available() {
                    let _ = thread.queue(move |mut model| {
                        if model.as_ref().generation == gen {
                            model.as_mut().set_notice(QString::from(
                                "Previews need bubblewrap — install bwrap to enable them",
                            ));
                        }
                    });
                }
                crate::thumbnails::run(&dir, want, &cancel, |batch| {
                    let publish_cancel = cancel.clone();
                    let _ = thread.queue(move |model| {
                        if !publish_cancel.is_cancelled() {
                            Self::publish_thumbnails(model, gen, batch);
                        }
                    });
                });
                let _ = thread.queue(move |mut model| {
                    if model.as_mut().rust_mut().thumb_work.finish() {
                        model.as_mut().start_thumbnails();
                    }
                });
            });
        if spawned.is_err() {
            self.as_mut().rust_mut().thumb_work.finish();
            self.as_mut().rust_mut().thumb_work.invalidate();
        }
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
            let mp = gust_remote::mount::mount_point_for(&h.alias);
            let mounted = mp
                .as_ref()
                .map(|p| gust_remote::mount::is_mounted(p))
                .unwrap_or(false);
            // Mounted is not the same as working: a server that went away
            // leaves the mount in the table, answering every read with an
            // I/O error. Only a mounted place is asked, by a killable child
            // with a short limit, so this never hangs the window.
            let lost = mounted
                && mp.as_ref().is_some_and(|p| {
                    !gust_core::stall::probe(p, std::time::Duration::from_millis(500))
                        .is_responsive()
                });
            // Escaped: an alias is arbitrary text from a user's config.
            out.push_str(&format!(
                r#"{{"alias":{},"host":{},"proxy":{},"connected":{},"lost":{}}}"#,
                Self::json_str(&h.alias),
                Self::json_str(h.hostname.as_deref().unwrap_or(&h.alias)),
                h.has_proxy,
                mounted,
                lost
            ));
        }
        out.push(']');
        QString::from(out)
    }

    // --- cloud remotes (DESIGN §6.8) -------------------------------------

    /// The outcome of a connection that worked, naming any files that had
    /// to be set aside from the mount point first.
    fn opened(
        path: std::path::PathBuf,
        status: String,
        place: &str,
        recovered: Option<std::path::PathBuf>,
    ) -> PlaceOutcome {
        match recovered {
            None => PlaceOutcome::Open { path, status },
            Some(dir) => PlaceOutcome::OpenNotice {
                path,
                status,
                notice: format!(
                    "Files were left in {place}'s folder from an earlier connection. \
                     Nothing was deleted: they are in {}",
                    dir.display()
                ),
            },
        }
    }

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
        // A Unix socket in the user's runtime dir: only this user can open
        // it, so the RC needs no password and no port is exposed to other
        // users or to web pages that can reach loopback.
        match Self::cloud_rc_socket(name) {
            Some(p) => gust_remote::cloud::rc_bind_arg(&p),
            None => String::from("unix:///nonexistent"),
        }
    }

    fn cloud_rc_socket(name: &str) -> Option<std::path::PathBuf> {
        let mp = Self::cloud_mount_point(name)?;
        let leaf = mp.file_name()?.to_string_lossy().into_owned();
        let runtime = mp.parent()?.parent()?.parent()?.to_path_buf();
        Some(gust_remote::cloud::rc_socket_path(&runtime, &leaf))
    }

    /// Per-object upload state for every cloud mount in THIS process.
    ///
    /// Process-wide, not per-model: one mount is shared by every tab and
    /// pane, so a file written in one tab must still block a disconnect
    /// triggered from another. rclone's VFS queue describes rclone's own
    /// work; this records what Gust promised the user about each object, and
    /// only a confirmed revision clears it (DESIGN §6.8, remote rule 1).
    fn cloud_tracker() -> &'static std::sync::Mutex<gust_remote::upload::UploadTracker> {
        use std::sync::{Mutex, OnceLock};
        static T: OnceLock<Mutex<gust_remote::upload::UploadTracker>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(gust_remote::upload::UploadTracker::new()))
    }

    /// Record every file a finished job wrote INSIDE a cloud mount as dirty.
    ///
    /// Landing in the VFS cache is not an upload. Until the backend confirms
    /// the revision these files are work that exists only on this computer,
    /// and a disconnect that discards them is data loss.
    fn track_cloud_writes(record: &gust_ops::journal::Job) {
        let mounts: Vec<std::path::PathBuf> = match gust_remote::cloud::list_remotes(None) {
            Ok(rs) => rs
                .into_iter()
                .filter_map(|r| Self::cloud_mount_point(&r.name))
                .collect(),
            Err(_) => return,
        };
        if mounts.is_empty() {
            return;
        }
        let Ok(mut t) = Self::cloud_tracker().lock() else {
            return;
        };
        for e in &record.entries {
            if !e.done {
                continue;
            }
            let Some(dest) = &e.destination else { continue };
            if mounts.iter().any(|mp| dest.starts_with(mp)) {
                t.set(dest, gust_remote::upload::UploadState::Cached, None);
            }
        }
    }

    // --- detaching a tab (DESIGN §6.20) ----------------------------------

    /// The durable journal every real operation records into.
    ///
    /// Without this the record is an in-memory `Arc<Mutex<Job>>` that dies
    /// with the process, so a crash mid-copy leaves staging debris that can
    /// never be attributed to an operation and nothing for a restart to
    /// recover (DESIGN.md §5, §7). Tests pass `None` to stay off disk.
    fn production_journal() -> Option<gust_ops::journal::Journal> {
        // A QA run points HOME/XDG_STATE_HOME at a fixture, so this follows
        // the same isolation as every other piece of state.
        Some(gust_ops::journal::Journal::new(
            gust_ops::journal::Journal::default_path(),
        ))
    }

    /// Operations a crash interrupted, as JSON for the start-up notice.
    fn interrupted_jobs(&self) -> QString {
        let Some(j) = Self::production_journal() else {
            return QString::from("[]");
        };
        let items: Vec<String> = j
            .interrupted()
            .into_iter()
            .map(|i| {
                let dest = i
                    .dest_dir
                    .as_ref()
                    .filter(|d| d.is_dir())
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!(
                    "{{\"id\":{},\"text\":{},\"dest\":{}}}",
                    Self::json_str(&i.id),
                    Self::json_str(&i.sentence()),
                    Self::json_str(&dest)
                )
            })
            .collect();
        QString::from(&format!("[{}]", items.join(",")))
    }

    fn request_peek_visual(self: Pin<&mut Self>, row: i32) {
        self.peek_visual_for(row, 1);
    }

    fn request_peek_page(self: Pin<&mut Self>, row: i32, page: i32) {
        self.peek_visual_for(row, u32::try_from(page).unwrap_or(1).max(1));
    }

    /// Build Quick Look's visual for a row on a worker (DESIGN §6.9.1):
    /// an image or PDF page rendered in the sandbox, an archive listing,
    /// rendered Markdown, or `{"audio":true}` for something to play.
    fn peek_visual_for(mut self: Pin<&mut Self>, row: i32, page: u32) {
        if admin_session() {
            return;
        }
        let gen = self.as_ref().peek_gen.wrapping_add(1);
        self.as_mut().rust_mut().peek_gen = gen;
        // A page turn keeps the old page up until the new one is ready.
        if page == 1 {
            self.as_mut().set_peek_visual(QString::default());
        }
        let Some(path) = self.as_ref().path_of_row(row) else {
            return;
        };
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
        std::thread::Builder::new()
            .name("gust-peek".into())
            .spawn(move || {
                // Nothing is fetched from a place that can stall.
                if gust_core::stall::needs_probe(&path) {
                    return;
                }
                let Some(json) = Self::visual_json(&path, &name, page) else {
                    return;
                };
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().peek_gen == gen {
                        model.as_mut().set_peek_visual(QString::from(&json));
                    }
                });
            })
            .ok();
    }

    /// The visual for one file, as JSON. Runs on a worker.
    fn visual_json(path: &std::path::Path, name: &std::ffi::OsStr, page: u32) -> Option<String> {
        use gust_core::mime::{category_for_name, Category};
        let lower = name.to_string_lossy().to_lowercase();
        let cat = category_for_name(name);
        if cat == Category::Pdf {
            let pages = gust_thumbs::render::pdf_pages(path).unwrap_or(1);
            let page = page.min(pages);
            return Some(match gust_thumbs::render::pdf_page(path, page, 1024) {
                Ok(img) => format!(
                    "{{\"image\":{},\"page\":{page},\"pages\":{pages}}}",
                    Self::json_str(&img.to_string_lossy())
                ),
                Err(e) => format!("{{\"why\":{}}}", Self::json_str(&e.to_string())),
            });
        }
        if cat == Category::Audio {
            return Some(match gust_thumbs::audio::available() {
                Ok(()) => "{\"audio\":true}".into(),
                Err(why) => format!("{{\"audio\":false,\"why\":{}}}", Self::json_str(why)),
            });
        }
        if lower.ends_with(".md") || lower.ends_with(".markdown") {
            use std::io::Read;
            let f = std::fs::File::open(path).ok()?;
            let mut buf = Vec::new();
            f.take(256 * 1024).read_to_end(&mut buf).ok()?;
            if buf.contains(&0) {
                return None; // not text after all: the plain peek says so
            }
            let (html, truncated) = gust_core::markdown::to_styled(&String::from_utf8_lossy(&buf));
            return Some(format!(
                "{{\"markdown\":{},\"truncated\":{}}}",
                Self::json_str(&html),
                truncated || buf.len() >= 256 * 1024
            ));
        }
        if let Some(mime) = gust_core::mime::thumbnail_mime(name) {
            let video = mime.starts_with("video/");
            return Some(
                match gust_thumbs::render::thumbnail(
                    path,
                    mime,
                    // Big enough for the panel on a 1440p screen; small
                    // images keep their own size (never enlarged).
                    gust_thumbs::cache::ThumbSize::XxLarge,
                ) {
                    Ok(img) => format!(
                        "{{\"image\":{},\"video\":{video}}}",
                        Self::json_str(&img.to_string_lossy())
                    ),
                    Err(e) => format!(
                        "{{\"why\":{},\"video\":{video}}}",
                        Self::json_str(&e.to_string())
                    ),
                },
            );
        }
        if gust_ops::archive::io::looks_like_archive(&name.to_string_lossy()) {
            return Some(match gust_ops::archive::io::list(path) {
                Ok(entries) => {
                    let total = entries.len();
                    let shown: Vec<String> = entries
                        .iter()
                        .take(200)
                        .map(|e| Self::json_str(&gust_core::peek::sanitise_line(e.name.as_bytes())))
                        .collect();
                    format!("{{\"archive\":[{}],\"total\":{}}}", shown.join(","), total)
                }
                Err(e) => format!("{{\"why\":{}}}", Self::json_str(&e.message())),
            });
        }
        None
    }

    fn audio_play(mut self: Pin<&mut Self>, row: i32) {
        self.as_mut().rust_mut().peek_player = None;
        if admin_session() {
            return;
        }
        let Some(path) = self.as_ref().path_of_row(row) else {
            return;
        };
        if gust_core::stall::needs_probe(&path) {
            return;
        }
        self.as_mut().rust_mut().peek_player = Some(gust_thumbs::audio::Player::start(&path));
    }

    fn audio_toggle(self: Pin<&mut Self>) {
        if let Some(p) = self.as_ref().peek_player.as_ref() {
            p.toggle();
        }
    }

    fn audio_seek(self: Pin<&mut Self>, secs: f64) {
        if let Some(p) = self.as_ref().peek_player.as_ref() {
            if secs.is_finite() {
                p.seek(secs);
            }
        }
    }

    fn audio_stop(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().peek_player = None;
    }

    fn audio_status(&self) -> QString {
        let Some(p) = self.peek_player.as_ref() else {
            return QString::default();
        };
        let st = p.status();
        QString::from(&format!(
            r#"{{"state":"{}","pos":{:.2},"duration":{},"title":{},"artist":{},"album":{},"why":{}}}"#,
            st.state.as_str(),
            st.pos,
            st.duration
                .map(|d| format!("{d:.2}"))
                .unwrap_or_else(|| "null".into()),
            Self::json_str(&st.title),
            Self::json_str(&st.artist),
            Self::json_str(&st.album),
            Self::json_str(&st.why),
        ))
    }

    fn request_delete_total(mut self: Pin<&mut Self>) {
        let gen = self.as_ref().delete_gen.wrapping_add(1);
        self.as_mut().rust_mut().delete_gen = gen;
        self.as_mut().set_delete_total(QString::default());
        let targets = self.as_ref().target_paths();
        if targets.is_empty() || targets.iter().any(|p| gust_core::stall::needs_probe(p)) {
            // Never walk a network place just to phrase a dialog.
            return;
        }
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-delete-total".into())
            .spawn(move || {
                let cancel = CancelToken::new();
                let (mut folders, mut files, mut bytes, mut complete) = (0u64, 0u64, 0u64, true);
                for p in &targets {
                    let Ok(md) = std::fs::symlink_metadata(p) else {
                        continue;
                    };
                    if md.is_dir() {
                        folders += 1;
                        let s = gust_core::dirsize::dir_size(
                            p,
                            &cancel,
                            gust_core::dirsize::SizeLimits {
                                max_entries: 500_000,
                            },
                        );
                        files += s.files;
                        bytes += s.bytes;
                        complete &= s.complete;
                    } else {
                        files += 1;
                        bytes += md.len();
                    }
                }
                let text = delete_total_sentence(folders, files, bytes, complete);
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().delete_gen == gen {
                        model.as_mut().set_delete_total(QString::from(&text));
                    }
                });
            })
            .ok();
    }

    fn recent_list(&self) -> QString {
        let offset = gust_core::properties::local_offset_seconds();
        let items: Vec<String> = gust_core::recent::load(300)
            .into_iter()
            .map(|r| {
                // Stored in UTC; shown in local time like every other date.
                let when = parse_iso_utc(&r.when)
                    .map(|t| gust_core::properties::full_time(t, offset))
                    .unwrap_or_else(|| r.when.replace('T', " "));
                // Never stat a network mount here; assume present.
                let exists = gust_core::stall::needs_probe(&r.path) || r.path.exists();
                format!(
                    "{{\"name\":{},\"folder\":{},\"path\":{},\"when\":{},\"exists\":{}}}",
                    Self::json_str(
                        &r.path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Self::json_str(
                        &r.path
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Self::json_str(&r.path.to_string_lossy()),
                    Self::json_str(&when),
                    exists
                )
            })
            .collect();
        QString::from(&format!("[{}]", items.join(",")))
    }

    fn open_path(&self, path: &QString) -> QString {
        let p = PathBuf::from(path.to_string());
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            return QString::from("That file is no longer there.");
        };
        use std::os::unix::fs::PermissionsExt;
        let mode = Some(md.permissions().mode());
        let name = p.file_name().map(|n| n.to_owned()).unwrap_or_default();
        if gust_core::mime::needs_trusted_launch(&name, mode) {
            return QString::from("Programs are not started from here.");
        }
        match gust_core::open::open_with_desktop(&p, mode) {
            Ok(_) => QString::default(),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn reveal(mut self: Pin<&mut Self>, path: &QString) {
        let p = PathBuf::from(path.to_string());
        let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else {
            return;
        };
        // Already showing that folder: navigate is a no-op there, so put
        // the cursor on the file with a relist instead.
        if gust_core::location::same_place(&self.as_ref().key, dir) {
            self.as_mut().relist_focusing(name.to_owned());
            return;
        }
        self.as_mut().rust_mut().focus_name = Some(name.to_owned());
        self.navigate(&QString::from(dir.to_string_lossy().as_ref()));
    }

    fn trash_list(&self) -> QString {
        let Some(t) = gust_ops::trash::TrashDir::home() else {
            return QString::from("[]");
        };
        let items: Vec<String> = gust_ops::trash::list(&t)
            .into_iter()
            .take(2000)
            .map(|e| {
                let when = e.deleted.replace('T', " ");
                format!(
                    "{{\"name\":{},\"shown\":{},\"from\":{},\"when\":{}}}",
                    Self::json_str(&e.trashed_name.to_string_lossy()),
                    Self::json_str(
                        &e.original
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Self::json_str(
                        &e.original
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ),
                    Self::json_str(when.get(..16).unwrap_or(&when))
                )
            })
            .collect();
        QString::from(&format!("[{}]", items.join(",")))
    }

    fn trash_restore(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let Some(t) = gust_ops::trash::TrashDir::home() else {
            return QString::from("no Trash folder");
        };
        let n = std::ffi::OsString::from(name.to_string());
        match gust_ops::trash::restore_named(&t, &n) {
            Ok(()) => {
                // The folder on screen may be where it went back to.
                self.as_mut().reload();
                QString::default()
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => QString::from(
                "Something with that name is there now. Rename or move it first; nothing was overwritten.",
            ),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn request_columns(mut self: Pin<&mut Self>) {
        let Some(here) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            self.as_mut().set_columns_side(QString::default());
            return;
        };
        let gen = self.as_ref().columns_gen.wrapping_add(1);
        self.as_mut().rust_mut().columns_gen = gen;
        let show_hidden = self.as_ref().show_hidden;
        let cursor = self
            .as_ref()
            .entry_at(self.as_ref().cursor)
            .map(|e| (e.name.clone(), e.kind == EntryKind::Dir));
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-columns".into())
            .spawn(move || {
                let deadline = std::time::Duration::from_secs(2);
                let names_json = |b: &gust_core::columns::Bounded, mark: Option<&std::ffi::OsStr>| {
                    let items: Vec<String> = b
                        .names
                        .iter()
                        .map(|(n, d)| {
                            format!(
                                "{{\"n\":{},\"d\":{},\"cur\":{}}}",
                                Self::json_str(&n.to_string_lossy()),
                                d,
                                mark.is_some_and(|m| m == n.as_os_str())
                            )
                        })
                        .collect();
                    format!(
                        "\"names\":[{}],\"more\":{},\"error\":{}",
                        items.join(","),
                        b.more,
                        Self::json_str(b.error.as_deref().unwrap_or(""))
                    )
                };
                // Parent column: the folder above, with this one marked.
                let parent = match (here.parent(), here.file_name()) {
                    (Some(p), Some(me)) => {
                        let b = gust_core::columns::list_names_bounded(p, 200, show_hidden, deadline);
                        format!(
                            "{{\"path\":{},{}}}",
                            Self::json_str(&p.to_string_lossy()),
                            names_json(&b, Some(me))
                        )
                    }
                    _ => "null".into(),
                };
                // Preview column: a folder's first entries, or a file's peek.
                let peek = match &cursor {
                    Some((name, true)) => {
                        let p = here.join(name);
                        let b = gust_core::columns::list_names_bounded(&p, 60, show_hidden, deadline);
                        format!(
                            "{{\"kind\":\"dir\",\"name\":{},{}}}",
                            Self::json_str(&name.to_string_lossy()),
                            names_json(&b, None)
                        )
                    }
                    Some((name, false)) => {
                        let p = here.join(name);
                        if gust_core::stall::needs_probe(&p) {
                            format!(
                                "{{\"kind\":\"file\",\"name\":{},\"kindWords\":\"\",\"sizeText\":\"\",\"lines\":[],\"noPreview\":\"Previews are off on network places\"}}",
                                Self::json_str(&name.to_string_lossy())
                            )
                        } else {
                            let pk = gust_core::peek::peek(&p);
                            let lines: Vec<String> =
                                pk.text.iter().take(40).map(|l| Self::json_str(l)).collect();
                            format!(
                                "{{\"kind\":\"file\",\"name\":{},\"kindWords\":{},\"sizeText\":{},\"mode\":{},\"lines\":[{}],\"noPreview\":{}}}",
                                Self::json_str(&pk.name),
                                Self::json_str(&pk.kind_words),
                                Self::json_str(&human_size(pk.size)),
                                Self::json_str(&pk.mode_line),
                                lines.join(","),
                                Self::json_str(pk.no_preview_reason.as_deref().unwrap_or(""))
                            )
                        }
                    }
                    None => "null".into(),
                };
                let json = format!("{{\"parent\":{parent},\"peek\":{peek}}}");
                let _ = thread.queue(move |mut model| {
                    if model.as_ref().columns_gen == gen {
                        model.as_mut().set_columns_side(QString::from(&json));
                    }
                });
            })
            .ok();
    }

    fn settings_json(&self) -> QString {
        use gust_core::settings::Settings;
        let st = Settings::default_path()
            .map(|p| Settings::load(&p))
            .unwrap_or_default();
        let is_default = gust_core::defaults::mimeapps_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| gust_core::defaults::MimeApps::parse(&t))
            .and_then(|m| m.get("inode/directory"))
            .as_deref()
            == Some(gust_core::defaults::GUST_DESKTOP);
        let now = if std::env::var("QT_QUICK_BACKEND").as_deref() == Ok("software") {
            "software"
        } else {
            "gpu"
        };
        let fields = [
            ("key_preset", Self::json_str(st.key_preset_name())),
            ("show_hidden", st.show_hidden.to_string()),
            ("sidebar", st.sidebar.to_string()),
            ("density", Self::json_str(st.density_name())),
            ("renderer", Self::json_str(st.renderer_name())),
            ("restore_tabs", st.restore_tabs.to_string()),
            ("spacing", st.density.spacing().to_string()),
            ("is_default", is_default.to_string()),
            (
                "omarchy_keys",
                gust_core::omarchy_keys::bindings_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .is_some_and(|t| gust_core::omarchy_keys::has_block(&t))
                    .to_string(),
            ),
            (
                "file_dialogs",
                gust_core::portal_conf::paths()
                    .and_then(|(u, _)| std::fs::read_to_string(u).ok())
                    .is_some_and(|t| gust_core::portal_conf::is_claimed(&t))
                    .to_string(),
            ),
            (
                "is_omarchy",
                gust_core::omarchy_keys::bindings_path()
                    .is_some_and(|p| p.exists())
                    .to_string(),
            ),
            ("renderer_now", Self::json_str(now)),
        ];
        let body: Vec<String> = fields
            .iter()
            .map(|(k, v)| format!("{}:{}", Self::json_str(k), v))
            .collect();
        QString::from(&format!("{{{}}}", body.join(",")))
    }

    fn settings_set(&self, key: &QString, value: &QString) -> QString {
        if admin_session() {
            return QString::from("Settings belong to your normal Gust window.");
        }
        use gust_core::settings::Settings;
        let Some(path) = Settings::default_path() else {
            return QString::from("no home folder to keep settings in");
        };
        let mut st = Settings::load(&path);
        if let Err(e) = st.set(&key.to_string(), &value.to_string()) {
            return QString::from(&e);
        }
        match st.save(&path) {
            Ok(()) => QString::default(),
            Err(e) => QString::from(&format!("could not save settings: {e}")),
        }
    }

    fn set_default_file_manager(&self, on: bool) -> QString {
        let Ok(exe) = std::env::current_exe() else {
            return QString::from("cannot find the gust program");
        };
        match std::process::Command::new(exe)
            .arg("--default")
            .arg(if on { "on" } else { "off" })
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => QString::default(),
            Ok(o) => QString::from(String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn chooser_mode(&self) -> QString {
        QString::from(&std::env::var("GUST_CHOOSER").unwrap_or_default())
    }

    fn chooser_title(&self) -> QString {
        QString::from(&std::env::var("GUST_CHOOSER_TITLE").unwrap_or_default())
    }

    fn chooser_name(&self) -> QString {
        QString::from(&std::env::var("GUST_CHOOSER_NAME").unwrap_or_default())
    }

    fn chooser_accept(&self, save_name: &QString) -> QString {
        let Some(out) = std::env::var_os("GUST_CHOOSER_RESULT").map(PathBuf::from) else {
            return QString::from("not choosing for another app");
        };
        let mode = std::env::var("GUST_CHOOSER").unwrap_or_default();
        let here = self.key.as_path().map(|p| p.to_path_buf());
        let picked: Vec<PathBuf> = match mode.as_str() {
            "save" => {
                let name = save_name.to_string();
                let bare = std::path::Path::new(&name)
                    .file_name()
                    .map(|n| n.to_owned());
                let (Some(dir), Some(bare)) = (here, bare) else {
                    return QString::from("type a file name");
                };
                if name.contains('/') {
                    return QString::from("a file name cannot contain /");
                }
                vec![dir.join(bare)]
            }
            "open-folder" => {
                // The folder under the cursor if it is one, else this folder.
                let cur = self.path_of_row(self.cursor).filter(|p| p.is_dir());
                match cur.or(here) {
                    Some(p) => vec![p],
                    None => return QString::from("choose a folder"),
                }
            }
            _ => {
                let mut v = self.target_paths();
                v.retain(|p| !p.is_dir());
                if mode != "open-many" {
                    v.truncate(1);
                }
                if v.is_empty() {
                    return QString::from("choose a file (folders open with Enter)");
                }
                v
            }
        };
        match gust_core::chooser::write_result(&out, &picked) {
            Ok(()) => QString::default(),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn set_file_dialogs(&self, on: bool) -> QString {
        let Ok(exe) = std::env::current_exe() else {
            return QString::from("cannot find the gust program");
        };
        match std::process::Command::new(exe)
            .arg("--file-dialogs")
            .arg(if on { "on" } else { "off" })
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => QString::default(),
            Ok(o) => QString::from(String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn set_omarchy_keys(&self, on: bool) -> QString {
        let Ok(exe) = std::env::current_exe() else {
            return QString::from("cannot find the gust program");
        };
        match std::process::Command::new(exe)
            .arg("--bind-keys")
            .arg(if on { "on" } else { "off" })
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => QString::default(),
            Ok(o) => QString::from(String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => QString::from(&e.to_string()),
        }
    }

    fn bookmarks_json(&self) -> QString {
        let Some(file) = gust_core::bookmarks::default_path() else {
            return QString::from("[]");
        };
        let items: Vec<String> = gust_core::bookmarks::load(&file)
            .into_iter()
            .filter(|b| b.uri.is_none())
            .map(|b| {
                format!(
                    "{{\"label\":{},\"path\":{},\"exists\":{}}}",
                    Self::json_str(&b.display()),
                    Self::json_str(&b.path.to_string_lossy()),
                    // Never stat a network mount here (it can hang the
                    // window); those are listed as present and a click
                    // goes through the normal stall-guarded navigation.
                    gust_core::stall::needs_probe(&b.path) || b.path.is_dir()
                )
            })
            .collect();
        QString::from(&format!("[{}]", items.join(",")))
    }

    fn bookmark_add(&self, path: &QString) -> QString {
        self.bookmark_edit(path, true)
    }

    fn bookmark_remove(&self, path: &QString) -> QString {
        self.bookmark_edit(path, false)
    }

    fn bookmark_edit(&self, path: &QString, add: bool) -> QString {
        let Some(file) = gust_core::bookmarks::default_path() else {
            return QString::from("no home folder to keep bookmarks in");
        };
        let mut list = gust_core::bookmarks::load(&file);
        let p = PathBuf::from(path.to_string());
        if add {
            gust_core::bookmarks::add(&mut list, p);
        } else {
            gust_core::bookmarks::remove(&mut list, &p);
        }
        match gust_core::bookmarks::save(&file, &list) {
            Ok(()) => QString::default(),
            Err(e) => QString::from(&format!("could not save bookmarks: {e}")),
        }
    }

    fn acknowledge_interrupted(&self) {
        let Some(j) = Self::production_journal() else {
            return;
        };
        for i in j.interrupted() {
            let _ = j.acknowledge(&i.id);
        }
    }

    /// Park a detached child and reap any that have already exited.
    ///
    /// A detached window is a separate process on purpose (§6.20), so it is
    /// never waited on — but an exited child still needs reaping or it stays
    /// a zombie for the life of the parent. `try_wait` never blocks, so this
    /// is safe to call from the GUI thread.
    fn reap_detached(new_child: Option<std::process::Child>) {
        use std::sync::{Mutex, OnceLock};
        static DETACHED: OnceLock<Mutex<Vec<std::process::Child>>> = OnceLock::new();
        let list = DETACHED.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(mut g) = list.lock() else {
            return;
        };
        if let Some(c) = new_child {
            g.push(c);
        }
        g.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
    }

    /// Open this location in a new Gust window.
    ///
    /// A separate PROCESS, not another Window in this one. The plan asks for
    /// "a normal window that Hyprland can tile", and a second window inside
    /// one process shares that process's fate — one stuck mount would freeze
    /// both, which is the opposite of what §6.12 guarantees.
    fn detach_here(&self, view: &QString) -> QString {
        // A running job belongs to the process that started it. Handing this
        // location to a new process would leave the transfer running here
        // with nothing on screen to represent it.
        if self.job.is_some() {
            return QString::from(
                "A transfer is running in this tab. Wait for it, or cancel it first.",
            );
        }

        let Some(dir) = self.key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be opened in a new window.");
        };
        // The location must still be there: detaching into a folder that has
        // gone away opens a window showing home, which is not what was asked.
        //
        // This runs on the GUI thread, so it must never be an unbounded
        // `is_dir()`: on a stalled remote mount that call sits in
        // uninterruptible sleep and the whole window freezes instead of
        // refusing the drag (DESIGN §6.12). Anything that can stall is
        // checked in a supervised child with a deadline.
        if gust_core::stall::needs_probe(&dir) {
            match gust_core::stall::probe_if_risky(&dir, std::time::Duration::from_secs(4)) {
                r if !r.is_responsive() => {
                    return QString::from(r.message(&gust_core::stall::place_name_for(&dir)));
                }
                _ => {}
            }
        } else if !dir.is_dir() {
            // Local filesystems cannot stall, so this is safe here.
            return QString::from("That folder is no longer there.");
        }

        let Ok(exe) = std::env::current_exe() else {
            return QString::from("Gust could not find its own program to start.");
        };

        // The child must NOT inherit this process's QA state path: two
        // processes writing one state file makes every later assertion read
        // whichever wrote last, which silently invalidates a QA run.
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match std::env::var_os("GUST_QA_STATE") {
            Some(path) => {
                // Give the detached window its own sibling file so a test can
                // still observe it, without overwriting ours.
                let mut p = std::path::PathBuf::from(path);
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "state".to_string());
                p.set_file_name(format!("{stem}-detached-{}.json", std::process::id()));
                cmd.env("GUST_QA_STATE", p);
            }
            None => {
                cmd.env_remove("GUST_QA_STATE");
            }
        }

        // What the tab carries: history, selection, cursor and view, not
        // just the location (a detach used to open a bare folder).
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("gust");
        let _ = std::fs::create_dir_all(&runtime);
        let tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let handoff_path = runtime.join(format!("handoff-{tag}"));
        let ready_path = runtime.join(format!("ready-{tag}"));
        let visible = self.visible_entries();
        let handoff = gust_core::handoff::Handoff {
            location: dir.clone(),
            view: view.to_string(),
            cursor_name: self.entry_at(self.cursor).map(|e| e.name.clone()),
            selected: self
                .selection
                .resolve(&visible)
                .iter()
                .map(|e| e.name.clone())
                .collect(),
            back: self
                .back
                .iter()
                .filter_map(|k| k.as_path().map(|p| p.to_path_buf()))
                .collect(),
            show_hidden: self.show_hidden,
        };
        if handoff.write(&handoff_path).is_ok() {
            cmd.env("GUST_HANDOFF", &handoff_path);
        }
        cmd.env("GUST_READY_FILE", &ready_path);

        match cmd.spawn() {
            // Not waited on: the new window is independent. The tab stays
            // until the new window says it is on screen (poll_detach), so a
            // child that dies at startup never takes the tab with it.
            Ok(child) => {
                if let Ok(mut g) = DETACH_PENDING.lock() {
                    // A previous detach still pending is simply let go.
                    if let Some((c, _)) = g.take() {
                        Self::reap_detached(Some(c));
                    }
                    *g = Some((
                        child,
                        gust_core::handoff::ReadyWait::new(
                            ready_path,
                            std::time::Duration::from_secs(20),
                        ),
                    ));
                }
                QString::default()
            }
            Err(e) => {
                let _ = std::fs::remove_file(&handoff_path);
                QString::from(format!("Could not open a new window: {e}"))
            }
        }
    }

    fn poll_detach(&self) -> QString {
        let Ok(mut g) = DETACH_PENDING.lock() else {
            return QString::from("ready");
        };
        let Some((child, wait)) = g.as_mut() else {
            return QString::from("ready");
        };
        match wait.poll(child) {
            Ok(false) => QString::from("wait"),
            Ok(true) => {
                if let Some((c, _)) = g.take() {
                    Self::reap_detached(Some(c));
                }
                QString::from("ready")
            }
            Err(why) => {
                if let Some((c, _)) = g.take() {
                    Self::reap_detached(Some(c));
                }
                QString::from(&why)
            }
        }
    }

    fn handoff_view(&self) -> QString {
        QString::from(&HANDOFF_VIEW.lock().map(|v| v.clone()).unwrap_or_default())
    }

    fn signal_ready(&self) {
        if let Some(p) = std::env::var_os("GUST_READY_FILE") {
            let _ = std::fs::write(&p, b"ok");
            std::env::remove_var("GUST_READY_FILE");
        }
    }

    // --- opening several files (DESIGN §6.18) ----------------------------

    /// Open the whole selection with its default handler.
    ///
    /// Returns "" when this is not a multi-file situation, so the caller
    /// falls back to ordinary single-row activation rather than this
    /// silently doing nothing.
    fn activate_selection(mut self: Pin<&mut Self>) -> QString {
        if admin_session() {
            return QString::from(ADMIN_REFUSAL);
        }
        let paths = self.as_ref().target_paths();
        if paths.len() < 2 {
            return QString::default();
        }
        if paths.len() > gust_core::open::with::MAX_OPEN_AT_ONCE {
            return QString::from(format!(
                "{} files is more than Gust will open at once — select up to {}.",
                paths.len(),
                gust_core::open::with::MAX_OPEN_AT_ONCE
            ));
        }

        // A folder in the selection makes this a navigation question rather
        // than an open, and an executable is refused exactly as it is for a
        // single file (§4). Either way, fall back to the cursor row.
        for p in &paths {
            let Ok(meta) = std::fs::symlink_metadata(p) else {
                return QString::default();
            };
            if meta.is_dir() {
                return QString::default();
            }
            if let Ok(m) = std::fs::metadata(p) {
                use std::os::unix::fs::PermissionsExt;
                let mode = m.permissions().mode();
                if gust_core::mime::needs_trusted_launch(
                    p.file_name().unwrap_or_default(),
                    Some(mode),
                ) {
                    return QString::default();
                }
            }
        }

        // Every file must be the same kind, or "the default handler" means
        // nothing: opening a PDF and a video together is two applications.
        let Some(first) = paths.first() else {
            return QString::default();
        };
        let Some(mime) = Self::mime_of(first) else {
            return QString::default();
        };
        for p in paths.iter().skip(1) {
            if Self::mime_of(p).as_deref() != Some(mime.as_str()) {
                // Mixed kinds: open them one at a time with their own
                // defaults, which is what the user would get by pressing
                // Enter on each.
                let mut opened = 0usize;
                for p in &paths {
                    let mode = std::fs::metadata(p).ok().map(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.permissions().mode()
                    });
                    if gust_core::open::open_with_desktop(p, mode).is_ok() {
                        opened += 1;
                    }
                }
                let msg = format!("Opened {opened} files");
                self.as_mut().set_status(QString::from(&msg));
                return QString::from(format!("OK:{msg}"));
            }
        }

        let handlers = gust_core::open::with::handlers_for(&mime);
        let Some(handler) = handlers.first() else {
            return QString::default();
        };
        let outcome = gust_core::open::with::launch_many(handler, &paths);
        let msg = outcome.message();
        match outcome {
            gust_core::open::with::MultiOpen::Refused(_) => QString::from(msg),
            _ => {
                self.as_mut().set_status(QString::from(&msg));
                QString::from(format!("OK:{msg}"))
            }
        }
    }

    // --- open with (DESIGN §6.17) ----------------------------------------

    /// The MIME type of a path, from the shared MIME database.
    ///
    /// `xdg-mime query filetype` prints a bare type with no prose, so there
    /// is nothing localized to scrape. Bounded, because it reads the file's
    /// magic bytes and the file may be on a remote.
    fn mime_of(path: &std::path::Path) -> Option<String> {
        let out = std::process::Command::new("xdg-mime")
            .arg("query")
            .arg("filetype")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if t.is_empty() || !t.contains('/') {
            None
        } else {
            Some(t)
        }
    }

    fn open_with_list(&self, row: i32) -> QString {
        let Some(path) = self.path_of_row(row) else {
            return QString::from("[]");
        };
        let Some(mime) = Self::mime_of(&path) else {
            return QString::from("[]");
        };
        let found = gust_core::open::with::handlers_for(&mime);

        let mut out = String::from("[");
        for (i, h) in found.iter().take(20).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"id":{},"name":{},"default":{},"terminal":{}}}"#,
                Self::json_str(&h.id),
                Self::json_str(&h.name),
                h.is_default,
                h.terminal
            ));
        }
        out.push(']');
        QString::from(out)
    }

    fn open_with_id(&self, row: i32, id: &QString) -> QString {
        if admin_session() {
            return QString::from(ADMIN_REFUSAL);
        }
        let Some(path) = self.path_of_row(row) else {
            return QString::from("That item is no longer there.");
        };
        let Some(mime) = Self::mime_of(&path) else {
            return QString::from("Gust could not tell what kind of file that is.");
        };
        let wanted = id.to_string();

        // The handler must be one the SYSTEM says can open this type. A
        // desktop id arriving from anywhere else is not a licence to launch
        // an arbitrary program.
        let Some(handler) = gust_core::open::with::handlers_for(&mime)
            .into_iter()
            .find(|h| h.id == wanted)
        else {
            return QString::from("That application is no longer available.");
        };

        match gust_core::open::with::launch(&handler, &path) {
            Ok(name) => {
                // Reported, not silent: the window does not change, so
                // without a line here nothing appears to have happened.
                QString::from(format!("OK:Opened with {name}"))
            }
            Err(e) => QString::from(format!("{e}")),
        }
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
        let claim = self.as_ref().archive_work.claim(vec![dir.clone()]);

        std::thread::Builder::new()
            .name("gust-compress".into())
            .spawn(move || {
                let _claim = claim;
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
        let claim = self
            .as_ref()
            .archive_work
            .claim(vec![archive.clone(), dir.clone()]);

        std::thread::Builder::new()
            .name("gust-extract".into())
            .spawn(move || {
                let _claim = claim;
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
        self: Pin<&mut Self>,
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
        // Moved into the worker and dropped there; never stored.
        let secret = password.to_string();
        let label = format!("{}/{}", share.server, share.share);
        self.place_op(
            "share",
            label.clone(),
            format!("Connecting to {label}…"),
            move || {
                // Bounded: a server that never answers fails after the deadline.
                match mount(&share, secret, &runtime, std::time::Duration::from_secs(25)) {
                    Ok(mp) => PlaceOutcome::Open {
                        path: mp,
                        status: format!("Connected to {label}"),
                    },
                    Err(e) => PlaceOutcome::Failed(e.message()),
                }
            },
        );
        QString::default()
    }

    // --- removable devices (DESIGN §6.13) --------------------------------

    fn watch_devices(mut self: Pin<&mut Self>) {
        if !self.as_ref().device_watches.is_empty() {
            return;
        }
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let mut watches = Vec::new();
        // by-diskseq changes on every add, remove and media change;
        // by-label and by-uuid change when a filesystem appears or goes
        // (a formatted stick). /dev itself is avoided: every terminal
        // opened there would wake Gust. Mounts are reported by the place
        // operations that make them.
        for dir in gust_core::devices::DEVICE_WATCH_DIRS {
            let thread = thread.clone();
            if let Ok(w) = gust_core::watch::DirWatch::start(std::path::Path::new(dir), move |_| {
                let _ = thread.queue(|model| model.request_devices());
            }) {
                watches.push(w);
            }
        }
        // Phones have no block device: a USB device node appearing or going
        // under /dev/bus/usb/<bus>/ is what says one was plugged in.
        let usb = gust_remote::phones::watch_dir();
        let mut buses: Vec<std::path::PathBuf> = std::fs::read_dir(&usb)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        buses.push(usb);
        for dir in buses {
            let thread = thread.clone();
            if let Ok(w) = gust_core::watch::DirWatch::start(&dir, move |_| {
                let _ = thread.queue(|model| model.request_devices());
            }) {
                watches.push(w);
            }
        }
        self.as_mut().rust_mut().device_watches = watches;
    }

    /// The last device list a worker built ("[]" until the first one).
    fn devices_json(&self) -> QString {
        QString::from(
            devices_cache()
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default()
                .as_str(),
        )
    }

    fn request_devices(self: Pin<&mut Self>) {
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("gust-devices".into())
            .spawn(move || {
                let json = Self::build_devices_json();
                if let Ok(mut g) = devices_cache().lock() {
                    *g = json;
                }
                let _ = thread.queue(|model| model.devices_changed());
            });
    }

    /// lsblk and the udisks check: call on a worker.
    fn build_devices_json() -> String {
        let found = gust_core::devices::sidebar_devices(gust_core::devices::list_devices());
        let can_act = gust_core::devices::udisks_available();
        let mut out = String::from("[");
        for (i, d) in found.iter().take(30).enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"name":{},"path":{},"label":{},"state":{},"mountPoint":{},"mounted":{},"empty":{},"canAct":{},"encrypted":{},"locked":{}}}"#,
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
                d.encrypted,
                d.is_locked(),
            ));
        }
        // Phones and cameras (no block device; read from sysfs and udev).
        let phones = gust_remote::phones::scan();
        let names = if phones.is_empty() {
            Vec::new()
        } else {
            gust_remote::phones::friendly_names()
        };
        let states = phone_states();
        let mut st = states.lock().unwrap_or_else(|e| e.into_inner());
        // A phone that was unplugged stops being waited for.
        st.retain(|id, _| phones.iter().any(|p| &p.id() == id));
        for p in phones.iter().take(10) {
            use gust_remote::phones::{backend_installed, PhoneState};
            let id = p.id();
            // A missing backend is the first thing to fix, whatever happened
            // before; an open phone is open regardless.
            let state = match st.get(&id).cloned() {
                Some(s @ PhoneState::Open(_)) => s,
                _ if !backend_installed(p.kind) => PhoneState::NeedsPackage(p.kind.package()),
                Some(PhoneState::NeedsPackage(_)) | None => PhoneState::Ready,
                Some(s) => s,
            };
            let mount = match &state {
                PhoneState::Open(m) => m.to_string_lossy().to_string(),
                _ => String::new(),
            };
            if out.len() > 1 {
                out.push(',');
            }
            out.push_str(&format!(
                r#"{{"name":{},"path":{},"label":{},"state":{},"stateKey":{},"mountPoint":{},"mounted":{},"empty":false,"canAct":true,"phone":{},"apps":{}}}"#,
                Self::json_str(&id),
                Self::json_str(&id),
                Self::json_str(&gust_remote::phones::display_name(p, &names)),
                Self::json_str(&state.words(p.kind)),
                Self::json_str(state.key()),
                Self::json_str(&mount),
                !mount.is_empty(),
                Self::json_str(p.kind.as_str()),
                p.apps_uri.is_some(),
            ));
        }
        out.push(']');
        out
    }

    fn set_phone_state(id: &str, state: gust_remote::phones::PhoneState) {
        if let Ok(mut g) = phone_states().lock() {
            g.insert(id.to_string(), state);
        }
    }

    fn phone_open(self: Pin<&mut Self>, id: QString, apps: bool) -> QString {
        use gust_remote::phones::{mount, scan, PhoneState};
        if self.as_ref().place_busy {
            return QString::from("Another connection is still in progress.");
        }
        let id = id.to_string();
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return QString::default();
        };
        if let Ok(mut c) = phone_cancels().lock() {
            c.remove(&id);
        }
        let refresh = thread.clone();
        self.place_op(
            "phone-open",
            id.clone(),
            "Opening the phone…".into(),
            move || {
                // It must still be plugged in, and it is looked up again here so
                // a stale or crafted id cannot reach gio.
                let Some(mut phone) = scan().into_iter().find(|p| p.id() == id) else {
                    Self::set_phone_state(&id, PhoneState::Ready);
                    return PlaceOutcome::Failed("That phone is no longer plugged in.".into());
                };
                if apps {
                    match phone.apps_uri.clone() {
                        Some(u) => phone.uri = u,
                        None => {
                            return PlaceOutcome::Failed("This device has no app files.".into())
                        }
                    }
                }
                Self::set_phone_state(&id, PhoneState::Opening);
                let _ = refresh.queue(|model| model.request_devices());
                let cancel_id = id.clone();
                let cancelled = move || {
                    phone_cancels()
                        .lock()
                        .map(|c| c.contains(&cancel_id))
                        .unwrap_or(false)
                };
                let wait_id = id.clone();
                let mut progress = move |s: PhoneState| {
                    Self::set_phone_state(&wait_id, s);
                    let _ = thread.queue(|model| model.request_devices());
                };
                match mount(
                    &phone,
                    std::time::Duration::from_secs(120),
                    &mut progress,
                    &cancelled,
                ) {
                    Ok(path) => {
                        Self::set_phone_state(&id, PhoneState::Open(path.clone()));
                        let name = gust_remote::phones::display_name(
                            &phone,
                            &gust_remote::phones::friendly_names(),
                        );
                        let label = if apps {
                            format!("{name} · app files")
                        } else {
                            name
                        };
                        if let Ok(mut g) = place_labels().lock() {
                            g.retain(|(r, _)| r != &path);
                            g.push((path.clone(), label));
                        }
                        PlaceOutcome::Open {
                            path,
                            status: "Opened the phone".into(),
                        }
                    }
                    Err(PhoneState::Ready) => {
                        Self::set_phone_state(&id, PhoneState::Ready);
                        PlaceOutcome::Done("Stopped waiting for the phone".into())
                    }
                    Err(state) => {
                        let words = state.words(phone.kind);
                        Self::set_phone_state(&id, state);
                        PlaceOutcome::Failed(words)
                    }
                }
            },
        );
        QString::default()
    }

    fn phone_cancel(self: Pin<&mut Self>, id: QString) {
        if let Ok(mut c) = phone_cancels().lock() {
            c.insert(id.to_string());
        }
        let _ = self;
    }

    fn phone_eject(self: Pin<&mut Self>, id: QString) -> QString {
        use gust_remote::phones::{local_path, scan, unmount, uris, PhoneState};
        let id = id.to_string();
        // Never pull a phone out from under a running copy.
        if self.as_ref().job.is_some() {
            return QString::from("A transfer is still running: eject when it has finished.");
        }
        self.place_op("phone-eject", id.clone(), "Ejecting…".into(), move || {
            let Some(phone) = scan().into_iter().find(|p| p.id() == id) else {
                Self::set_phone_state(&id, PhoneState::Ready);
                return PlaceOutcome::Done("Safe to unplug".into());
            };
            // Every folder it is open at (photos, app files), asked here on
            // the worker because gio can take a moment to answer.
            let open: Vec<std::path::PathBuf> =
                uris(&phone).iter().filter_map(|u| local_path(u)).collect();
            // Back on the GUI thread: leave the phone if the window is on it,
            // then unmount on a worker again.
            PlaceOutcome::Then(Box::new(move |mut model| {
                for m in &open {
                    model.as_mut().leave_if_inside(m);
                }
                model.place_op("phone-eject", id.clone(), "Ejecting…".into(), move || {
                    let mut failed = None;
                    for u in uris(&phone) {
                        if local_path(&u).is_none() {
                            continue;
                        }
                        let mut one = phone.clone();
                        one.uri = u;
                        if let Err(w) = unmount(&one) {
                            failed.get_or_insert(w);
                        }
                    }
                    if let Ok(mut g) = place_labels().lock() {
                        g.retain(|(r, _)| !open.iter().any(|m| m == r));
                    }
                    match failed {
                        None => {
                            Self::set_phone_state(&id, PhoneState::Ready);
                            PlaceOutcome::Done("Safe to unplug".into())
                        }
                        Some(w) => PlaceOutcome::Failed(w),
                    }
                });
            }))
        });
        QString::default()
    }

    fn device_mount(self: Pin<&mut Self>, path: QString) -> QString {
        use gust_core::devices::{udisks_available, DeviceError};
        if !udisks_available() {
            return QString::from(DeviceError::NoUdisks.message());
        }
        let dev = path.to_string();
        self.place_op(
            "device-mount",
            dev.clone(),
            "Mounting…".into(),
            move || {
                // The device must be one the kernel actually reports, so a
                // crafted path cannot be handed to udisks.
                let Some(found) = gust_core::devices::list_devices()
                    .into_iter()
                    .find(|d| d.path == dev)
                else {
                    return PlaceOutcome::Failed("That device is no longer there.".into());
                };
                if found.is_locked() {
                    return PlaceOutcome::Failed(
                        "That drive is encrypted. Click it to type its passphrase.".into(),
                    );
                }
                Self::mount_and_open(found.mount_target())
            },
        );
        QString::default()
    }

    /// Mount a block device through udisks and browse it. On a worker.
    fn mount_and_open(target: &str) -> PlaceOutcome {
        use gust_core::devices::mount_args;
        {
            {
                let out = std::process::Command::new("udisksctl")
                    .args(mount_args(target))
                    .stdin(std::process::Stdio::null())
                    .output();
                let out = match out {
                    Ok(o) => o,
                    Err(e) => return PlaceOutcome::Failed(format!("Could not mount: {e}")),
                };
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    return PlaceOutcome::Failed(gust_core::devices::mount_failure(&err));
                }
                // udisksctl prints "Mounted /dev/sdb1 at /run/media/...".
                let said = String::from_utf8_lossy(&out.stdout);
                if let Some(at) = said.rsplit_once(" at ") {
                    let mount = at.1.trim().trim_end_matches('.').to_string();
                    if std::path::Path::new(&mount).is_dir() {
                        return PlaceOutcome::Open {
                            path: mount.into(),
                            status: String::new(),
                        };
                    }
                }
                // Mounted, but the path could not be read back: say so rather
                // than claiming to have gone somewhere.
                PlaceOutcome::Failed(
                    "Mounted, but Gust could not tell where. Check the sidebar.".into(),
                )
            }
        }
    }

    fn device_unlock(self: Pin<&mut Self>, path: QString, passphrase: QString) -> QString {
        use gust_core::devices::{
            udisks_available, unlock_args, unlock_failure, unlocked_as, DeviceError,
        };
        // Taken out of the QString at once; the bytes are wiped after use.
        let mut secret = passphrase.to_string().into_bytes();
        if !udisks_available() {
            wipe(&mut secret);
            return QString::from(DeviceError::NoUdisks.message());
        }
        if secret.is_empty() {
            return QString::from("Type the drive's passphrase to unlock it.");
        }
        let dev = path.to_string();
        self.place_op(
            "device-unlock",
            dev.clone(),
            "Unlocking…".into(),
            move || {
                let mut secret = secret;
                let Some(found) = gust_core::devices::list_devices()
                    .into_iter()
                    .find(|d| d.path == dev)
                else {
                    wipe(&mut secret);
                    return PlaceOutcome::Failed("That drive is no longer there.".into());
                };
                if !found.encrypted {
                    wipe(&mut secret);
                    return Self::mount_and_open(found.mount_target());
                }
                let cleartext = if let Some(c) = found.cleartext.clone() {
                    // Unlocked already (another window, or a terminal).
                    wipe(&mut secret);
                    c
                } else {
                    let child = std::process::Command::new("udisksctl")
                        .args(unlock_args(&dev))
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn();
                    let mut child = match child {
                        Ok(c) => c,
                        Err(e) => {
                            wipe(&mut secret);
                            return PlaceOutcome::Failed(format!("Could not unlock: {e}"));
                        }
                    };
                    if let Some(mut stdin) = child.stdin.take() {
                        use std::io::Write;
                        let _ = stdin.write_all(&secret);
                        // Dropping stdin closes it: udisksctl reads to EOF.
                    }
                    wipe(&mut secret);
                    let out = match child.wait_with_output() {
                        Ok(o) => o,
                        Err(e) => return PlaceOutcome::Failed(format!("Could not unlock: {e}")),
                    };
                    if !out.status.success() {
                        return PlaceOutcome::Failed(unlock_failure(&String::from_utf8_lossy(
                            &out.stderr,
                        )));
                    }
                    match unlocked_as(&String::from_utf8_lossy(&out.stdout)) {
                        Some(c) => c,
                        None => return PlaceOutcome::Failed(
                            "Unlocked, but Gust could not tell where. Click the drive to open it."
                                .into(),
                        ),
                    }
                };
                Self::mount_and_open(&cleartext)
            },
        );
        QString::default()
    }

    fn device_unmount(self: Pin<&mut Self>, path: QString) -> QString {
        use gust_core::devices::{udisks_available, unmount_args, DeviceError};
        if !udisks_available() {
            return QString::from(DeviceError::NoUdisks.message());
        }
        let dev = path.to_string();

        // Never pull a filesystem out from under a running transfer.
        if self.as_ref().job.is_some() {
            return QString::from(DeviceError::Busy("A transfer".into()).message());
        }
        self.place_op(
            "device-eject",
            dev.clone(),
            "Ejecting…".into(),
            move || {
                let found = gust_core::devices::list_devices()
                    .into_iter()
                    .find(|d| d.path == dev);
                let mount = found.as_ref().and_then(|d| d.mount_point.clone());
                // An encrypted drive: unmount what is inside, then lock it,
                // so it is safe to unplug and asks again next time.
                let target = found
                    .as_ref()
                    .map(|d| d.mount_target().to_string())
                    .unwrap_or_else(|| dev.clone());
                let lock = found
                    .as_ref()
                    .is_some_and(|d| d.encrypted && !d.is_locked());
                // Back on the GUI thread: leave the device if the pane is on it,
                // then eject on a worker again.
                PlaceOutcome::Then(Box::new(move |mut model| {
                    if let Some(mp) = &mount {
                        model.as_mut().leave_if_inside(mp);
                    }
                    model.place_op(
                        "device-eject",
                        dev.clone(),
                        "Ejecting…".into(),
                        move || {
                            let out = if mount.is_some() {
                                std::process::Command::new("udisksctl")
                                    .args(unmount_args(&target))
                                    .stdin(std::process::Stdio::null())
                                    .output()
                            } else {
                                // Unlocked but never mounted: nothing to unmount.
                                std::process::Command::new("true").output()
                            };
                            match out {
                                Ok(o) if o.status.success() && lock => {
                                    let locked = std::process::Command::new("udisksctl")
                                        .args(gust_core::devices::lock_args(&dev))
                                        .stdin(std::process::Stdio::null())
                                        .output();
                                    match locked {
                                        Ok(l) if l.status.success() => {
                                            PlaceOutcome::Done("Locked · safe to remove".into())
                                        }
                                        _ => PlaceOutcome::Failed(
                                            "Unmounted, but the drive could not be locked again. \
                                             Close anything using it and eject once more."
                                                .into(),
                                        ),
                                    }
                                }
                                Ok(o) if o.status.success() => {
                                    PlaceOutcome::Done("Safe to remove".into())
                                }
                                Ok(o) => {
                                    let err = String::from_utf8_lossy(&o.stderr);
                                    PlaceOutcome::Failed(format!(
                                        "Could not eject: {}",
                                        err.lines()
                                            .last()
                                            .unwrap_or("udisks did not say why")
                                            .trim()
                                    ))
                                }
                                Err(e) => PlaceOutcome::Failed(format!("Could not eject: {e}")),
                            }
                        },
                    );
                }))
            },
        );
        QString::default()
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
        let Some(mp) = Self::cloud_mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        let rc_bind = Self::cloud_rc_addr(&name);
        let sock = Self::cloud_rc_socket(&name);
        let label = name.clone();
        self.as_mut().place_op("cloud-connect", name.clone(), format!("Connecting to {name}…"), move || {
            // The remote must be one the installed rclone actually has.
            match gust_remote::cloud::list_remotes(None) {
                Ok(remotes) if remotes.iter().any(|r| r.name == name) => {}
                Ok(_) => {
                    return PlaceOutcome::Failed(
                        gust_remote::cloud::CloudError::NoSuchRemote(name).message(),
                    )
                }
                Err(e) => return PlaceOutcome::Failed(e.message()),
            }
            if gust_remote::mount::is_mounted(&mp) {
                return PlaceOutcome::Open { path: mp, status: String::new() };
            }
            // Never mount over files left in the folder: they are set aside
            // first, and the user is told where.
            let recovered = match gust_remote::mount::prepare_mount_point(&mp, &name) {
                Ok(r) => r,
                Err(e) => return PlaceOutcome::Failed(e.message()),
            };
            if let Some(sock) = &sock {
                if let Some(dir) = sock.parent() {
                    use std::os::unix::fs::DirBuilderExt;
                    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
                }
                // A socket left by a crashed mount would make rclone refuse
                // to bind. It is ours by path and never user data.
                let _ = std::fs::remove_file(sock);
            }
            let cache = mp.parent().map(|p| p.join(".cache")).unwrap_or_default();
            let _ = std::fs::create_dir_all(&cache);

            // --vfs-cache-mode writes is what makes "cached" and "uploaded"
            // different moments; Gust reports the difference rather than
            // hiding it. No credential is passed here — rclone reads its
            // own config.
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
                // Safe ONLY because the address is a socket in a 0700
                // runtime directory; never with a TCP address.
                .arg("--rc-no-auth")
                // NOT --daemon: its readiness handshake reports "daemon
                // exited with error code 1" over a mount that came up
                // perfectly. The mount point itself is the only trustworthy
                // signal.
                .env("RCLONE_ASK_PASSWORD", "false")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    let _ = std::fs::remove_dir(&mp);
                    return PlaceOutcome::Failed(format!("Could not start rclone: {e}"));
                }
            };
            // A mount appears asynchronously. Watch both the mount point and
            // the child: if rclone gives up first, say so.
            for _ in 0..60 {
                if gust_remote::mount::is_mounted(&mp) {
                    // Reap the child from a detached thread so it never
                    // becomes a zombie when it eventually exits.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Self::opened(mp, format!("Connected to {label}"), &label, recovered);
                }
                if let Ok(Some(status)) = child.try_wait() {
                    let _ = std::fs::remove_dir(&mp);
                    return PlaceOutcome::Failed(format!(
                        "Could not connect to {name}: rclone stopped ({status}). Check it with: rclone lsd {name}:"
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            // Still nothing: leave nothing running behind.
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir(&mp);
            PlaceOutcome::Failed(format!("{name} did not finish connecting."))
        });
        QString::default()
    }

    fn cloud_disconnect(self: Pin<&mut Self>, name: QString, force: bool) -> QString {
        let name = name.to_string();
        let Some(mp) = Self::cloud_mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        let rc = Self::cloud_rc_addr(&name);
        let label = name.clone();
        self.place_op("cloud-disconnect", name.clone(), format!("Disconnecting {name}…"), move || {
            if !gust_remote::mount::is_mounted(&mp) {
                return PlaceOutcome::Failed(format!("{name} is not connected."));
            }
            // Never lazy-unmount a dirty mount (plan §9). Ask first, and say
            // how much work is outstanding.
            //
            // A query that did not answer is NOT an empty queue: unknown
            // state is refused, not assumed safe (gust_remote::disconnect).
            if !force {
                let report =
                    match gust_remote::cloud::vfs_pending(&rc, std::time::Duration::from_secs(3)) {
                        Ok(p) => gust_remote::disconnect::PendingReport::Known {
                            queued: p.queued,
                            in_progress: p.in_progress,
                            errored: p.errored,
                        },
                        Err(e) => gust_remote::disconnect::PendingReport::Unknown(e.message()),
                    };
                let guard = Self::cloud_tracker().lock();
                let tracker = guard.as_ref().ok().map(|g| &**g);
                match gust_remote::disconnect::verdict_for(&name, &report, tracker, Some(&mp), false) {
                    gust_remote::disconnect::DisconnectVerdict::Safe => {}
                    // Answerable: the user may choose to go ahead.
                    gust_remote::disconnect::DisconnectVerdict::Ask(msg) => {
                        return PlaceOutcome::Failed(format!("ASK:{msg}"));
                    }
                    // Not a yes/no question: offering a button that can only
                    // lose data is worse than refusing (DESIGN §6.8).
                    gust_remote::disconnect::DisconnectVerdict::Refuse(msg) => {
                        return PlaceOutcome::Failed(msg);
                    }
                }
            }
            PlaceOutcome::Then(Box::new(move |mut model| {
                // If the user is inside the mount, leave before pulling it.
                model.as_mut().leave_if_inside(&mp);
                model.place_op("cloud-disconnect", label.clone(), format!("Disconnecting {label}…"), move || {
                    // Navigating away is not instant: the listing worker can
                    // still hold the directory open for a moment, and
                    // fusermount3 then reports "busy" for a mount nobody is
                    // really using.
                    let mut last = String::new();
                    for attempt in 0..8 {
                        if attempt > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(400));
                        }
                        match std::process::Command::new("fusermount3").arg("-u").arg(&mp).output() {
                            Ok(o) if o.status.success() => {
                                // Only a directory Gust created, under the runtime dir.
                                let _ = std::fs::remove_dir(&mp);
                                return PlaceOutcome::Done(format!("Disconnected {label}"));
                            }
                            Ok(o) => last = String::from_utf8_lossy(&o.stderr).trim().to_string(),
                            Err(e) => last = e.to_string(),
                        }
                    }
                    // Never lazy-unmount to paper over this (plan §9).
                    PlaceOutcome::Failed(format!(
                        "Could not disconnect {label}: {last}. Close anything using those files and try again."
                    ))
                });
            }))
        });
        QString::default()
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
            Ok(entries) => {
                let ok = gust_remote::cloud::confirmed_on_backend(&entries, &file, size);
                if ok {
                    // The backend has this object at this size: the file is
                    // no longer work that exists only on this computer, so it
                    // must stop blocking a disconnect.
                    if let Ok(mut t) = Self::cloud_tracker().lock() {
                        t.set(&local, gust_remote::upload::UploadState::Confirmed, None);
                    }
                }
                ok
            }
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
    fn remote_connect(self: Pin<&mut Self>, alias: QString) -> QString {
        let alias = alias.to_string();
        let Some(cfg) = gust_remote::ssh_config::user_config_path() else {
            return QString::from("No SSH configuration found.");
        };
        let Some(mp) = gust_remote::mount::mount_point_for(&alias) else {
            return QString::from("That name cannot be used for a place.");
        };
        self.place_op(
            "remote-connect",
            alias.clone(),
            format!("Connecting to {alias}…"),
            move || {
                let hosts = gust_remote::ssh_config::parse_ssh_config(&cfg);
                let Some(h) = hosts.iter().find(|h| h.alias == alias) else {
                    return PlaceOutcome::Failed(format!("No saved computer called \"{alias}\"."));
                };
                if gust_remote::mount::is_mounted(&mp) {
                    return PlaceOutcome::Open {
                        path: mp,
                        status: String::new(),
                    };
                }
                let mut spec = gust_remote::mount::RemoteSpec::new(
                    &alias,
                    h.hostname.clone().unwrap_or_else(|| alias.clone()),
                );
                spec.user = h.user.clone();
                spec.port = h.port;
                spec.config_file = Some(cfg);
                // The alias names a whole Host block: identity, known_hosts,
                // port, proxy. Use it verbatim, or all of that is dropped.
                spec.use_alias_as_target = true;
                // Where the place opens. Without an explicit folder a connection
                // lands in the login directory, which is that account's home.
                spec.remote_path = h.remote_path.clone();
                // Bounded: sshfs is given a connect timeout.
                match gust_remote::mount::mount_recovering(
                    &spec,
                    &mp,
                    std::time::Duration::from_secs(20),
                ) {
                    Ok(rec) => Self::opened(mp, format!("Connected to {alias}"), &alias, rec),
                    Err(e) => PlaceOutcome::Failed(e.message()),
                }
            },
        );
        QString::default()
    }

    fn remote_disconnect(mut self: Pin<&mut Self>, alias: QString) -> QString {
        let alias = alias.to_string();
        let Some(mp) = gust_remote::mount::mount_point_for(&alias) else {
            return QString::from("That name cannot be used for a place.");
        };
        // Leaving the folder first: unmounting what is on screen is how a
        // listing worker ends up reading a filesystem that just vanished.
        self.as_mut().leave_if_inside(&mp);
        self.place_op(
            "remote-disconnect",
            alias.clone(),
            format!("Disconnecting {alias}…"),
            move || match gust_remote::mount::unmount(&mp) {
                Ok(()) => PlaceOutcome::Done(format!("Disconnected from {alias}")),
                Err(e) => PlaceOutcome::Failed(e.message()),
            },
        );
        QString::default()
    }

    // --- every connectable place (DESIGN §6.4.1) --------------------------

    fn request_sharing(mut self: Pin<&mut Self>) {
        if self.sharing_checking || self.sharing_busy {
            return;
        }
        let Some(thread) = self.qt_thread_handle() else {
            return;
        };
        let home = PathBuf::from(self.home_path.to_string());
        self.as_mut().set_sharing_checking(true);
        let spawned = std::thread::Builder::new().name("gust-sharing-state".into()).spawn(move || {
            let snapshot = std::panic::catch_unwind(|| {
                let cancel = CancelToken::new();
                let snapshot = gust_remote::sharing::snapshot(&home, &gust_remote::helper::CallContext {
                    cancel: &cancel, timeout: gust_remote::sharing::discovery_timeout(),
                });
                serde_json::to_string(&snapshot).unwrap_or_default()
            }).unwrap_or_else(|_| r#"{"send_error":"Sharing could not be checked. Check again.","drive_error":"Sharing could not be checked. Check again."}"#.into());
            let _ = thread.queue(move |mut model| {
                model.as_mut().set_sharing_json(QString::from(&snapshot));
                model.as_mut().set_sharing_checking(false);
            });
        });
        if spawned.is_err() {
            self.as_mut().set_sharing_checking(false);
            self.as_mut().set_sharing_json(QString::from(r#"{"send_error":"Could not start checking sharing. Check again.","drive_error":"Could not start checking sharing. Check again."}"#));
        }
    }

    fn prepare_device_send(mut self: Pin<&mut Self>, row: i32) -> QString {
        if self.sharing_busy {
            return QString::default();
        }
        self.as_mut().set_cursor_if_no_selection(row);
        let paths = self.target_paths();
        let description = if paths.len() == 1 {
            gust_core::location::escape_for_display(paths[0].file_name().unwrap_or_default())
        } else {
            format!("{} selected items", paths.len())
        };
        self.as_mut().rust_mut().sharing_work.prepared = paths;
        QString::from(&description)
    }

    fn device_folder(&self, row: i32) -> QString {
        let path = if row < 0 {
            self.key.as_path().map(Path::to_path_buf)
        } else {
            self.path_of_row(row)
        };
        let descriptor = path.as_ref().and_then(|p| gust_core::location::PathToken::from_path(p)).map(|p|
            serde_json::json!({"path": gust_core::location::escape_for_display(p.as_path().as_os_str()), "token": p.to_token()}))
            .unwrap_or_else(|| serde_json::json!({"path": "This location cannot be shared", "token": ""}));
        QString::from(&descriptor.to_string())
    }

    fn cancel_device_share(self: Pin<&mut Self>) {
        self.sharing_work.cancel.cancel();
    }

    fn device_share(mut self: Pin<&mut Self>, action: QString, value: QString) -> QString {
        if self.sharing_busy || self.sharing_checking {
            return QString::from("Wait for the current sharing action to finish.");
        }
        let action = action.to_string();
        if !matches!(
            action.as_str(),
            "send" | "receive" | "publish" | "unpublish"
        ) {
            return QString::from("That sharing action is unsupported.");
        }
        let Some(thread) = self.qt_thread_handle() else {
            return QString::from("Sharing is not ready.");
        };
        let home = PathBuf::from(self.home_path.to_string());
        let paths = self.sharing_work.prepared.clone();
        if action == "send" && paths.is_empty() {
            return QString::from("Select local files to send first.");
        }
        let value = value.to_string();
        let folder = if action == "publish" || action == "unpublish" {
            let Some(token) = gust_core::location::PathToken::parse(&value) else {
                return QString::from("That folder identity is invalid. Choose the folder again.");
            };
            if token.as_path().to_str().is_none() {
                return QString::from("Tailscale folder sharing needs a UTF-8 folder path. Choose another folder; no name was changed.");
            }
            Some(token.as_path().to_path_buf())
        } else {
            None
        };
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().sharing_work.cancel = cancel.clone();
        self.as_mut().rust_mut().sharing_work.active_paths = if action == "send" {
            paths.clone()
        } else if action == "publish" {
            folder.iter().cloned().collect()
        } else {
            Vec::new()
        };
        self.as_mut().set_sharing_busy(true);
        let spawned = std::thread::Builder::new().name("gust-device-sharing".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(String, PathBuf), String> {
                use gust_remote::{helper::CallContext, sharing};
                let ctx = CallContext { cancel: &cancel, timeout: std::time::Duration::from_secs(30 * 60) };
                match action.as_str() {
                    "send" => {
                        sharing::eligible_target(&value, &CallContext { cancel: &cancel, timeout: std::time::Duration::from_secs(5) }).map_err(|e| e.message)?;
                        match gust_remote::send::taildrop(&value, &paths, &ctx) {
                            Ok(receipt) => Ok((format!("{} file(s) accepted by the receiving device. Your originals stay here. Open Tailscale on the device to finish saving them.", receipt.accepted_files), PathBuf::new())),
                            Err(failure) => Err(format!("{} {} file(s) were already accepted. {}", failure.error.message,
                                failure.accepted_files, if failure.delivery_uncertain { "Other files may also have arrived. Check the receiver before retrying." } else { "Your originals stay here." })),
                        }
                    }
                    "receive" => {
                        let received = sharing::receive(&home, &ctx).map_err(|e| e.message)?;
                        Ok((if received.files == 0 { "No files are waiting. On your phone or tablet, select a file or photo, tap Share, choose Tailscale, then choose this computer.".into() }
                            else { format!("{} file(s) received in Gust Inbox. Nothing was opened automatically.", received.files) }, received.path))
                    }
                    "publish" => {
                        let path = folder.as_ref().ok_or("No folder was selected.")?;
                        sharing::publish(&home, path, &CallContext { cancel: &cancel, timeout: std::time::Duration::from_secs(15) }).map_err(|e| e.message)?;
                        Ok(("Devices can open this folder now. In the Files app on your phone or tablet, choose Browse → Tailscale → your network → this computer → gust. Access follows your Tailscale policy.".into(), path.clone()))
                    }
                    "unpublish" => {
                        let path = folder.as_ref().and_then(|p| p.to_str()).ok_or("No folder was selected.")?;
                        sharing::unpublish(path, &CallContext { cancel: &cancel, timeout: std::time::Duration::from_secs(15) }).map_err(|e| e.message)?;
                        Ok(("Devices can no longer open this folder. All files stay on this computer.".into(), PathBuf::new()))
                    }
                    _ => unreachable!(),
                }
            })).unwrap_or_else(|_| Err("The sharing worker stopped unexpectedly. Some work may have completed; check sharing and the receiver before retrying.".into()));
            let _ = thread.queue(move |mut model| {
                model.as_mut().rust_mut().sharing_work.active_paths.clear();
                model.as_mut().set_sharing_busy(false);
                let (success, message, path) = match result {
                    Ok((message, path)) => (true, message, path),
                    Err(message) => (false, message, PathBuf::new()),
                };
                model.as_mut().sharing_done(QString::from(&action), success, QString::from(&message), QString::from(path.to_string_lossy().as_ref()));
                model.as_mut().request_sharing();
            });
        });
        if spawned.is_err() {
            self.as_mut().rust_mut().sharing_work.active_paths.clear();
            self.as_mut().set_sharing_busy(false);
            return QString::from("Could not start the sharing worker. Nothing was started.");
        }
        QString::default()
    }

    fn request_places(mut self: Pin<&mut Self>) {
        if self.as_ref().places_refreshing {
            self.as_mut().rust_mut().places_again = true;
            return;
        }
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        self.as_mut().rust_mut().places_refreshing = true;
        let spawned = std::thread::Builder::new()
            .name("gust-places".into())
            .spawn(move || {
                let json = Self::places_snapshot();
                let _ = thread.queue(move |mut model| {
                    model.as_mut().rust_mut().places_refreshing = false;
                    let again = std::mem::take(&mut model.as_mut().rust_mut().places_again);
                    model.as_mut().set_places_json(QString::from(&json));
                    if again {
                        model.as_mut().request_places();
                    }
                });
            });
        if spawned.is_err() {
            self.as_mut().rust_mut().places_refreshing = false;
            self.as_mut().rust_mut().places_again = false;
        }
    }

    /// SSH hosts, Tailscale devices and cloud remotes, with their state.
    /// Runs on a worker. Every source here is local: a config file, the
    /// local tailscaled socket, rclone's config. No device is contacted
    /// except mounted places, each probed by a killable child.
    fn places_snapshot() -> String {
        use gust_remote::{computers, mount, ssh_config, tailscale};
        use std::sync::{Mutex, OnceLock};
        static PEERS: OnceLock<Mutex<tailscale::PeerCache>> = OnceLock::new();
        static KEYS: OnceLock<Mutex<computers::TrustedKeyCache>> = OnceLock::new();
        let current = tailscale::peers(std::time::Duration::from_secs(3));
        let snapshot = PEERS
            .get_or_init(|| Mutex::new(tailscale::PeerCache::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(current);
        let local_user = std::env::var("USER").unwrap_or_default();
        let local_user = if tailscale::safe_user(&local_user) {
            local_user
        } else {
            String::new()
        };
        let hosts = ssh_config::user_config_path()
            .map(|p| ssh_config::parse_ssh_config(&p))
            .unwrap_or_default();
        let mut routes = Vec::new();
        for h in hosts.iter().take(50) {
            let explicit: Vec<_> = h
                .hostname
                .as_ref()
                .filter(|host| tailscale::safe_target(host))
                .map(|host| computers::HostEndpoint {
                    host: host.clone(),
                    port: h.port.unwrap_or(22),
                })
                .into_iter()
                .collect();
            // Saved aliases are executed with -F, exactly as before this feature;
            // their system-wide config is not part of that existing execution path.
            let certain = h.identity_certain && !h.has_proxy;
            let policy = computers::route_policy(
                &explicit,
                &snapshot.peers,
                snapshot.available,
                certain,
                h.has_proxy,
            );
            routes.push(computers::IdentifiedRoute {
                route: computers::ComputerRoute {
                    kind: computers::RouteKind::Ssh,
                    name: h.alias.clone(),
                    target: h.alias.clone(),
                    detail: h.hostname.clone().unwrap_or_else(|| h.alias.clone()),
                    user: h.user.clone(),
                    path: mount::mount_point_for(&h.alias)
                        .map(|p| p.to_string_lossy().into_owned()),
                    port: h.port,
                    remote_path: h.remote_path.clone(),
                    connected: false,
                    lost: false,
                    online: policy.online,
                    available: true,
                    requires_tailnet: policy.requires_tailnet,
                    endpoint_ids: policy.endpoint_ids,
                },
                endpoints: if certain { explicit } else { Vec::new() },
                trusted_host_keys: Vec::new(),
            });
        }
        let users = tailscale::users_path();
        let mobile: Vec<_> = snapshot
            .peers
            .iter()
            .filter(|p| gust_remote::sharing::is_apple_mobile(&p.os))
            .map(|p| {
                serde_json::json!({"name": p.name, "target": p.target, "os": p.os,
                "online": p.online, "available": snapshot.available})
            })
            .collect();
        for peer in snapshot.peers.iter().take(200) {
            if gust_remote::sharing::is_apple_mobile(&peer.os) {
                continue;
            }
            let user = users
                .as_ref()
                .and_then(|p| tailscale::remembered_user(p, &peer.name))
                .unwrap_or_else(|| local_user.clone());
            let endpoints: Vec<_> = std::iter::once(peer.target.clone())
                .chain(peer.ips.clone())
                .chain(peer.dns_name.clone())
                .map(|host| computers::HostEndpoint { host, port: 22 })
                .collect();
            let policy = computers::route_policy(
                &endpoints,
                &snapshot.peers,
                snapshot.available,
                true,
                false,
            );
            routes.push(computers::IdentifiedRoute {
                route: computers::ComputerRoute {
                    kind: computers::RouteKind::Tailscale,
                    name: peer.name.clone(),
                    target: peer.target.clone(),
                    detail: peer.target.clone(),
                    user: Some(user),
                    path: tailscale::mount_point(&peer.name)
                        .map(|p| p.to_string_lossy().into_owned()),
                    port: None,
                    remote_path: None,
                    connected: false,
                    lost: false,
                    online: peer.online,
                    available: snapshot.available,
                    requires_tailnet: true,
                    endpoint_ids: policy.endpoint_ids,
                },
                endpoints,
                trusted_host_keys: Vec::new(),
            });
        }
        // Gather all local identity evidence BEFORE probing mounts. Cached results
        // remain stable when a dead filesystem consumes its separate probe budget.
        if let Some(kh) = tailscale::known_hosts_path() {
            let endpoints: Vec<_> = routes
                .iter()
                .flat_map(|r| r.endpoints.iter().cloned())
                .collect();
            let found = KEYS
                .get_or_init(|| Mutex::new(computers::TrustedKeyCache::default()))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .lookup_batch(&kh, &endpoints, std::time::Duration::from_millis(750));
            let mut found = found.into_iter();
            for route in &mut routes {
                for _ in &route.endpoints {
                    for key in found.next().and_then(Result::ok).unwrap_or_default() {
                        if !route.trusted_host_keys.contains(&key) {
                            route.trusted_host_keys.push(key);
                        }
                    }
                }
            }
        }
        for input in &mut routes {
            let r = &mut input.route;
            if let Some(path) = r.path.as_ref().map(std::path::Path::new) {
                r.connected = mount::is_mounted(path);
                r.lost = r.connected
                    && ((r.kind == computers::RouteKind::Tailscale && !snapshot.available)
                        || !gust_core::stall::probe(path, std::time::Duration::from_millis(500))
                            .is_responsive());
                if r.connected && !r.lost {
                    r.online = true;
                }
            }
        }
        let ssh: Vec<_> = routes
            .iter()
            .filter(|r| r.route.kind == computers::RouteKind::Ssh)
            .map(|r| r.route.clone())
            .collect();
        let tail: Vec<_> = routes
            .iter()
            .filter(|r| r.route.kind == computers::RouteKind::Tailscale)
            .map(|r| {
                let mut value = serde_json::to_value(&r.route).unwrap_or_default();
                value["os"] = serde_json::json!(snapshot
                    .peers
                    .iter()
                    .find(|p| p.name == r.route.name)
                    .map(|p| p.os.as_str())
                    .unwrap_or(""));
                value
            })
            .collect();
        let (mut cloud, mut cloud_error) = (Vec::new(), String::new());
        match gust_remote::cloud::list_remotes(None) {
            Ok(remotes) => {
                for remote in remotes.iter().take(50) {
                    let path = Self::cloud_mount_point(&remote.name);
                    let connected = path.as_ref().is_some_and(|p| mount::is_mounted(p));
                    cloud.push(
                        serde_json::json!({"name": remote.name, "detail": remote.backend,
                    "connected": connected, "lost": false,
                    "path": path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()}),
                    );
                }
            }
            Err(error) => cloud_error = error.message(),
        }
        serde_json::json!({"computers": computers::group_computers(routes), "ssh": ssh, "tailnet": tail, "mobile": mobile,
            "tailnetError": snapshot.error, "cloud": cloud, "cloudError": cloud_error,
            "sshfs": mount::Backend::Sshfs.available(), "setup": gust_remote::setup::snapshot(),
            "localUser": local_user}).to_string()
    }

    /// Host keys shown to the user, by target: a "Trust and connect"
    /// records exactly these, never a fresh scan that could differ from
    /// what was on screen.
    fn pending_host_keys(
    ) -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<String>>> {
        use std::sync::{Mutex, OnceLock};
        static K: OnceLock<Mutex<std::collections::HashMap<String, Vec<String>>>> = OnceLock::new();
        K.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }

    fn tailnet_connect(
        self: Pin<&mut Self>,
        name: QString,
        target: QString,
        user: QString,
        trust: bool,
    ) -> QString {
        use gust_remote::{mount, tailscale};
        let (name, target, user) = (
            name.to_string(),
            target.to_string(),
            user.to_string().trim().to_string(),
        );
        let Some(mp) = tailscale::mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        if !tailscale::safe_target(&target) {
            return QString::from("That is not a usable address.");
        }
        if !tailscale::safe_user(&user) {
            return QString::from("That is not a usable login name.");
        }
        let Some(kh) = tailscale::known_hosts_path() else {
            return QString::from("No home folder to keep host keys in.");
        };
        self.place_op(
            "tailnet-connect",
            name.clone(),
            format!("Connecting to {name}…"),
            move || {
                if mount::is_mounted(&mp) {
                    return PlaceOutcome::Open { path: mp, status: String::new() };
                }
                if !mount::Backend::Sshfs.available() {
                    return PlaceOutcome::Failed(mount::MountError::ToolMissing("sshfs").message());
                }
                // A device whose SSH key is not on record: show its
                // fingerprint and let the user decide. Never trust silently,
                // never turn checking off.
                if !tailscale::host_known(&kh, &target) {
                    if trust {
                        let lines = Self::pending_host_keys()
                            .lock()
                            .ok()
                            .and_then(|mut m| m.remove(&target));
                        let Some(lines) = lines else {
                            return PlaceOutcome::Failed("Connect again to see the device's key.".into());
                        };
                        if let Err(e) = tailscale::trust_host_keys(&kh, &lines) {
                            return PlaceOutcome::Failed(format!("Could not record the key: {e}"));
                        }
                    } else {
                        return match tailscale::scan_host_keys(&target) {
                            Ok(keys) => {
                                if let Ok(mut m) = Self::pending_host_keys().lock() {
                                    m.insert(target.clone(), keys.lines);
                                }
                                let fps: Vec<String> =
                                    keys.fingerprints.iter().map(|f| Self::json_str(f)).collect();
                                PlaceOutcome::Failed(format!(
                                    r#"ASKKEY:{{"name":{},"target":{},"user":{},"fingerprints":[{}]}}"#,
                                    Self::json_str(&name),
                                    Self::json_str(&target),
                                    Self::json_str(&user),
                                    fps.join(",")
                                ))
                            }
                            Err(why) => PlaceOutcome::Failed(why),
                        };
                    }
                }
                if let Some(u) = tailscale::users_path() {
                    let _ = tailscale::remember_user(&u, &name, &user);
                }
                let mut spec = mount::RemoteSpec::new(&name, target.clone());
                spec.user = Some(user.clone());
                // The same file the key was checked against and recorded in.
                spec.known_hosts = Some(kh);
                match mount::mount_recovering(&spec, &mp, std::time::Duration::from_secs(20)) {
                    Ok(rec) => Self::opened(mp, format!("Connected to {name}"), &name, rec),
                    Err(e) => PlaceOutcome::Failed(e.message()),
                }
            },
        );
        QString::default()
    }

    fn tailnet_disconnect(mut self: Pin<&mut Self>, name: QString) -> QString {
        let name = name.to_string();
        let Some(mp) = gust_remote::tailscale::mount_point(&name) else {
            return QString::from("That name cannot be used for a place.");
        };
        self.as_mut().leave_if_inside(&mp);
        self.place_op(
            "tailnet-disconnect",
            name.clone(),
            format!("Disconnecting {name}…"),
            move || match gust_remote::mount::unmount(&mp) {
                Ok(()) => PlaceOutcome::Done(format!("Disconnected from {name}")),
                Err(e) => PlaceOutcome::Failed(e.message()),
            },
        );
        QString::default()
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
        // started from within results must still return to the real folder,
        // and to the row the user was on.
        if self.as_ref().find_origin.is_none() {
            let here = {
                let r = self.as_ref();
                r.entry_at(r.cursor).map(|e| e.name.clone())
            };
            self.as_mut().rust_mut().find_origin = Some(key.clone());
            self.as_mut().rust_mut().find_return = here;
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
                    // Typing quickly starts a find per keystroke; a slower
                    // earlier one finishing last must not replace the
                    // results for what is in the field now.
                    if model.as_ref().find_needle.to_string() != needle {
                        return;
                    }
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

    fn hash_cancel(mut self: Pin<&mut Self>) {
        self.as_ref()
            .hash_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.as_mut().set_peek_hash(QString::default());
    }

    fn hash_row(mut self: Pin<&mut Self>, row: i32) {
        self.as_mut().hash_cancel();
        let Some(path) = self.as_ref().path_of_row(row) else {
            return;
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.as_mut().rust_mut().hash_cancel = cancel.clone();
        self.as_mut().set_peek_hash(QString::from("…"));
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-hash".into())
            .spawn(move || {
                let r = gust_core::hash::sha256_file(&path, &cancel);
                let text = match r {
                    Ok(Some(h)) => h,
                    Ok(None) => return,
                    Err(e) => format!("no hash: {e}"),
                };
                let _ = thread.queue(move |mut model| {
                    if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        model.as_mut().set_peek_hash(QString::from(&text));
                    }
                });
            })
            .ok();
    }

    fn preview_neighbor(&self, row: i32, delta: i32) -> i32 {
        use gust_core::mime::{category_for_name, needs_trusted_launch, Category};
        let Some(current) = self.entry_at(row) else {
            return row;
        };
        if delta == 0 {
            return row;
        }
        let is_media = |entry: &Entry| {
            matches!(
                category_for_name(&entry.name),
                Category::Image | Category::Audio | Category::Video
            )
        };
        let media_only = is_media(current);
        let mut next = row + delta.signum();
        while let Some(entry) = self.entry_at(next) {
            if entry.kind == EntryKind::File
                && (!media_only || is_media(entry))
                && !needs_trusted_launch(&entry.name, entry.meta.map(|meta| meta.mode))
            {
                return next;
            }
            next += delta.signum();
        }
        row
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
        // Properties: facts about the item, from lstat only.
        let mut facts: Vec<String> = gust_core::properties::describe(&path)
            .into_iter()
            .map(|f| format!("[{},{}]", Self::json_str(f.label), Self::json_str(&f.value)))
            .collect();
        // A folder's size is its contents', once measured in the background.
        if let Some(name) = path.file_name() {
            if let Some(d) = self.dir_sizes.get(name) {
                let n = if d.files == 1 {
                    "1 file".to_string()
                } else {
                    format!("{} files", d.files)
                };
                let v = if d.complete {
                    format!("{} ({n})", human_size(d.bytes))
                } else {
                    format!(
                        "at least {} ({n}; some could not be read)",
                        human_size(d.bytes)
                    )
                };
                facts.insert(0, format!("[\"Contents\",{}]", Self::json_str(&v)));
            }
        }
        QString::from(format!(
            r#"{{"ok":true,"facts":[{}],"name":{},"kind":{},"kindWords":{},"size":{},"sizeText":{},"mode":{},"lines":{},"truncated":{},"noPreview":{}}}"#,
            facts.join(","),
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
        // Back on the row the user left, not the top of the folder.
        if let Some(name) = self.as_mut().rust_mut().find_return.take() {
            self.as_mut().rust_mut().focus_name = Some(name);
        }
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

    /// QML owns whether this view displays thumbnails. A finishing helper
    /// remains counted after cancellation, so scrolling cannot multiply it.
    fn request_thumbnails(mut self: Pin<&mut Self>, enabled: bool) {
        self.as_mut().rust_mut().thumbs_wanted = enabled;
        if enabled {
            self.as_mut().start_thumbnails();
        } else {
            self.as_mut().rust_mut().thumb_cancel.cancel();
            self.as_mut().rust_mut().thumb_work.stop();
        }
    }

    /// Hand one finished thumbnail to the model, on the GUI thread.
    ///
    /// Tagged with the generation it was requested for, so a thumbnail that
    /// finishes after the user has navigated away is dropped rather than
    /// painted over the new folder's tiles.
    /// Measure every sub-folder on screen on ONE worker, smallest work
    /// first by position, publishing each row as it finishes.
    ///
    /// Local folders only: a network mount's size means walking the whole
    /// remote tree, and a stalled one would hold the worker forever. Already
    /// measured folders are kept across live refreshes, so a file appearing
    /// here does not re-walk every sibling.
    fn start_dir_sizes(mut self: Pin<&mut Self>) {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return;
        };
        if self.as_ref().finding || gust_core::stall::needs_probe(&dir) {
            return;
        }
        let want: Vec<std::ffi::OsString> = {
            let r = self.as_ref();
            let (first, count) = r.view_rows;
            let range = gust_core::dirsize::rows_in_view(first, count, r.visible.len());
            r.visible[range]
                .iter()
                .filter_map(|i| r.all.get(*i))
                .filter(|e| e.kind == EntryKind::Dir && !r.dir_sizes.contains_key(&e.name))
                .map(|e| e.name.clone())
                .collect()
        };
        if want.is_empty() {
            return;
        }
        self.as_mut().rust_mut().size_cancel.cancel();
        let cancel = CancelToken::new();
        self.as_mut().rust_mut().size_cancel = cancel.clone();
        let gen = self.as_ref().generation;
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-sizes".into())
            .spawn(move || {
                for name in want {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let s = gust_core::dirsize::dir_size(
                        &dir.join(&name),
                        &cancel,
                        gust_core::dirsize::SizeLimits::default(),
                    );
                    if cancel.is_cancelled() {
                        return;
                    }
                    let _ = thread.queue(move |model| Self::publish_dir_size(model, gen, name, s));
                }
            })
            .ok();
    }

    fn publish_dir_size(
        mut self: Pin<&mut Self>,
        gen: u64,
        name: std::ffi::OsString,
        size: gust_core::dirsize::Sized,
    ) {
        if self.as_ref().generation != gen {
            return;
        }
        self.as_mut()
            .rust_mut()
            .dir_sizes
            .insert(name.clone(), size);
        let Some(row) = self
            .as_ref()
            .visible
            .iter()
            .position(|i| self.as_ref().all.get(*i).map(|e| &e.name) == Some(&name))
        else {
            return;
        };
        let mut roles = cxx_qt_lib::QVector::<i32>::default();
        roles.append(ROLE_SIZE);
        unsafe {
            let idx = self
                .as_ref()
                .index(row as i32, 0, &cxx_qt_lib::QModelIndex::default());
            self.as_mut().data_changed(&idx, &idx, &roles);
        }
    }

    /// Gust's own transfer refresh must not carry an old aggregate past
    /// the new directory metadata. Keep unrelated siblings cached.
    fn invalidate_job_dir_sizes(mut self: Pin<&mut Self>, job: &gust_ops::journal::Job) {
        let Some(here) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return;
        };
        let mut r = self.as_mut().rust_mut();
        if crate::folder_sizes::invalidate_job(&mut r.dir_sizes, &here, job) {
            r.size_cancel.cancel();
        }
    }

    fn publish_thumbnails(
        mut self: Pin<&mut Self>,
        gen: u64,
        batch: Vec<(std::ffi::OsString, String)>,
    ) {
        if self.as_ref().generation != gen {
            return;
        }
        let mut changed = std::collections::HashSet::new();
        for (name, url) in batch {
            let mut r = self.as_mut().rust_mut();
            if url.is_empty() {
                if r.thumbs.remove(&name).is_some() {
                    changed.insert(name);
                }
            } else if r.thumbs.get(&name) != Some(&url) {
                r.thumbs.insert(name.clone(), url);
                changed.insert(name);
            }
        }
        if changed.is_empty() {
            return;
        }
        let rows: Vec<_> = self
            .as_ref()
            .visible
            .iter()
            .enumerate()
            .filter_map(|(row, &i)| changed.contains(&self.as_ref().all[i].name).then_some(row))
            .collect();
        let mut roles = cxx_qt_lib::QVector::<i32>::default();
        roles.append(ROLE_THUMB);
        for row in rows {
            unsafe {
                let idx = self
                    .as_ref()
                    .index(row as i32, 0, &cxx_qt_lib::QModelIndex::default());
                self.as_mut().data_changed(&idx, &idx, &roles);
            }
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
        fill_metadata(&mut snap, &cancel);
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
        // A file deleted by another program must not stay selected and
        // become the silent target of the next command. Nothing selected
        // means nothing to prune: skip hashing every name in the folder.
        if !self.as_ref().selection.is_untouched() {
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
        // The selection may have just shrunk (navigation, a hidden or
        // filtered-out entry, a file that vanished). Republish it, or the
        // status bar keeps announcing a selection that no longer exists.
        self.as_mut().refresh_selection_flags();

        self.as_mut().publish_empty_reason(&snap.complete);
        let status = match &snap.complete {
            gust_core::entry::Completeness::Failed { error } => format!("cannot open: {error}"),
            _ if !settled => format!("{} — reading details…", count_items(n)),
            _ => count_items(n),
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
        // A name waiting for the cursor (rename, new folder, "Show in
        // folder") is honoured as soon as the names are in.
        if let Some(want) = self.as_ref().focus_name.clone() {
            let row = {
                let r = self.as_ref();
                r.visible
                    .iter()
                    .position(|i| r.all.get(*i).is_some_and(|e| e.name == want))
            };
            if let Some(row) = row {
                self.as_mut().rust_mut().focus_name = None;
                self.as_mut().set_cursor(row as i32);
            } else if settled {
                self.as_mut().rust_mut().focus_name = None;
            }
        }
        self.as_mut().set_status(QString::from(&status));

        if settled && snap.settled {
            self.as_mut().start_thumbnails();
            self.as_mut().start_cloud_notice();
            self.as_mut().ensure_watch();
            self.as_mut().start_dir_sizes();
        }
    }

    /// Publish why a listing has no rows, from how it ended.
    fn publish_empty_reason(mut self: Pin<&mut Self>, complete: &gust_core::entry::Completeness) {
        let reason = match complete {
            gust_core::entry::Completeness::Failed { error } => empty_reason_for(error),
            _ => String::new(),
        };
        if self.as_ref().empty_reason.to_string() != reason {
            self.as_mut().set_empty_reason(QString::from(&reason));
        }
    }

    /// Watch the folder on screen, if it is local and not already watched.
    ///
    /// Started after the listing settles so the first paint never waits on
    /// it. A network or FUSE mount is not watched: inotify cannot see what
    /// changed on the server, and touching a stalled mount can hang.
    fn ensure_watch(mut self: Pin<&mut Self>) {
        let dir = match self.as_ref().key.as_path() {
            Some(p) if !self.as_ref().finding => p.to_path_buf(),
            _ => {
                self.as_mut().stop_watch();
                return;
            }
        };
        if self.as_ref().watch.is_some() && self.as_ref().watch_key.as_deref() == Some(&dir) {
            return;
        }
        self.as_mut().stop_watch();
        if gust_core::stall::needs_probe(&dir) {
            return;
        }
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let wgen = self.as_ref().watch_gen + 1;
        let started = gust_core::watch::DirWatch::start(&dir, move |change| {
            let _ = thread.queue(move |model| Self::live_change(model, wgen, change));
        });
        let mut r = self.as_mut().rust_mut();
        r.watch_gen = wgen;
        if let Ok(w) = started {
            r.watch = Some(w);
            r.watch_key = Some(dir);
        }
    }

    fn stop_watch(mut self: Pin<&mut Self>) {
        let mut r = self.as_mut().rust_mut();
        // Dropping joins the watch thread; it wakes on its stop fd at once.
        r.watch = None;
        r.watch_key = None;
        r.watch_gen += 1;
        r.live_dirty = false;
    }

    /// A change reported by the watch. GUI thread.
    fn live_change(mut self: Pin<&mut Self>, wgen: u64, change: gust_core::watch::Change) {
        if wgen != self.as_ref().watch_gen {
            return;
        }
        match change {
            gust_core::watch::Change::Gone => {
                // Stay put: jumping somewhere else would lose the user's
                // place (tabs, history). The relist fails, which empties the
                // list — nothing that no longer exists can be acted on — and
                // the list says why instead of "This folder is empty".
                self.as_mut().stop_watch();
                self.as_mut().start_live_relist();
            }
            gust_core::watch::Change::Contents => {
                if self.as_ref().live_hold || self.as_ref().live_busy {
                    self.as_mut().rust_mut().live_dirty = true;
                } else {
                    self.as_mut().start_live_relist();
                }
            }
        }
    }

    /// Relist the current folder on a worker and merge it in place.
    fn start_live_relist(mut self: Pin<&mut Self>) {
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let key = self.as_ref().key.clone();
        let gen = self.as_ref().generation;
        let sort = self.as_ref().sort_spec();
        let cancel = self.as_ref().load_cancel.clone();
        let filter_now = self.as_ref().filter.to_string();
        let hidden_now = self.as_ref().show_hidden;
        let (view_first, view_count) = self.as_ref().view_rows;
        let mut r = self.as_mut().rust_mut();
        r.live_busy = true;
        r.live_dirty = false;
        std::thread::Builder::new()
            .name("gust-live".into())
            .spawn(move || {
                let mut snap = list_names(&key, gen, &cancel, |_| {});
                if snap.sort != sort {
                    snap.sort = sort;
                    snap.sort_entries();
                }
                if !cancel.is_cancelled() {
                    if snap.sort_needs_metadata() {
                        fill_metadata(&mut snap, &cancel);
                    } else {
                        // The rows on screen, the first screenful, and every
                        // folder: their dates decide which cached folder
                        // sizes are stale.
                        let shown = visible_indices(&snap.entries, &filter_now, hidden_now);
                        let count = (view_count.max(0) as usize).max(FIRST_WINDOW / 2);
                        let centre = view_first.max(0) as usize + count / 2;
                        let mut window = first_window(&shown, Some(centre), count * 2);
                        window.extend(
                            snap.entries
                                .iter()
                                .enumerate()
                                .filter(|(_, e)| e.kind == EntryKind::Dir)
                                .map(|(i, _)| i)
                                .take(2000),
                        );
                        fill_metadata_for(&mut snap, &window, &cancel);
                    }
                }
                let _ = thread.queue(move |model| Self::apply_live(model, snap, cancel));
            })
            .ok();
    }

    /// Merge a relisted folder into the rows on screen WITHOUT a reset:
    /// rows are removed and inserted individually, so the view keeps its
    /// scroll position, the cursor stays on the same file, and a file
    /// another program is writing appears in place.
    fn apply_live(
        mut self: Pin<&mut Self>,
        snap: gust_core::entry::DirectorySnapshot,
        cancel: CancelToken,
    ) {
        self.as_mut().rust_mut().live_busy = false;
        // Navigated away, or a newer listing superseded this one.
        if cancel.is_cancelled() || snap.generation != self.as_ref().generation {
            return;
        }
        // The watch observed a filesystem change. Revalidate cached images
        // on the worker, including replacements that kept the same name.
        self.as_mut().rust_mut().thumb_cancel.cancel();
        self.as_mut().rust_mut().thumb_work.invalidate();

        let snap_complete = snap.complete.clone();
        // A sub-folder whose own timestamp moved had something added,
        // removed or renamed inside it: its cached size is stale. (A file
        // rewritten deeper down does not move it; Ctrl+R re-measures all.)
        {
            let old_mtimes: std::collections::HashMap<std::ffi::OsString, i64> = self
                .as_ref()
                .all
                .iter()
                .filter(|e| e.kind == EntryKind::Dir)
                .filter_map(|e| e.meta.map(|m| (e.name.clone(), m.mtime)))
                .collect();
            let stale: Vec<std::ffi::OsString> = snap
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Dir)
                .filter(|e| {
                    let now = e.meta.map(|m| m.mtime);
                    old_mtimes.get(&e.name).copied() != now
                })
                .map(|e| e.name.clone())
                .collect();
            let mut r = self.as_mut().rust_mut();
            for n in stale {
                r.dir_sizes.remove(&n);
            }
        }
        // The folder could not be read any more. DELETE_SELF is not a
        // reliable signal: while any process still has its cwd inside the
        // folder the kernel delays it, so the failed relist is what ends
        // the watch — a watch on a dead inode would never fire again.
        if matches!(snap_complete, gust_core::entry::Completeness::Failed { .. }) {
            self.as_mut().stop_watch();
        }
        // The model may have been reset meanwhile (a filter, a sort): only
        // merge onto what is on screen, never onto a stale idea of it.
        let cursor_name = self
            .as_ref()
            .entry_at(self.as_ref().cursor)
            .map(|e| e.name.clone());
        let old_names: Vec<std::ffi::OsString> = {
            let r = self.as_ref();
            r.visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .map(|e| e.name.clone())
                .collect()
        };

        // The new visible list, by the same rule as recompute_visible.
        let new_visible = visible_indices(
            &snap.entries,
            &self.as_ref().filter.to_string(),
            self.as_ref().show_hidden,
        );
        let new_visible_names: Vec<std::ffi::OsString> = new_visible
            .iter()
            .map(|&i| snap.entries[i].name.clone())
            .collect();

        let edits = gust_core::watch::row_edits(&old_names, &new_visible_names);
        let root = cxx_qt_lib::QModelIndex::default();

        // Removals, highest row first. Contiguous rows go in one signal: an
        // archive unpacked into the folder is one insert, not 50 000.
        let removes: Vec<usize> = edits
            .iter()
            .filter_map(|e| match *e {
                gust_core::watch::RowEdit::Remove(i) => Some(i),
                _ => None,
            })
            .collect();
        let mut k = 0;
        while k < removes.len() {
            // `removes` descends; extend while the next is one row above.
            let hi = removes[k];
            let mut lo = hi;
            while k + 1 < removes.len() && removes[k + 1] + 1 == lo {
                k += 1;
                lo = removes[k];
            }
            k += 1;
            unsafe { self.as_mut().begin_remove_rows(&root, lo as i32, hi as i32) };
            self.as_mut().rust_mut().visible.drain(lo..=hi);
            unsafe { self.as_mut().end_remove_rows() };
        }

        // Swap in the new entries. `visible` now holds only surviving rows,
        // still pointing into the OLD `all`; re-point them by name into the
        // new list, then insert the new rows.
        let survivors: Vec<std::ffi::OsString> = {
            let r = self.as_ref();
            r.visible
                .iter()
                .filter_map(|i| r.all.get(*i))
                .map(|e| e.name.clone())
                .collect()
        };
        let index_of: std::collections::HashMap<std::ffi::OsString, usize> = snap
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        {
            let mut r = self.as_mut().rust_mut();
            crate::thumbnails::retain_eligible(&mut r.thumbs, &snap.entries);
            r.all = snap.entries;
            r.visible = survivors
                .iter()
                .filter_map(|n| index_of.get(n).copied())
                .collect();
        }
        // Inserts, lowest row first; `at` is also the row in the new list,
        // so a run of consecutive `at` values is one contiguous block.
        let inserts: Vec<usize> = edits
            .iter()
            .filter_map(|e| match *e {
                gust_core::watch::RowEdit::Insert { at, .. } => Some(at),
                _ => None,
            })
            .collect();
        let mut k = 0;
        while k < inserts.len() {
            let first = inserts[k];
            let mut last = first;
            while k + 1 < inserts.len() && inserts[k + 1] == last + 1 {
                k += 1;
                last = inserts[k];
            }
            k += 1;
            unsafe {
                self.as_mut()
                    .begin_insert_rows(&root, first as i32, last as i32)
            };
            {
                let mut r = self.as_mut().rust_mut();
                let block: Vec<usize> = new_visible[first..=last].to_vec();
                r.visible.splice(first..first, block);
            }
            unsafe { self.as_mut().end_insert_rows() };
        }

        // By construction the edits produced exactly the new visible list.
        debug_assert_eq!(self.as_ref().visible, new_visible);
        self.as_mut().rust_mut().visible = new_visible;

        // Rows that stayed may still have changed (size while a download
        // finished, mtime, a thumbnail): repaint them all, cheaply.
        let n = self.as_ref().visible.len() as i32;
        if n > 0 {
            unsafe {
                let top = self.as_ref().index(0, 0, &root);
                let bottom = self.as_ref().index(n - 1, 0, &root);
                let roles = cxx_qt_lib::QVector::<i32>::default();
                self.as_mut().data_changed(&top, &bottom, &roles);
            }
        }

        // Selection: files that vanished leave it. The cursor follows its
        // file; if that file is gone it stays at the same height.
        {
            let mut r = self.as_mut().rust_mut();
            r.selection.retain_names(&new_visible_names);
        }
        let wanted = self.as_mut().rust_mut().focus_name.take();
        let new_cursor = wanted
            .and_then(|w| new_visible_names.iter().position(|n| *n == w))
            .or_else(|| cursor_name.and_then(|c| new_visible_names.iter().position(|n| *n == c)))
            .map(|p| p as i32)
            .unwrap_or_else(|| self.as_ref().cursor.min((n - 1).max(0)));
        if new_cursor != self.as_ref().cursor {
            self.as_mut().set_cursor(new_cursor);
        }
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_count(n);
        self.as_mut().publish_empty_reason(&snap_complete);
        let status = match &snap_complete {
            gust_core::entry::Completeness::Failed { error } => format!("cannot open: {error}"),
            _ => count_items(n),
        };
        self.as_mut().set_status(QString::from(&status));
        self.as_mut().start_thumbnails();
        self.as_mut().start_dir_sizes();

        // Changes that arrived while this relist ran.
        if self.as_ref().live_dirty && !self.as_ref().live_hold {
            self.as_mut().start_live_relist();
        }
    }

    /// After Gust itself changed this folder: merge the change in place and
    /// put the cursor on `name`. A full reload here reset the view, so the
    /// list jumped and the cursor stayed at the old row NUMBER — on
    /// whichever file had moved into it.
    fn relist_focusing(mut self: Pin<&mut Self>, name: std::ffi::OsString) {
        self.as_mut().rust_mut().focus_name = Some(name);
        if self.as_ref().live_busy {
            self.as_mut().rust_mut().live_dirty = true;
        } else {
            self.as_mut().start_live_relist();
        }
    }

    fn want_rows(mut self: Pin<&mut Self>, first: i32, count: i32) {
        self.as_mut().rust_mut().view_rows = (first, count);
        // Rows that scrolled into view get their folder sizes now.
        if self.as_ref().settled {
            self.as_mut().start_dir_sizes();
            self.as_mut().start_thumbnails();
        }
        let n = self.as_ref().visible.len();
        if n == 0 || count <= 0 {
            return;
        }
        let first = (first.max(0) as usize).min(n - 1);
        let count = count as usize;
        // Half a screen of margin each way, so a normal scroll finds the
        // next rows already read.
        let lo = first.saturating_sub(count / 2);
        let hi = (first + count + count / 2).min(n);
        let missing: Vec<(usize, std::ffi::OsString, EntryKind)> = {
            let r = self.as_ref();
            // The cursor's row too: Enter, Properties and the status line
            // act on it even when the view has not caught up yet.
            let cursor_row = usize::try_from(r.cursor).ok().filter(|c| *c < n);
            r.visible[lo..hi]
                .iter()
                .chain(cursor_row.and_then(|c| r.visible.get(c)))
                .filter_map(|&i| {
                    r.all
                        .get(i)
                        .filter(|e| e.meta.is_none())
                        .map(|e| (i, e.name.clone(), e.kind))
                })
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        if self.as_ref().meta_busy {
            // One reader at a time; the newest request wins.
            self.as_mut().rust_mut().meta_again = Some((first as i32, count as i32));
            return;
        }
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return;
        };
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        let gen = self.as_ref().generation;
        let cancel = self.as_ref().load_cancel.clone();
        self.as_mut().rust_mut().meta_busy = true;
        std::thread::Builder::new()
            .name("gust-meta".into())
            .spawn(move || {
                let mut got = Vec::with_capacity(missing.len());
                for (i, name, kind) in missing {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let meta = stat_entry(&dir, &name, kind);
                    got.push((i, name, meta));
                }
                let _ = thread.queue(move |model| Self::apply_row_meta(model, gen, lo, hi, got));
            })
            .ok();
    }

    /// Install details read for rows that scrolled into view. GUI thread.
    fn apply_row_meta(
        mut self: Pin<&mut Self>,
        gen: u64,
        lo: usize,
        hi: usize,
        got: Vec<(
            usize,
            std::ffi::OsString,
            Option<gust_core::entry::Metadata>,
        )>,
    ) {
        self.as_mut().rust_mut().meta_busy = false;
        if gen != self.as_ref().generation {
            // The folder changed; that request described rows that are gone.
            self.as_mut().rust_mut().meta_again = None;
            return;
        }
        {
            let mut r = self.as_mut().rust_mut();
            for (i, name, meta) in got {
                if let Some(e) = r.all.get_mut(i) {
                    // Identity by name: the listing may have been merged in
                    // place meanwhile, and an index alone could now name a
                    // different file.
                    if e.name == name && e.meta.is_none() {
                        e.meta = meta;
                    }
                }
            }
        }
        let n = self.as_ref().visible.len();
        if n > 0 && lo < n {
            let hi = hi.min(n) - 1;
            let root = cxx_qt_lib::QModelIndex::default();
            let mut roles = cxx_qt_lib::QVector::<i32>::default();
            roles.append(ROLE_SIZE);
            roles.append(ROLE_MODIFIED);
            roles.append(ROLE_LINK_OK);
            // Executable, empty, link-to-folder, read-only and mount all
            // come from the details just read.
            roles.append(ROLE_ICON_KEY);
            roles.append(ROLE_EMBLEMS);
            // Safety: GUI thread, inside a queued model callback.
            unsafe {
                let top = self.as_ref().index(lo as i32, 0, &root);
                let bottom = self.as_ref().index(hi as i32, 0, &root);
                self.as_mut().data_changed(&top, &bottom, &roles);
            }
        }
        let rev = self.as_ref().meta_rev.wrapping_add(1);
        self.as_mut().set_meta_rev(rev);
        if let Some((f, c)) = self.as_mut().rust_mut().meta_again.take() {
            self.as_mut().want_rows(f, c);
        }
    }

    fn hold_live_updates(mut self: Pin<&mut Self>, hold: bool) {
        self.as_mut().rust_mut().live_hold = hold;
        if !hold && self.as_ref().live_dirty && !self.as_ref().live_busy {
            self.as_mut().start_live_relist();
        }
    }

    fn is_live(&self) -> bool {
        self.watch.is_some()
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
        // A typed "~/…" (palette, Ctrl+L) means the home folder, as in every
        // shell; it used to be taken literally and refused.
        let typed = path.to_string();
        let p = match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            Some(h) if typed == "~" || typed.starts_with("~/") => {
                gust_core::jump::expand(&typed, std::path::Path::new(&h))
            }
            _ => PathBuf::from(typed),
        };
        // "/x/" and "/x" are one folder: rebuilding from components drops a
        // trailing slash (and doubled ones), so history and breadcrumbs see
        // one location, not two. "/" stays "/".
        let p: PathBuf = p.components().collect();
        // The folder already on screen: nothing to do (Flea 0.3.7). Not on
        // the first listing, and not over search results, which are left by
        // navigating to the folder they were found in.
        if self.as_ref().generation > 0
            && !self.as_ref().finding
            && self.as_ref().empty_reason.is_empty()
            && gust_core::location::same_place(&self.as_ref().key, &p)
        {
            return;
        }
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
        self.as_mut().rust_mut().size_cancel.cancel();
        self.as_mut().rust_mut().dir_sizes.clear();
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
        self.as_mut().rust_mut().size_cancel.cancel();
        self.as_mut().rust_mut().dir_sizes.clear();
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
        self.load_current_cached(false);
    }

    /// Ctrl+R: the user asked for fresh facts, so folders are re-measured
    /// too. (Internal reloads after an operation keep the sizes of folders
    /// the operation did not touch; the watch invalidates the ones it did.)
    fn reload_all(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().size_cancel.cancel();
        self.as_mut().rust_mut().dir_sizes.clear();
        self.load_current_cached(false);
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
        // The selection may have just shrunk (navigation, a hidden or
        // filtered-out entry, a file that vanished). Republish it, or the
        // status bar keeps announcing a selection that no longer exists.
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_count(n);
        self.as_mut().set_status(QString::from(&count_items(n)));
        self.as_mut().start_thumbnails();
    }

    fn set_sort(self: Pin<&mut Self>, field: &QString) {
        // Clicking the active column flips direction.
        let asc = if self.as_ref().sort_field.to_string() == field.to_string() {
            !self.as_ref().sort_ascending
        } else {
            true
        };
        self.set_sort_to(field, asc);
    }

    fn set_sort_to(mut self: Pin<&mut Self>, field: &QString, ascending: bool) {
        let f = field.to_string();
        if !matches!(f.as_str(), "name" | "kind" | "size" | "modified") {
            return;
        }
        if self.as_ref().sort_field.to_string() != f {
            self.as_mut().set_sort_field(QString::from(&f));
        }
        if self.as_ref().sort_ascending != ascending {
            self.as_mut().set_sort_ascending(ascending);
        }
        let spec = self.as_ref().sort_spec();
        // Remembered for THIS folder only (per-folder sort, D31).
        {
            let key = self.as_ref().key.clone();
            self.as_mut()
                .rust_mut()
                .sorts
                .remember(&key, spec.field, spec.ascending);
            if let Some(path) = gust_core::sorts::SortMemory::default_path() {
                let _ = self.as_ref().sorts.save(&path);
            }
        }
        // Sorting by size or date on rows whose details were never read
        // would order them as zero. Read everything first.
        let needs_meta = matches!(spec.field, SortField::Size | SortField::Modified);
        if needs_meta && self.as_ref().all.iter().any(|e| e.meta.is_none()) {
            self.as_mut().load_current();
            return;
        }
        unsafe { self.as_mut().begin_reset_model() };
        {
            let mut r = self.as_mut().rust_mut();
            gust_core::entry::sort_entries(&mut r.all, spec);
        }
        self.as_mut().rust_mut().recompute_visible();
        unsafe { self.as_mut().end_reset_model() };
        self.as_mut().start_thumbnails();
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
        // The selection may have just shrunk (navigation, a hidden or
        // filtered-out entry, a file that vanished). Republish it, or the
        // status bar keeps announcing a selection that no longer exists.
        self.as_mut().refresh_selection_flags();
        self.as_mut().set_count(n);
        self.as_mut().set_cursor(0);
        let t = text.to_string();
        let status = if t.is_empty() {
            count_items(n)
        } else {
            format!("{n} of {} match \"{}\"", self.as_ref().all.len(), t)
        };
        self.as_mut().set_status(QString::from(&status));
        self.as_mut().start_thumbnails();
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
        if admin_session() {
            return QString::from(&format!("blocked:{ADMIN_REFUSAL}"));
        }
        // A row whose details were not read yet has an unknown mode, and an
        // unknown mode could hide an executable bit. Never guess: read it.
        if kind == EntryKind::File && mode.is_none() {
            self.as_mut().want_rows(row, 1);
            return QString::from("blocked:reading this file's details — press Enter again");
        }
        // Executables never launch from a plain activation.
        if gust_core::mime::needs_trusted_launch(&name, mode) {
            return QString::from(
                "blocked:this is a program — use Run… (right-click or the : palette) to start it",
            );
        }

        let Some(path) = target.as_path() else {
            return QString::from("blocked:this location is not a local file");
        };
        // One media window needs the neighbours, in this pane's displayed
        // order. Build from cached rows; process lookup and playlist I/O
        // happen on a worker so opening media cannot stall the UI.
        if let Some(playlist) = self.as_ref().key.as_path().and_then(|directory| {
            gust_core::open::media::Playlist::from_entries(
                directory,
                self.as_ref().visible_entries(),
                &name,
            )
        }) {
            let Some(thread) = self.as_ref().qt_thread_handle() else {
                return QString::from("blocked:the media viewer is not ready");
            };
            let path = path.to_path_buf();
            let launch = std::thread::Builder::new()
                .name("gust-media".into())
                .spawn(move || {
                    let result = if let Some(player) = gust_core::open::which("mpv") {
                        playlist
                            .launch(&player)
                            .and_then(|mut child| {
                                let status = child.wait()?;
                                if status.success() {
                                    Ok(())
                                } else {
                                    Err(std::io::Error::other(format!(
                                        "the media viewer exited with {status}"
                                    )))
                                }
                            })
                            .map_err(|error| error.to_string())
                    } else {
                        gust_core::open::open_with_desktop(&path, mode)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    };
                    if let Err(error) = result {
                        let _ = thread.queue(move |mut model| {
                            model.as_mut().set_status(QString::from(&format!(
                                "Could not open media: {error}"
                            )));
                        });
                    }
                });
            return match launch {
                Ok(_) => QString::from("file"),
                Err(error) => QString::from(&format!("blocked:could not open media: {error}")),
            };
        }
        // Non-media keeps its desktop handler; explicit Open with… also
        // remains available when a different media application is wanted.
        match gust_core::open::open_with_desktop(path, mode) {
            Ok(_opener) => QString::from("file"),
            Err(e) => QString::from(&format!("blocked:{e}")),
        }
    }

    fn install_support(self: Pin<&mut Self>, key: QString) -> QString {
        if self.as_ref().place_busy {
            return QString::from("Another connection or setup is still in progress.");
        }
        let key = key.to_string();
        self.place_op(
            "setup-install",
            key.clone(),
            "Installing connection support…".into(),
            move || match gust_remote::setup::install(&key) {
                Ok(()) => PlaceOutcome::Done("Connection support is ready".into()),
                Err(message) => PlaceOutcome::Failed(message),
            },
        );
        QString::default()
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
        if admin_session() {
            return QString::from(ADMIN_REFUSAL);
        }
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
        // Each crumb is "path\tlabel". Inside a mounted place the bar starts
        // at the place ("nas / Shared"), not at "/ run / user / 1000 / …".
        let (start, first_label) = match place_of(path) {
            Some((root, label)) => (root, label),
            None => (PathBuf::from("/"), "/".to_string()),
        };
        let rest = path.strip_prefix(&start).unwrap_or(path);
        let mut acc = start.clone();
        list.append(QString::from(&format!(
            "{}\t{first_label}",
            start.to_string_lossy()
        )));
        for c in rest.components() {
            acc.push(c);
            let label = gust_core::location::escape_for_display(c.as_os_str());
            list.append(QString::from(&format!(
                "{}\t{label}",
                acc.to_string_lossy()
            )));
        }
        list
    }

    fn entry_icon_key(&self, row: i32) -> QString {
        let k = self.entry_at(row).map(|e| self.icon_key_of(e));
        QString::from(&k.unwrap_or_default())
    }

    fn entry_tags(&self, row: i32) -> QString {
        let v = self
            .entry_at(row)
            .map(|e| gust_core::tags::keys_of(e.meta.map(|m| m.tags).unwrap_or(0)).join(","));
        QString::from(&v.unwrap_or_default())
    }

    fn entry_emblems(&self, row: i32) -> QString {
        let v = self
            .entry_at(row)
            .map(|e| gust_core::icons::emblems_for(e.kind, e.meta.as_ref()).join(","));
        QString::from(&v.unwrap_or_default())
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
                "\"status\":\"{}\",\"meta_known\":{},\"cursor_meta\":{},\"marked\":{},\"sized\":{},\"generation\":{},\"visible\":[{}]}}"
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
            self.all.iter().filter(|e| e.meta.is_some()).count(),
            self.entry_at(self.cursor).is_some_and(|e| e.meta.is_some()),
            self.key
                .as_path()
                .map(|d| self.all.iter().filter(|e| self.clip_set.contains(&d.join(&e.name))).count())
                .unwrap_or(0),
            self.dir_sizes.len(),
            self.generation,
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
            r.last_link = None;
            r.last_modes.clear();
            r.last_extra = UndoExtra::None;
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
        let extra = std::mem::take(&mut self.as_mut().rust_mut().last_extra);
        match extra {
            UndoExtra::None => {}
            UndoExtra::Renamed(made) => {
                if let Err(why) = gust_ops::attrs::undo_rename(&made) {
                    self.as_mut().rust_mut().last_extra = UndoExtra::Renamed(made);
                    return QString::from(&why);
                }
                if self.as_ref().key.as_path() == made.original.parent() {
                    if let Some(name) = made.original.file_name() {
                        self.as_mut().relist_focusing(name.to_os_string());
                    }
                } else {
                    self.as_mut().reload();
                }
                self.as_mut()
                    .set_status(QString::from("Restored the original name"));
                return QString::default();
            }
            UndoExtra::Tags(ch) => {
                let (back, refused) = gust_ops::tags::undo_tags(&ch);
                self.as_mut().reload();
                if refused.is_empty() {
                    self.as_mut().set_status(QString::from(&format!(
                        "Tags put back on {back} item{}",
                        if back == 1 { "" } else { "s" }
                    )));
                    return QString::default();
                }
                return QString::from(&format!(
                    "Put back {back}; {} left as they are: {}",
                    refused.len(),
                    refused.join("; ")
                ));
            }
            UndoExtra::Links(made) => {
                let mut refused = Vec::new();
                for m in &made {
                    if let Err(e) = gust_ops::attrs::undo_link(m) {
                        refused.push(e);
                    }
                }
                self.as_mut().reload();
                if refused.is_empty() {
                    self.as_mut().set_status(QString::from(&format!(
                        "Removed {} link{}",
                        made.len(),
                        if made.len() == 1 { "" } else { "s" }
                    )));
                    return QString::default();
                }
                return QString::from(&refused.join("; "));
            }
            UndoExtra::Owners(ch) => {
                let (back, refused) = gust_ops::attrs::undo_owners(&ch);
                self.as_mut().reload();
                let msg = if refused.is_empty() {
                    format!(
                        "Owner put back on {back} item{}",
                        if back == 1 { "" } else { "s" }
                    )
                } else {
                    format!(
                        "Put back {back}; {} left as they are: {}",
                        refused.len(),
                        refused.join("; ")
                    )
                };
                self.as_mut().set_status(QString::from(&msg));
                return QString::default();
            }
            UndoExtra::Created(c) => {
                let r = gust_ops::create::undo_create(&c);
                self.as_mut().reload();
                return match r {
                    Ok(()) => {
                        self.as_mut()
                            .set_status(QString::from("Removed the pasted image"));
                        QString::default()
                    }
                    Err(e) => QString::from(&e),
                };
            }
        }
        if let Some(made) = self.as_ref().last_link.clone() {
            self.as_mut().rust_mut().last_link = None;
            let r = gust_ops::attrs::undo_link(&made);
            self.as_mut().reload();
            return match r {
                Ok(()) => {
                    self.as_mut()
                        .set_status(QString::from("Removed the link it made"));
                    QString::default()
                }
                Err(e) => QString::from(&e),
            };
        }
        let modes = std::mem::take(&mut self.as_mut().rust_mut().last_modes);
        if !modes.is_empty() {
            let (back, refused) = gust_ops::attrs::undo_modes(&modes);
            self.as_mut().reload();
            let msg = if refused.is_empty() {
                format!(
                    "Permissions put back on {back} item{}",
                    if back == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "Put back {back}; {} left as they are: {}",
                    refused.len(),
                    refused.join("; ")
                )
            };
            self.as_mut().set_status(QString::from(&msg));
            return QString::default();
        }
        // A bulk rename is one user action, so one Ctrl+Z puts every name
        // back — not forty separate undos (DESIGN §6.16).
        let renames = self.as_ref().last_bulk_rename.clone();
        if !renames.is_empty() {
            let mut back = 0usize;
            let mut refusals: Vec<String> = Vec::new();
            // Reverse order, and through the same two-phase trick, so a
            // swap undoes as cleanly as it was done.
            let dir = self
                .as_ref()
                .key
                .as_path()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let plan = gust_ops::bulk::RenamePlan {
                renames: renames
                    .iter()
                    .rev()
                    .map(|(from, to)| gust_ops::bulk::PlannedRename {
                        from: to.clone(),
                        to: from.clone(),
                        old_name: to
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        new_name: from
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        unchanged: false,
                    })
                    .collect(),
                problems: Vec::new(),
                ..Default::default()
            };
            let outcome = gust_ops::bulk::execute(&dir, &plan);
            back += outcome.done.len();
            refusals.extend(outcome.failed);

            self.as_mut().rust_mut().last_bulk_rename.clear();
            self.as_mut().reload();
            let msg = if refusals.is_empty() {
                format!("Put {back} name{} back", if back == 1 { "" } else { "s" })
            } else {
                format!("Put {back} back; {} could not be undone", refusals.len())
            };
            self.as_mut().set_status(QString::from(&msg));
            return QString::default();
        }

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
        self.as_mut().invalidate_job_dir_sizes(job);
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
        match &self.last_extra {
            UndoExtra::None => {}
            UndoExtra::Links(_) => return QString::from("Undo: remove the links"),
            UndoExtra::Owners(_) => return QString::from("Undo: put owners back"),
            UndoExtra::Created(_) => return QString::from("Undo: remove the pasted image"),
            UndoExtra::Renamed(_) => return QString::from("Undo: restore the original name"),
            UndoExtra::Tags(_) => return QString::from("Undo: put the tags back"),
        }
        if self.last_link.is_some() {
            return QString::from("Undo: remove the link");
        }
        if !self.last_modes.is_empty() {
            return QString::from("Undo: put permissions back");
        }
        let n = self.last_bulk_rename.len();
        if n > 0 {
            return QString::from(&format!(
                "Undo: put {n} name{} back",
                if n == 1 { "" } else { "s" }
            ));
        }
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
        // Also offer it to the rest of the desktop, so a copy here can be
        // pasted in another file manager (DESIGN §6.19). A failure here
        // does not lose the copy: the internal clipboard still works.
        let shared = gust_core::clipboard::offer_with(
            gust_core::clipboard::ClipVerb::Copy,
            &paths,
            &clip_holder(),
        )
        .is_ok();
        if !shared && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            self.as_mut().clear_clip();
            return QString::from("Could not copy files to the desktop clipboard. Try Copy again.");
        }
        self.as_mut().set_clip(paths, ClipMode::Copy);
        if shared {
            QString::from(&format!("Copied {label} — paste with Ctrl+V"))
        } else {
            QString::from(&format!(
                "Copied {label} — paste with Ctrl+V (only inside Gust)"
            ))
        }
    }

    fn can_paste(&self) -> bool {
        self.clipboard.is_some()
            || gust_core::clipboard::offers_files()
            || gust_core::clipboard::offers_image()
    }

    fn copy_path(mut self: Pin<&mut Self>, row: i32) -> QString {
        let paths = if row < 0 {
            self.key
                .as_path()
                .map(|p| vec![p.to_path_buf()])
                .unwrap_or_default()
        } else {
            self.as_mut().set_cursor_if_no_selection(row);
            self.as_ref().target_paths()
        };
        if paths.is_empty() {
            return QString::from("Nothing to copy the path of");
        }
        let text = gust_core::clipboard::paths_as_text(&paths);
        // Text replaces a file offer too, including reads in other tabs.
        self.as_mut().clear_clip();
        match gust_core::clipboard::offer_text(&text) {
            Ok(()) if paths.len() == 1 => QString::from(&format!("Copied path {text}")),
            Ok(()) => QString::from(&format!("Copied {} paths", paths.len())),
            Err(e) => QString::from(&format!("Could not copy the path: {e}")),
        }
    }

    fn clip_cut(mut self: Pin<&mut Self>, row: i32) -> QString {
        self.as_mut().set_cursor_if_no_selection(row);
        let paths = self.as_ref().target_paths();
        if paths.is_empty() {
            return QString::from("nothing to cut");
        }
        let label = clip_label(&paths);
        let shared = gust_core::clipboard::offer_with(
            gust_core::clipboard::ClipVerb::Cut,
            &paths,
            &clip_holder(),
        )
        .is_ok();
        if !shared && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            self.as_mut().clear_clip();
            return QString::from("Could not cut files to the desktop clipboard. Try Cut again.");
        }
        self.as_mut().set_clip(paths, ClipMode::Cut);
        if shared {
            QString::from(&format!("Cut {label} — paste with Ctrl+V"))
        } else {
            QString::from(&format!(
                "Cut {label} — paste with Ctrl+V (only inside Gust)"
            ))
        }
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
        self.as_mut().clear_clip();
    }

    fn clip_paste(self: Pin<&mut Self>) -> QString {
        self.paste_into(None)
    }

    fn clip_paste_into(self: Pin<&mut Self>, row: i32) -> QString {
        let dir = match self.entry_at(row) {
            Some(e) if e.kind == EntryKind::Dir => {
                self.key.join(&e.name).as_path().map(|p| p.to_path_buf())
            }
            _ => None,
        };
        match dir {
            Some(d) => self.paste_into(Some(d)),
            None => QString::from("that is not a folder"),
        }
    }

    /// Paste into `into`, or into the folder on screen when `None`.
    fn paste_into(mut self: Pin<&mut Self>, into: Option<PathBuf>) -> QString {
        if self.as_ref().job.is_some() || self.as_ref().clipboard_read.busy() {
            return QString::from("another operation is still running");
        }
        let allow_image = into.is_none();
        let Some(dest) = into.or_else(|| self.as_ref().key.as_path().map(|p| p.to_path_buf()))
        else {
            return QString::from("cannot paste here");
        };
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return QString::from("cannot paste here");
        };
        let origin = self.as_ref().key.clone();
        let own = self.as_ref().clipboard.clone();
        let has_desktop = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let ticket = self.as_mut().rust_mut().clipboard_read.start();
        // Qt can own and serve a newer text copy. Waiting for wl-paste on
        // Qt's own thread prevents that response and deadlocks this window.
        let spawned = std::thread::Builder::new()
            .name("gust-paste-files".into())
            .spawn(move || {
                let system = if has_desktop {
                    gust_core::clipboard::read()
                } else {
                    None
                }
                .map(|c| {
                    let mode = match c.verb {
                        gust_core::clipboard::ClipVerb::Cut => ClipMode::Cut,
                        gust_core::clipboard::ClipVerb::Copy => ClipMode::Copy,
                    };
                    (c.paths, mode)
                });
                let chosen = crate::clipboard::choose(own, system, has_desktop);
                let _ = thread.queue(move |mut model| {
                    if !model.as_mut().rust_mut().clipboard_read.finish(ticket) {
                        return;
                    }
                    if model.as_ref().key != origin || model.as_ref().job.is_some() {
                        model.paste_ready(
                            QString::from("Paste cancelled because the destination changed."),
                            false,
                        );
                        return;
                    }
                    if model.as_ref().clipboard != chosen {
                        // Observing the desktop is not a new clipboard write.
                        model.as_mut().forget_clip();
                    }
                    let result = model.as_mut().apply_clip_paste(chosen, dest);
                    model.paste_ready(result, allow_image);
                });
            });
        if spawned.is_err() {
            self.as_mut().rust_mut().clipboard_read.cancel();
            return QString::from("Could not read the clipboard. Try Paste again.");
        }
        QString::from("?clipboard")
    }

    fn clipboard_busy(&self) -> bool {
        self.clipboard_read.busy()
    }

    fn cancel_clip_paste(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().clipboard_read.cancel();
    }

    fn apply_clip_paste(
        mut self: Pin<&mut Self>,
        chosen: Option<(Vec<PathBuf>, ClipMode)>,
        dest: PathBuf,
    ) -> QString {
        let Some((srcs, mode)) = chosen else {
            return QString::from("nothing to paste");
        };
        // Drop anything that vanished since it was copied.
        // (A file on a network place is not stat'ed here, on the UI thread:
        // begin_transfer probes those folders with a killable child first.)
        let sources: Vec<PathBuf> = srcs
            .into_iter()
            .filter(|p| gust_core::stall::needs_probe(p) || p.exists())
            .collect();
        if sources.is_empty() {
            self.as_mut().forget_clip();
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
        // A cut is consumed by its paste; a copy stays available.
        self.begin_transfer(req, mode == ClipMode::Cut)
    }

    /// The one door every copy and move goes through (paste, drop, send to
    /// the other pane). Name clashes are found BEFORE anything is written;
    /// if there are any, the transfer waits for the user's answers.
    fn begin_transfer(
        mut self: Pin<&mut Self>,
        req: gust_ops::jobs::Request,
        consumes_cut: bool,
    ) -> QString {
        self.as_mut().rust_mut().clipboard_read.cancel();
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        // Finding name clashes stats every source and destination. On a
        // network place that can block for minutes when the server stops
        // answering, so a killable child probes each such folder first and a
        // silent one is refused in words instead of freezing the window.
        if let Some(why) = Self::stalled_among(&req) {
            return QString::from(&why);
        }
        let clashes = gust_ops::jobs::conflicts(&req);
        let pending = PendingTransfer {
            req,
            clashes,
            answers: std::collections::HashMap::new(),
            consumes_cut,
        };
        if pending.clashes.is_empty() {
            return self.launch(pending);
        }
        self.as_mut().rust_mut().pending = Some(pending);
        QString::from(NEEDS_ANSWERS)
    }

    /// The first folder of a transfer that sits on a network place and does
    /// not answer, as a sentence; None when all of them answer (or none can
    /// stall). Local folders are never probed.
    fn stalled_among(req: &gust_ops::jobs::Request) -> Option<String> {
        use gust_core::stall::{needs_probe, place_name_for, probe, Reachability};
        let mut dirs: Vec<PathBuf> = req
            .sources()
            .iter()
            .filter_map(|p| p.parent().map(|q| q.to_path_buf()))
            .collect();
        if let Some(d) = req.dest_dir() {
            dirs.push(d.to_path_buf());
        }
        dirs.sort();
        dirs.dedup();
        for d in dirs.into_iter().filter(|d| needs_probe(d)).take(4) {
            let r = probe(&d, std::time::Duration::from_millis(2500));
            if let Reachability::Stalled { .. } = r {
                return Some(r.message(&place_name_for(&d)));
            }
        }
        None
    }

    fn launch(mut self: Pin<&mut Self>, p: PendingTransfer) -> QString {
        self.as_mut().rust_mut().clipboard_read.cancel();
        let id = format!("job-{}", gust_ops::journal::now_unix());
        // An item with no answer runs under Ask: if its name was taken after
        // the check, the job stops instead of overwriting.
        let handle = gust_ops::jobs::start_resolved(
            id,
            p.req,
            p.answers,
            Self::production_journal(),
            gust_ops::trash::TrashDir::home(),
        );
        let mut r = self.as_mut().rust_mut();
        r.job = Some(handle);
        r.job_rate = gust_ops::rate::RateMeter::new();
        r.job_clock = None;
        // A new operation supersedes the previous undo.
        r.last_job = None;
        r.last_trashed_batch.clear();
        r.last_extra = UndoExtra::None;
        let cut = p.consumes_cut;
        drop(r);
        if cut {
            self.as_mut().clear_clip();
        }
        QString::default()
    }

    fn conflict_json(&self) -> QString {
        let Some(p) = self.pending.as_ref() else {
            return QString::default();
        };
        let Some(i) = p
            .clashes
            .iter()
            .position(|c| !p.answers.contains_key(&c.source))
        else {
            return QString::default();
        };
        use std::os::unix::fs::MetadataExt;
        let c = &p.clashes[i];
        let facts = |path: &std::path::Path| match path.symlink_metadata() {
            Ok(m) if m.is_dir() => (true, "folder".to_string(), human_time(m.mtime())),
            Ok(m) => (false, human_size(m.len()), human_time(m.mtime())),
            Err(_) => (false, "gone".to_string(), String::new()),
        };
        let (old_dir, old_size, old_time) = facts(&c.existing);
        let (_, new_size, new_time) = facts(&c.source);
        let can_replace = gust_ops::trash::TrashDir::home()
            .map(|t| {
                gust_ops::trash::trash_capability(&c.existing, &t) == gust_core::location::Cap::Yes
            })
            .unwrap_or(false);
        let verb = match p.req {
            gust_ops::jobs::Request::Move { .. } => "move",
            _ => "copy",
        };
        // Offered only for a real folder landing on a real folder.
        let can_merge = gust_ops::merge::can_merge(&c.source, &c.existing);
        QString::from(&format!(
            concat!(
                "{{\"index\":{},\"count\":{},\"name\":{},\"isDir\":{},",
                "\"existingSize\":{},\"existingTime\":{},\"incomingSize\":{},\"incomingTime\":{},",
                "\"canReplace\":{},\"canMerge\":{},\"verb\":\"{}\",\"into\":{}}}"
            ),
            i,
            p.clashes.len(),
            Self::json_str(
                &c.existing
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            old_dir,
            Self::json_str(&old_size),
            Self::json_str(&old_time),
            Self::json_str(&new_size),
            Self::json_str(&new_time),
            can_replace,
            can_merge,
            verb,
            Self::json_str(
                &c.existing
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            ),
        ))
    }

    fn conflict_answer(mut self: Pin<&mut Self>, choice: &QString, all: bool) -> QString {
        use gust_ops::journal::ConflictPolicy;
        let policy = match choice.to_string().as_str() {
            "replace" => ConflictPolicy::Replace,
            "keep-both" => ConflictPolicy::KeepBoth,
            "skip" => ConflictPolicy::Skip,
            "merge" => ConflictPolicy::Merge,
            other => return QString::from(&format!("unknown answer {other:?}")),
        };
        let Some(mut p) = self.as_mut().rust_mut().pending.take() else {
            return QString::from("nothing is waiting for an answer");
        };
        let open: Vec<PathBuf> = p
            .clashes
            .iter()
            .filter(|c| !p.answers.contains_key(&c.source))
            .map(|c| c.source.clone())
            .collect();
        for src in open.into_iter().take(if all { usize::MAX } else { 1 }) {
            p.answers.insert(src, policy);
        }
        if p.clashes.iter().all(|c| p.answers.contains_key(&c.source)) {
            return self.launch(p);
        }
        self.as_mut().rust_mut().pending = Some(p);
        QString::from(NEEDS_ANSWERS)
    }

    fn conflict_cancel(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().pending = None;
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
        // /proc answers from the kernel's tables: no file is touched here.
        let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
        self.as_mut().rust_mut().drag_mounts = gust_core::drop::mount_points(&mounts);
    }

    fn drag_count(&self) -> i32 {
        i32::try_from(self.drag_payload.len()).unwrap_or(i32::MAX)
    }

    fn drag_uri_list(&self) -> QString {
        QString::from(&gust_core::clipboard::uri_list_payload(&self.drag_payload))
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
        use gust_core::drop::DropAction;
        let action = if as_move {
            DropAction::Move
        } else {
            DropAction::Copy
        };
        self.plan_drag(row, None, Some(action))
    }

    /// Plan a drop of the in-flight drag onto `row` of this view, or onto
    /// the absolute folder `dest`. `forced` is None for "whatever suits
    /// where it lands": Move within one drive, Copy across drives.
    fn plan_drag(
        &self,
        row: i32,
        dest: Option<PathBuf>,
        forced: Option<gust_core::drop::DropAction>,
    ) -> gust_core::drop::DropPlan {
        use gust_core::drop::{default_action, plan_drop, DropPlan};
        let (target, is_dir) = match dest {
            Some(d) => (d, true),
            None => match self.drop_target(row) {
                Some(t) => t,
                None => return DropPlan::Refused("cannot drop here".into()),
            },
        };
        let sources = self.drag_sources(row);
        let folder = if is_dir {
            target.clone()
        } else {
            target.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        };
        let action = forced.unwrap_or_else(|| default_action(&self.drag_mounts, &sources, &folder));
        plan_drop(&sources, &target, is_dir, action)
    }

    fn drag_mode(mode: i32) -> Option<gust_core::drop::DropAction> {
        match mode {
            1 => Some(gust_core::drop::DropAction::Copy),
            2 => Some(gust_core::drop::DropAction::Move),
            _ => None,
        }
    }

    fn describe_plan(plan: gust_core::drop::DropPlan) -> QString {
        use gust_core::drop::DropPlan;
        match plan {
            DropPlan::Perform { description, .. } => QString::from(&description),
            DropPlan::Refused(why) => QString::from(&why),
            DropPlan::Nothing => QString::default(),
        }
    }

    fn drag_preview(&self, row: i32, mode: i32) -> QString {
        Self::describe_plan(self.plan_drag(row, None, Self::drag_mode(mode)))
    }

    fn drag_preview_into(&self, dest: &QString, mode: i32) -> QString {
        let d = PathBuf::from(dest.to_string());
        Self::describe_plan(self.plan_drag(-1, Some(d), Self::drag_mode(mode)))
    }

    fn drag_drop_on_row(self: Pin<&mut Self>, row: i32, mode: i32) -> QString {
        let plan = self.as_ref().plan_drag(row, None, Self::drag_mode(mode));
        self.perform_drop(plan)
    }

    fn drag_drop_into(self: Pin<&mut Self>, dest: &QString, mode: i32) -> QString {
        // No is_dir() here: the UI thread never touches the filesystem. A
        // folder that has gone away fails in the job, which says so.
        let d = PathBuf::from(dest.to_string());
        let plan = self.as_ref().plan_drag(-1, Some(d), Self::drag_mode(mode));
        self.perform_drop(plan)
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

    fn drop_on_row(self: Pin<&mut Self>, row: i32, as_move: bool) -> QString {
        let plan = self.as_ref().plan_for(row, as_move);
        self.perform_drop(plan)
    }

    /// Run a planned drop through the transfer engine (journal, undo).
    fn perform_drop(self: Pin<&mut Self>, plan: gust_core::drop::DropPlan) -> QString {
        use gust_core::drop::{DropAction, DropPlan};
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
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
            // Links never go through the transfer engine (drop_link_on_row).
            DropAction::Link => return QString::from("Link here makes links, not copies"),
        };
        self.begin_transfer(req, false)
    }

    /// The shared half of "send this selection over there". The destination
    /// comes from the other pane, so it is a real folder the user is looking
    /// at — but it is still checked, because a pane can be showing a place
    /// that has gone away underneath it.
    fn send_selection_to(self: Pin<&mut Self>, dest: &QString, as_move: bool) -> QString {
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
        self.begin_transfer(req, false)
    }

    fn copy_selection_to(self: Pin<&mut Self>, dest: &QString) -> QString {
        self.send_selection_to(dest, false)
    }

    fn move_selection_to(self: Pin<&mut Self>, dest: &QString) -> QString {
        self.send_selection_to(dest, true)
    }

    fn drag_mime_json(&self) -> QString {
        use gust_core::clipboard::{gnome_payload, uri_list_payload, ClipVerb};
        let p = &self.drag_payload;
        let text = p
            .iter()
            .map(|x| x.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        QString::from(&format!(
            r#"{{"uris":{},"text":{},"gnome":{}}}"#,
            Self::json_str(&uri_list_payload(p)),
            Self::json_str(&text),
            Self::json_str(&gnome_payload(ClipVerb::Copy, p)),
        ))
    }

    fn drop_enter(mut self: Pin<&mut Self>) {
        // /proc answers from the kernel's tables: no file is touched here.
        let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
        self.as_mut().rust_mut().drag_mounts = gust_core::drop::mount_points(&mounts);
    }

    fn drag_end(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().drag_payload.clear();
    }

    fn uri_plan(
        &self,
        row: i32,
        dest: &QString,
        uris: &QString,
        mode: i32,
        gust: bool,
    ) -> gust_core::drop::DropPlan {
        use gust_core::drop::{
            default_action, paths_from_uri_list, plan_drop, DropAction, DropPlan,
        };
        let sources = paths_from_uri_list(&uris.to_string());
        if sources.is_empty() {
            return DropPlan::Nothing;
        }
        let dest = dest.to_string();
        let (target, is_dir) = if !dest.is_empty() {
            (PathBuf::from(dest), true)
        } else {
            match self.drop_target(row) {
                Some(t) => t,
                None => return DropPlan::Refused("cannot drop here".into()),
            }
        };
        let folder = if is_dir {
            target.clone()
        } else {
            target.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        };
        let action = match Self::drag_mode(mode) {
            Some(a) => a,
            // Another Gust window: the same rule as a drop inside one.
            None if gust => default_action(&self.drag_mounts, &sources, &folder),
            // Another application cannot say what it meant: copy.
            None => DropAction::Copy,
        };
        plan_drop(&sources, &target, is_dir, action)
    }

    fn uri_preview(
        &self,
        row: i32,
        dest: &QString,
        uris: &QString,
        mode: i32,
        gust: bool,
    ) -> QString {
        Self::describe_plan(self.uri_plan(row, dest, uris, mode, gust))
    }

    fn uri_drop(
        self: Pin<&mut Self>,
        row: i32,
        dest: &QString,
        uris: &QString,
        mode: i32,
        gust: bool,
    ) -> QString {
        let plan = self.as_ref().uri_plan(row, dest, uris, mode, gust);
        if matches!(plan, gust_core::drop::DropPlan::Nothing) {
            return QString::default();
        }
        self.perform_drop(plan)
    }

    fn drop_uris(self: Pin<&mut Self>, row: i32, uris: &QString, as_move: bool) -> QString {
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
                    DropAction::Link => return QString::from("Link here makes links, not copies"),
                };
                self.begin_transfer(req, false)
            }
            DropPlan::Refused(why) => QString::from(&why),
            DropPlan::Nothing => QString::default(),
        }
    }

    // --- direct operations ----------------------------------------------

    fn rename_row(mut self: Pin<&mut Self>, row: i32, new_name: &QString) -> QString {
        if self.as_ref().job.is_some() {
            return QString::from("another operation is still running");
        }
        let Some(path) = self.as_ref().path_of_row(row) else {
            return QString::from("no such row");
        };
        let name = std::ffi::OsString::from(new_name.to_string());
        if path.file_name() == Some(name.as_os_str()) {
            return QString::default();
        }
        match gust_ops::attrs::rename_recorded(&path, &name) {
            Ok(made) => {
                {
                    let mut state = self.as_mut().rust_mut();
                    state.last_job = None;
                    state.last_trashed_batch.clear();
                    state.last_bulk_rename.clear();
                    state.last_link = None;
                    state.last_modes.clear();
                    state.last_extra = UndoExtra::Renamed(made);
                }
                self.as_mut().relist_focusing(name);
                QString::default()
            }
            Err(gust_ops::engine::OpError::Conflict(_)) => {
                QString::from("a file with that name already exists")
            }
            Err(e) => QString::from(&format!("{e}")),
        }
    }

    /// The files a bulk rename would act on, in the order shown on screen.
    ///
    /// `target_paths` already resolves the selection against the VISIBLE
    /// entries, so the order is the user's current sort — numbering that
    /// matches nothing on screen would be useless. Selection stores names,
    /// not indices, which is why this cannot be done by index arithmetic.
    fn bulk_targets(&self, row: i32) -> Vec<std::path::PathBuf> {
        if self.selection.is_empty() {
            return self.path_of_row(row).into_iter().collect();
        }
        self.target_paths()
    }

    fn bulk_preview(&self, row: i32, pattern: &QString) -> QString {
        let Some(dir) = self.key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from(r#"{"ok":false,"summary":"Not a folder"}"#);
        };
        let files = self.bulk_targets(row);
        let plan = gust_ops::bulk::plan(&dir, &files, &pattern.to_string());

        let mut rows = String::from("[");
        for (i, r) in plan.renames.iter().take(200).enumerate() {
            if i > 0 {
                rows.push(',');
            }
            rows.push_str(&format!(
                r#"{{"from":{},"to":{},"unchanged":{}}}"#,
                Self::json_str(&r.old_name),
                Self::json_str(&r.new_name),
                r.unchanged
            ));
        }
        rows.push(']');

        let mut problems = String::from("[");
        for (i, p) in plan.problems.iter().take(50).enumerate() {
            if i > 0 {
                problems.push(',');
            }
            problems.push_str(&Self::json_str(&p.message()).to_string());
        }
        problems.push(']');

        QString::from(format!(
            r#"{{"ok":{},"summary":{},"changing":{},"rows":{},"problems":{}}}"#,
            plan.is_safe(),
            Self::json_str(&plan.summary()),
            plan.changing(),
            rows,
            problems
        ))
    }

    fn bulk_apply(self: Pin<&mut Self>, row: i32, pattern: &QString) -> QString {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be renamed in.");
        };
        let files = self.as_ref().bulk_targets(row);
        let plan = gust_ops::bulk::plan(&dir, &files, &pattern.to_string());
        self.bulk_execute(dir, plan)
    }

    fn bulk_names(&self, row: i32) -> QString {
        let names: Vec<String> = self
            .bulk_targets(row)
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if names.iter().any(|n| n.contains('\n') || n.contains('\r')) {
            return QString::default();
        }
        QString::from(names.join("\n"))
    }

    /// The card's text as one name per line. A trailing newline (an editor
    /// adds one) is not an extra empty name.
    fn split_names(text: &str) -> Vec<String> {
        let t = text.strip_suffix('\n').unwrap_or(text);
        t.split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect()
    }

    fn bulk_check(&self, row: i32, names: &QString) -> QString {
        let Some(dir) = self.key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from(
                r#"{"ok":false,"summary":"Not a folder","changing":0,"lines":[]}"#,
            );
        };
        let files = self.bulk_targets(row);
        let plan = gust_ops::bulk::plan_names(&dir, &files, &Self::split_names(&names.to_string()));
        let lines: Vec<String> = plan
            .line_problems
            .iter()
            .map(|l| match l {
                Some(m) => Self::json_str(m),
                None => "null".into(),
            })
            .collect();
        QString::from(format!(
            r#"{{"ok":{},"summary":{},"changing":{},"lines":[{}]}}"#,
            plan.is_safe(),
            Self::json_str(&plan.summary()),
            plan.changing(),
            lines.join(",")
        ))
    }

    fn bulk_apply_names(self: Pin<&mut Self>, row: i32, names: &QString) -> QString {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("This place cannot be renamed in.");
        };
        let files = self.as_ref().bulk_targets(row);
        let plan = gust_ops::bulk::plan_names(&dir, &files, &Self::split_names(&names.to_string()));
        self.bulk_execute(dir, plan)
    }

    /// Run a bulk rename plan and record its one-step Undo.
    fn bulk_execute(
        mut self: Pin<&mut Self>,
        dir: std::path::PathBuf,
        plan: gust_ops::bulk::RenamePlan,
    ) -> QString {
        // The plan is rebuilt by the caller rather than carried from the
        // preview: the folder may have changed while the card was open, and
        // renaming against a stale plan is how a bystander gets overwritten.
        if !plan.is_safe() {
            return QString::from(plan.summary());
        }

        let outcome = gust_ops::bulk::execute(&dir, &plan);

        // One journal entry, not forty: one Ctrl+Z undoes one user action.
        if !outcome.done.is_empty() {
            self.as_mut().rust_mut().last_bulk_rename = outcome.done.clone();
            self.as_mut().rust_mut().last_link = None;
            self.as_mut().rust_mut().last_modes.clear();
            self.as_mut().rust_mut().last_extra = UndoExtra::None;
        }
        self.as_mut().rust_mut().selection.clear();
        self.as_mut().reload();

        let msg = outcome.summary();
        self.as_mut().set_status(QString::from(&msg));
        if outcome.failed.is_empty() {
            QString::default()
        } else {
            QString::from(outcome.failed.join(", "))
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
                self.as_mut().relist_focusing(raw);
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
        self.as_mut().rust_mut().clipboard_read.cancel();
        let handle = gust_ops::jobs::start_journalled(
            id,
            gust_ops::jobs::Request::CopyAs {
                source: src,
                dest_path,
            },
            gust_ops::journal::ConflictPolicy::Ask,
            Self::production_journal(),
        );
        self.as_mut().rust_mut().job = Some(handle);
        QString::default()
    }

    // --- job lifecycle ---------------------------------------------------

    fn job_running(&self) -> bool {
        self.job.is_some()
    }

    fn job_stopping(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| j.cancel_requested() && !j.is_finished())
    }

    fn archive_running(&self) -> bool {
        self.archive_work.is_running()
    }

    fn work_on_place(&self, kind: QString, name: QString) -> QString {
        let name = name.to_string();
        // Each of these is a path computed from the runtime directory; none
        // of them looks at the mount itself.
        let mount = match kind.to_string().as_str() {
            "ssh" => gust_remote::mount::mount_point_for(&name),
            "tailnet" => gust_remote::tailscale::mount_point(&name),
            "cloud" => Self::cloud_mount_point(&name),
            _ => None,
        };
        let Some(mount) = mount else {
            return QString::default();
        };
        if self.sharing_busy && crate::place_work::touches(&self.sharing_work.active_paths, &mount)
        {
            return QString::from(if self.sharing_work.cancel.is_cancelled() {
                "stopping"
            } else {
                "transfer"
            });
        }
        if let Some(job) = self.job.as_ref() {
            if crate::place_work::touches(&crate::place_work::job_paths(job), &mount) {
                return QString::from(if self.job_stopping() {
                    "stopping"
                } else {
                    "transfer"
                });
            }
        }
        if self.archive_work.touches(&mount) {
            return QString::from("archive");
        }
        QString::default()
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
        let from = self.as_ref().cursor.max(0) as usize;
        self.as_mut().rust_mut().selection.extend_by_name(
            &names,
            from,
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

    fn sel_invert(mut self: Pin<&mut Self>) {
        let names: Vec<std::ffi::OsString> = self
            .as_ref()
            .visible_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        self.as_mut().rust_mut().selection.invert_by_name(&names);
        self.as_mut().refresh_selection_flags();
    }

    fn band_begin(mut self: Pin<&mut Self>, additive: bool) {
        let base = if additive {
            self.as_ref().selection.names_vec()
        } else {
            Vec::new()
        };
        self.as_mut().rust_mut().band_base = base;
    }

    fn band_update(mut self: Pin<&mut Self>, r1: i32, r2: i32, c1: i32, c2: i32, cols: i32) {
        let cols = cols.max(1) as usize;
        let (r1, r2) = (r1.min(r2).max(0) as usize, r1.max(r2).max(0) as usize);
        let (c1, c2) = (
            c1.min(c2).clamp(0, cols as i32 - 1) as usize,
            c1.max(c2).clamp(0, cols as i32 - 1) as usize,
        );
        let picked: Vec<std::ffi::OsString> = {
            let me = self.as_ref();
            let vis = me.visible_entries();
            let mut out = Vec::new();
            // Bounded by what is on screen plus the drag: never the whole
            // folder in one go, whatever the pointer does.
            for r in r1..=r2.min(r1 + 4000) {
                for c in c1..=c2 {
                    if let Some(e) = vis.get(r * cols + c) {
                        out.push(e.name.clone());
                    }
                }
            }
            out
        };
        let base = std::mem::take(&mut self.as_mut().rust_mut().band_base);
        self.as_mut().rust_mut().selection.set_band(&base, &picked);
        self.as_mut().rust_mut().band_base = base;
        self.as_mut().refresh_selection_flags();
    }

    fn key_action(&self, preset: &QString, key: i32, mods: i32) -> QString {
        use gust_core::keymap::{action_for, Preset};
        let p = Preset::parse(&preset.to_string()).unwrap_or(Preset::Standard);
        QString::from(&action_for(p, key as u32, mods as u32).unwrap_or_default())
    }

    fn keymap_json(&self, preset: &QString) -> QString {
        use gust_core::keymap::{keymap_json, Preset};
        let p = Preset::parse(&preset.to_string()).unwrap_or(Preset::Standard);
        QString::from(&keymap_json(p))
    }

    fn new_file(mut self: Pin<&mut Self>, name: &QString) -> QString {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot create a file here");
        };
        let raw = std::ffi::OsString::from(name.to_string());
        if let Err(e) = gust_ops::recursive::validate_name(&raw) {
            return QString::from(&format!("{e}"));
        }
        match gust_ops::create::new_empty_file(&dir, &raw) {
            Ok(()) => {
                self.as_mut().relist_focusing(raw);
                QString::default()
            }
            Err(e) => QString::from(&e),
        }
    }

    fn device_here(&self) -> QString {
        let Some(here) = self.key.as_path() else {
            return QString::default();
        };
        let dev = gust_core::devices::list_devices().into_iter().find(|d| {
            d.mount_point
                .as_ref()
                .is_some_and(|mp| here.starts_with(mp))
        });
        QString::from(&dev.map(|d| d.path).unwrap_or_default())
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
        // Empty and single selections display no byte total. Avoid walking
        // a huge folder for the common case on every cursor update.
        if self.selection.len() <= 1 {
            return QString::from(&self.selection.summary(0));
        }
        let chosen: Vec<&Entry> = self
            .visible
            .iter()
            .filter_map(|&i| self.all.get(i))
            .filter(|e| self.selection.contains(&e.name))
            .collect();
        // A total over rows whose sizes were never read would be a wrong
        // number stated with confidence; leave it out until all are known.
        let bytes: u64 = if chosen.iter().any(|e| e.meta.is_none()) {
            0
        } else {
            chosen.iter().filter_map(|e| e.meta.map(|m| m.size)).sum()
        };
        QString::from(&self.selection.summary(bytes))
    }

    /// The paths a command should act on.
    ///
    /// The selection when there is one, otherwise the cursor row. Resolved
    /// ONCE, when the command is issued (DESIGN.md §4.1).
    fn jump_prepare(self: Pin<&mut Self>) {
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-jump".into())
            .spawn(move || {
                let bookmarks: Vec<PathBuf> = gust_core::bookmarks::default_path()
                    .map(|f| gust_core::bookmarks::load(&f))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|b| b.uri.is_none())
                    .map(|b| b.path)
                    .filter(|p| !gust_core::stall::needs_probe(p))
                    .collect();
                let cands = gust_core::jump::gather(&bookmarks);
                let _ = thread.queue(move |mut model| {
                    model.as_mut().rust_mut().jump_cands = cands;
                    let r = model.as_ref().jump_rev + 1;
                    model.as_mut().set_jump_rev(r);
                });
            })
            .ok();
    }

    fn jump_query(&self, query: &QString, limit: i32) -> QString {
        let q = query.to_string();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let rows: Vec<String> = gust_core::jump::rank(&q, &self.jump_cands, limit.max(0) as usize)
            .into_iter()
            .map(|c| {
                let full = c.path.to_string_lossy().into_owned();
                let shown = match home.as_ref().and_then(|h| c.path.strip_prefix(h).ok()) {
                    Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
                    Some(rest) => format!("~/{}", rest.to_string_lossy()),
                    None => full.clone(),
                };
                format!(
                    "{{\"path\":{},\"shown\":{},\"source\":{}}}",
                    Self::json_str(&full),
                    Self::json_str(&shown),
                    Self::json_str(c.source.label())
                )
            })
            .collect();
        QString::from(&format!("[{}]", rows.join(",")))
    }

    fn jump_complete(self: Pin<&mut Self>, typed: &QString) {
        let typed = typed.to_string();
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return;
        };
        std::thread::Builder::new()
            .name("gust-complete".into())
            .spawn(move || {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                let Some(dir) = gust_core::jump::completion_dir(&typed, &home) else {
                    return;
                };
                // A stalled network mount must not be listed from here.
                if gust_core::stall::needs_probe(&dir) {
                    return;
                }
                let kids: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .take(5000)
                            .collect()
                    })
                    .unwrap_or_default();
                let done = gust_core::jump::complete(&typed, &kids).unwrap_or_default();
                let _ = thread.queue(move |mut model| {
                    model
                        .as_mut()
                        .set_jump_completion(QString::from(&format!("{typed}\n{done}")));
                });
            })
            .ok();
    }

    fn custom_targets(&self) -> Vec<gust_core::actions::Target> {
        self.target_paths()
            .into_iter()
            .map(|p| {
                let is_dir = p.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false);
                gust_core::actions::Target { path: p, is_dir }
            })
            .collect()
    }

    fn custom_actions_json(&self) -> QString {
        let Some(dir) = gust_core::actions::actions_dir() else {
            return QString::from(r#"{"actions":[],"problems":[]}"#);
        };
        let (acts, problems) = gust_core::actions::load(&dir);
        let targets = self.custom_targets();
        let rows: Vec<String> = acts
            .iter()
            .filter(|a| a.applies_to(&targets))
            .map(|a| {
                format!(
                    "{{\"id\":{},\"name\":{}}}",
                    Self::json_str(&a.id),
                    Self::json_str(&a.name)
                )
            })
            .collect();
        let probs: Vec<String> = problems
            .iter()
            .map(|p| Self::json_str(p).to_string())
            .collect();
        QString::from(&format!(
            "{{\"actions\":[{}],\"problems\":[{}]}}",
            rows.join(","),
            probs.join(",")
        ))
    }

    fn run_custom_action(mut self: Pin<&mut Self>, id: &QString) -> QString {
        if admin_session() {
            return QString::from(ADMIN_REFUSAL);
        }
        let Some(dir) = gust_core::actions::actions_dir() else {
            return QString::from("no actions folder");
        };
        let id = id.to_string();
        let (acts, _) = gust_core::actions::load(&dir);
        let Some(act) = acts.into_iter().find(|a| a.id == id) else {
            return QString::from("that action no longer exists");
        };
        let targets = self.as_ref().custom_targets();
        if !act.applies_to(&targets) {
            return QString::from(&format!("{} does not apply to this selection", act.name));
        }
        let runs = act.argvs(&targets);
        if runs.len() > gust_core::open::with::MAX_OPEN_AT_ONCE {
            return QString::from(&format!(
                "{} would run {} times; select at most {}",
                act.name,
                runs.len(),
                gust_core::open::with::MAX_OPEN_AT_ONCE
            ));
        }
        if gust_core::open::which(&runs[0][0]).is_none() {
            return QString::from(&format!("{} is not installed", runs[0][0]));
        }
        let cwd = self.as_ref().key.as_path().map(|p| p.to_path_buf());
        let mut started = 0usize;
        for argv in &runs {
            let mut cmd = std::process::Command::new(&argv[0]);
            cmd.args(&argv[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Some(d) = &cwd {
                cmd.current_dir(d);
            }
            match cmd.spawn() {
                Ok(mut child) => {
                    started += 1;
                    // Reap it: a finished action must not linger as a zombie.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                }
                Err(e) => return QString::from(&format!("{}: {e}", act.name)),
            }
        }
        self.as_mut().set_status(QString::from(&format!(
            "Ran {} on {} item{}",
            act.name,
            targets.len(),
            if targets.len() == 1 { "" } else { "s" }
        )));
        let _ = started;
        QString::default()
    }

    fn run_argv_for(&self, row: i32) -> Result<Vec<String>, String> {
        use std::os::unix::fs::MetadataExt;
        let path = self
            .path_of_row(row)
            .ok_or_else(|| "nothing to run here".to_string())?;
        let md = path
            .symlink_metadata()
            .map_err(|_| "that file is gone".to_string())?;
        gust_core::open::run_argv(&path, md.mode(), md.is_file())
    }

    fn run_check(&self, row: i32) -> QString {
        match self.run_argv_for(row) {
            Ok(_) => QString::default(),
            Err(e) => QString::from(&e),
        }
    }

    fn run_row(mut self: Pin<&mut Self>, row: i32) -> QString {
        if admin_session() {
            return QString::from(ADMIN_REFUSAL);
        }
        let argv = match self.as_ref().run_argv_for(row) {
            Ok(v) => v,
            Err(e) => return QString::from(&e),
        };
        if gust_core::open::which(&argv[0]).is_none() {
            return QString::from("no terminal launcher (xdg-terminal-exec) is installed");
        }
        match std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                let name = self.as_ref().entry_name(row).to_string();
                self.as_mut()
                    .set_status(QString::from(&format!("Running {name} in a terminal")));
                QString::default()
            }
            Err(e) => QString::from(&format!("could not start the terminal: {e}")),
        }
    }

    fn set_clip(mut self: Pin<&mut Self>, paths: Vec<PathBuf>, mode: ClipMode) {
        {
            let mut r = self.as_mut().rust_mut();
            r.clipboard_read.clipboard_changed();
            r.clip_set = paths.iter().cloned().collect();
            r.clipboard = Some((paths, mode));
        }
        self.as_mut().refresh_clip_marks();
    }

    fn clear_clip(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().clipboard_read.clipboard_changed();
        self.forget_clip();
    }

    fn forget_clip(mut self: Pin<&mut Self>) {
        {
            let mut r = self.as_mut().rust_mut();
            r.clipboard_read.cancel();
            r.clipboard = None;
            r.clip_set.clear();
        }
        self.as_mut().refresh_clip_marks();
    }

    /// Repaint the copied/cut mark on every row (only that role).
    fn refresh_clip_marks(mut self: Pin<&mut Self>) {
        let n = self.as_ref().visible.len() as i32;
        if n == 0 {
            return;
        }
        let root = cxx_qt_lib::QModelIndex::default();
        let mut roles = cxx_qt_lib::QVector::<i32>::default();
        roles.append(ROLE_CLIP);
        // Safety: GUI thread.
        unsafe {
            let top = self.as_ref().index(0, 0, &root);
            let bottom = self.as_ref().index(n - 1, 0, &root);
            self.as_mut().data_changed(&top, &bottom, &roles);
        }
    }

    fn set_tags(mut self: Pin<&mut Self>, op: &QString) -> QString {
        use gust_ops::tags::TagOp;
        let op = match op.to_string().as_str() {
            "clear" => TagOp::Clear,
            n => match n.parse::<usize>() {
                Ok(i) if (1..=7).contains(&i) => TagOp::Toggle(i),
                _ => return QString::from("unknown tag"),
            },
        };
        let targets = self.as_ref().target_paths();
        if targets.is_empty() {
            return QString::from("Nothing to tag here");
        }
        // Local xattr calls, one per file: bounded by the selection. A
        // network folder is refused rather than risk a stalled UI thread.
        if let Some(dir) = self.as_ref().key.as_path() {
            if gust_core::stall::needs_probe(dir) {
                return QString::from("Tags can only be set on this computer's own drives.");
            }
        }
        let out = gust_ops::tags::set_tags(&targets, op);
        let n = out.changed.len();
        if n > 0 {
            let mut r = self.as_mut().rust_mut();
            r.last_extra = UndoExtra::Tags(out.changed.clone());
            r.last_modes.clear();
            r.last_link = None;
            r.last_job = None;
            r.last_bulk_rename.clear();
            r.last_trashed_batch.clear();
        }
        self.as_mut().reload();
        if n == 0 {
            if !out.failed.is_empty() {
                return QString::from(&out.failed.join("; "));
            }
            self.as_mut().set_status(QString::from("Nothing changed"));
            return QString::default();
        }
        let what = match op {
            TagOp::Clear => "Colours removed from".to_string(),
            TagOp::Toggle(i) => {
                let name = gust_core::tags::NAMES[i - 1];
                if out.changed.iter().any(|c| {
                    gust_core::tags::mask_of(
                        &c.after
                            .as_deref()
                            .map(gust_core::tags::parse)
                            .unwrap_or_default(),
                    ) & (1 << (i - 1))
                        != 0
                }) {
                    format!("{name} added to")
                } else {
                    format!("{name} removed from")
                }
            }
        };
        let mut msg = format!("{what} {n} item{}", if n == 1 { "" } else { "s" });
        if !out.failed.is_empty() {
            msg.push_str(&format!(" · not tagged: {}", out.failed.join("; ")));
        }
        self.as_mut().set_status(QString::from(&msg));
        QString::default()
    }

    fn set_owner(mut self: Pin<&mut Self>, spec: &QString) -> QString {
        let spec = match gust_ops::attrs::OwnerSpec::parse(&spec.to_string()) {
            Ok(s) => s,
            Err(e) => return QString::from(&e),
        };
        let targets = self.as_ref().target_paths();
        if targets.is_empty() {
            return QString::from("nothing to change");
        }
        let out = gust_ops::attrs::set_owners(&targets, spec);
        let n = out.changed.len();
        if n > 0 {
            let mut r = self.as_mut().rust_mut();
            r.last_extra = UndoExtra::Owners(out.changed);
            r.last_modes.clear();
            r.last_link = None;
            r.last_job = None;
            r.last_bulk_rename.clear();
            r.last_trashed_batch.clear();
        }
        self.as_mut().reload();
        if n == 0 {
            if !out.failed.is_empty() {
                return QString::from(&out.failed.join("; "));
            }
            self.as_mut()
                .set_status(QString::from("They already had that owner"));
            return QString::default();
        }
        let mut msg = format!("Owner changed on {n} item{}", if n == 1 { "" } else { "s" });
        if out.unchanged > 0 {
            msg.push_str(&format!(", {} already had it", out.unchanged));
        }
        if !out.failed.is_empty() {
            msg.push_str(&format!(
                "; {} not changed: {}",
                out.failed.len(),
                out.failed.join("; ")
            ));
        }
        msg.push_str("  ·  Ctrl+Z puts them back");
        self.as_mut().set_status(QString::from(&msg));
        QString::default()
    }

    fn target_name(&self) -> QString {
        self.target_paths()
            .into_iter()
            .next()
            .map(|p| QString::from(&display_file_name(&p)))
            .unwrap_or_default()
    }

    fn owner_text(&self) -> QString {
        use std::os::unix::fs::MetadataExt;
        let Some(p) = self.target_paths().into_iter().next() else {
            return QString::default();
        };
        let Ok(md) = p.symlink_metadata() else {
            return QString::default();
        };
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        QString::from(&gust_core::properties::owner_names(
            md.uid(),
            md.gid(),
            &passwd,
            &group,
        ))
    }

    fn paste_image(mut self: Pin<&mut Self>) -> QString {
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot paste here");
        };
        let Some(thread) = self.as_ref().qt_thread_handle() else {
            return QString::from("cannot paste here");
        };
        self.as_mut().rust_mut().clipboard_read.cancel();
        std::thread::Builder::new()
            .name("gust-paste-image".into())
            .spawn(move || {
                use gust_core::clipboard::{read_image, ClipImage};
                let r = match read_image(gust_ops::create::PASTE_MAX_BYTES) {
                    ClipImage::None => Err("nothing to paste".to_string()),
                    ClipImage::Refused(w) => Err(w),
                    ClipImage::Image { bytes, ext } => {
                        let stamp = gust_core::properties::full_time(
                            gust_ops::journal::now_unix(),
                            gust_core::properties::local_offset_seconds(),
                        )
                        .replace(':', ".");
                        let name = gust_ops::create::pasted_image_name(&dir, &stamp, ext);
                        gust_ops::create::create_from_bytes(&dir, &name, &bytes)
                    }
                };
                let _ = thread.queue(move |model| Self::paste_image_done(model, r));
            })
            .ok();
        QString::default()
    }

    fn paste_image_done(mut self: Pin<&mut Self>, r: Result<gust_ops::create::Created, String>) {
        let op = QString::from("paste-image");
        match r {
            Ok(c) => {
                let name = c
                    .path
                    .file_name()
                    .map(|n| n.to_os_string())
                    .unwrap_or_default();
                let here = c.path.parent().map(|p| p.to_path_buf());
                let msg = format!("Saved {}  ·  Ctrl+Z removes it", name.to_string_lossy());
                {
                    let mut r = self.as_mut().rust_mut();
                    r.last_extra = UndoExtra::Created(c);
                    r.last_modes.clear();
                    r.last_link = None;
                    r.last_job = None;
                    r.last_bulk_rename.clear();
                    r.last_trashed_batch.clear();
                }
                if here.as_deref() == self.as_ref().key.as_path() {
                    self.as_mut().relist_focusing(name);
                }
                self.as_mut()
                    .place_done(op, QString::from(&msg), QString::default());
            }
            Err(e) => self
                .as_mut()
                .place_done(op, QString::default(), QString::from(&e)),
        }
    }

    fn drop_link_preview(&self, row: i32) -> QString {
        use gust_core::drop::{plan_drop, DropAction, DropPlan};
        let Some((target, is_dir)) = self.drop_target(row) else {
            return QString::default();
        };
        match plan_drop(&self.drag_sources(row), &target, is_dir, DropAction::Link) {
            DropPlan::Perform { description, .. } => QString::from(&description),
            DropPlan::Refused(why) => QString::from(&why),
            DropPlan::Nothing => QString::default(),
        }
    }

    fn drop_link_on_row(mut self: Pin<&mut Self>, row: i32) -> QString {
        use gust_core::drop::{plan_drop, DropAction, DropPlan};
        let Some((target, is_dir)) = self.as_ref().drop_target(row) else {
            return QString::from("cannot drop here");
        };
        let (sources, dest) = match plan_drop(
            &self.as_ref().drag_sources(row),
            &target,
            is_dir,
            DropAction::Link,
        ) {
            DropPlan::Perform {
                sources,
                destination,
                ..
            } => (sources, destination),
            DropPlan::Refused(why) => return QString::from(&why),
            DropPlan::Nothing => return QString::default(),
        };
        let mut made = Vec::new();
        let mut failed = Vec::new();
        for src in &sources {
            // The link keeps the item's own name when it is free there.
            let own = src
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            let name = if !own.is_empty() && dest.join(&own).symlink_metadata().is_err() {
                own
            } else {
                gust_ops::attrs::link_name_for(&dest, src)
            };
            match gust_ops::attrs::create_link(src, &dest, &name) {
                Ok(m) => made.push(m),
                Err(e) => failed.push(format!("{}: {e}", name.to_string_lossy())),
            }
        }
        let n = made.len();
        if n > 0 {
            let mut r = self.as_mut().rust_mut();
            r.last_extra = UndoExtra::Links(made);
            r.last_modes.clear();
            r.last_link = None;
            r.last_job = None;
            r.last_bulk_rename.clear();
            r.last_trashed_batch.clear();
        }
        self.as_mut().reload();
        if n == 0 {
            return QString::from(&failed.join("; "));
        }
        let mut msg = format!(
            "Linked {n} item{} in {}  ·  Ctrl+Z removes {}",
            if n == 1 { "" } else { "s" },
            dest.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
            if n == 1 { "it" } else { "them" }
        );
        if !failed.is_empty() {
            msg.push_str(&format!("; not linked: {}", failed.join("; ")));
        }
        self.as_mut().set_status(QString::from(&msg));
        QString::default()
    }

    fn pause_job(self: Pin<&mut Self>) {
        if let Some(job) = self.job.as_ref() {
            job.pause();
        }
    }

    fn resume_job(self: Pin<&mut Self>) {
        if let Some(job) = self.job.as_ref() {
            job.resume();
        }
    }

    fn is_admin(&self) -> bool {
        admin_session()
    }

    fn open_as_admin(&self) -> QString {
        if admin_session() {
            return QString::from("This already is the administrator window.");
        }
        let Some(pkexec) = gust_core::open::which("pkexec") else {
            return QString::from(
                "Opening as administrator needs polkit (pkexec), which is not installed.",
            );
        };
        // The folder on screen: what the user is looking at is what opens.
        let folder = self.key.as_path().map(|p| p.to_path_buf());
        let Some(folder) = folder else {
            return QString::from("Only a local folder can be opened as administrator.");
        };
        // The root side refuses a folder that is gone; say so here instead
        // of asking for a password that leads nowhere.
        if !folder.is_dir() {
            return QString::from(
                "This folder no longer exists, so it cannot be opened as administrator.",
            );
        }
        let Ok(exe) = std::env::current_exe() else {
            return QString::from("Gust could not find its own program file.");
        };
        let socket = match (
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("XDG_RUNTIME_DIR"),
        ) {
            (Some(w), _) if PathBuf::from(&w).is_absolute() => PathBuf::from(w),
            (Some(w), Some(rt)) => PathBuf::from(rt).join(w),
            _ => return QString::from("Opening as administrator needs a Wayland session."),
        };
        let theme = gust_core::theme::default_state_dir().unwrap_or_default();
        // Its own probe file, never a sibling name the main window stages to.
        let qa = std::env::var("GUST_QA_STATE")
            .map(|p| match p.strip_suffix(".json") {
                Some(b) => format!("{b}-admin.json"),
                None => format!("{p}-admin"),
            })
            .unwrap_or_else(|_| "-".into());
        let child = std::process::Command::new(pkexec)
            .arg(exe)
            .arg("--admin")
            .arg(&socket)
            .arg(&theme)
            .arg(&qa)
            .arg(&folder)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut c) => {
                // Reaped on its own thread: polkit's prompt can wait for the
                // user as long as it likes without the window waiting too.
                std::thread::spawn(move || {
                    let _ = c.wait();
                });
                QString::default()
            }
            Err(e) => QString::from(&format!("Could not ask for administrator rights: {e}")),
        }
    }

    fn session_json(&self) -> QString {
        use gust_core::session::Session;
        if admin_session()
            || std::env::var_os("GUST_CHOOSER").is_some_and(|v| !v.is_empty())
            || std::env::args_os()
                .skip(1)
                .any(|a| !a.to_string_lossy().starts_with('-'))
        {
            return QString::default();
        }
        let on = gust_core::settings::Settings::default_path()
            .map(|p| gust_core::settings::Settings::load(&p).restore_tabs)
            .unwrap_or(false);
        if !on {
            return QString::default();
        }
        let Some(path) = Session::default_path() else {
            return QString::default();
        };
        let s = Session::load(&path).still_there();
        if s.tabs.is_empty() {
            return QString::default();
        }
        let tabs: Vec<String> = s
            .tabs
            .iter()
            .map(|t| {
                let name = t
                    .key
                    .as_path()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "/".into());
                format!(
                    "{{\"token\":{},\"view\":{},\"name\":{}}}",
                    Self::json_str(&t.key.to_token()),
                    Self::json_str(&t.view),
                    Self::json_str(&name)
                )
            })
            .collect();
        QString::from(&format!(
            "{{\"active\":{},\"tabs\":[{}]}}",
            s.active,
            tabs.join(",")
        ))
    }

    fn session_save(&self, lines: &QString, active: i32) {
        if admin_session() {
            return;
        }
        let parts: Vec<(String, String)> = lines
            .to_string()
            .lines()
            .filter_map(|l| {
                l.split_once(' ')
                    .map(|(t, v)| (t.to_string(), v.to_string()))
            })
            .collect();
        let s = gust_core::session::Session::from_parts(&parts, active.max(0) as usize);
        let Some(path) = gust_core::session::Session::default_path() else {
            return;
        };
        // Off the GUI thread: it reads the setting and fsyncs. Nothing is
        // written while restoring is off, so no folder list is kept then.
        std::thread::Builder::new()
            .name("gust-session".into())
            .spawn(move || {
                let on = gust_core::settings::Settings::default_path()
                    .map(|p| gust_core::settings::Settings::load(&p).restore_tabs)
                    .unwrap_or(false);
                if on {
                    let _ = s.save(&path);
                }
            })
            .ok();
    }

    fn token_for(&self, path: &QString) -> QString {
        QString::from(&LocationKey::local(PathBuf::from(path.to_string())).to_token())
    }

    fn start_at_token(mut self: Pin<&mut Self>, token: &QString) {
        self.as_mut().navigate_token(token);
        // A restored tab starts here; Back must not lead to Home.
        self.as_mut().rust_mut().back.clear();
    }

    fn location_token(&self) -> QString {
        QString::from(&self.key.to_token())
    }

    fn navigate_token(self: Pin<&mut Self>, token: &QString) {
        if let Some(p) = LocationKey::from_token(&token.to_string())
            .and_then(|k| k.as_path().map(|p| p.to_path_buf()))
        {
            self.navigate(&QString::from(&p.to_string_lossy().to_string()));
        }
    }

    fn make_link(mut self: Pin<&mut Self>) -> QString {
        let targets = self.as_ref().target_paths();
        let [target] = targets.as_slice() else {
            return QString::from("choose one item to link to");
        };
        let Some(dir) = self.as_ref().key.as_path().map(|p| p.to_path_buf()) else {
            return QString::from("cannot make a link here");
        };
        let name = gust_ops::attrs::link_name_for(&dir, target);
        match gust_ops::attrs::create_link(target, &dir, &name) {
            Ok(made) => {
                {
                    let mut r = self.as_mut().rust_mut();
                    r.last_link = Some(made);
                    r.last_modes.clear();
                    r.last_extra = UndoExtra::None;
                    r.last_job = None;
                    r.last_bulk_rename.clear();
                    r.last_trashed_batch.clear();
                }
                self.as_mut().relist_focusing(name.clone());
                self.as_mut().set_status(QString::from(&format!(
                    "Made a link: {}  ·  Ctrl+Z removes it",
                    name.to_string_lossy()
                )));
                QString::default()
            }
            Err(e) => QString::from(&e),
        }
    }

    fn set_mode(mut self: Pin<&mut Self>, spec: &QString) -> QString {
        let spec = match gust_ops::attrs::ModeSpec::parse(&spec.to_string()) {
            Ok(s) => s,
            Err(e) => return QString::from(&e),
        };
        let targets = self.as_ref().target_paths();
        if targets.is_empty() {
            return QString::from("nothing to change");
        }
        let out = gust_ops::attrs::set_modes(&targets, spec);
        let n = out.changed.len();
        if n > 0 {
            let mut r = self.as_mut().rust_mut();
            r.last_modes = out.changed;
            r.last_extra = UndoExtra::None;
            r.last_link = None;
            r.last_job = None;
            r.last_bulk_rename.clear();
            r.last_trashed_batch.clear();
        }
        self.as_mut().reload();
        if !out.failed.is_empty() && n == 0 {
            return QString::from(&out.failed.join("; "));
        }
        let mut msg = format!(
            "Permissions changed on {n} item{}",
            if n == 1 { "" } else { "s" }
        );
        if out.unchanged > 0 {
            msg.push_str(&format!(", {} already had them", out.unchanged));
        }
        if !out.failed.is_empty() {
            msg.push_str(&format!(
                "; {} not changed: {}",
                out.failed.len(),
                out.failed.join("; ")
            ));
        }
        if n > 0 {
            msg.push_str("  ·  Ctrl+Z puts them back");
        }
        self.as_mut().set_status(QString::from(&msg));
        QString::default()
    }

    fn mode_text(&self) -> QString {
        use std::os::unix::fs::MetadataExt;
        let Some(p) = self.target_paths().into_iter().next() else {
            return QString::default();
        };
        match p.symlink_metadata() {
            Ok(m) => {
                let mode = m.mode() & 0o777;
                QString::from(&format!(
                    "{}  {:03o}",
                    gust_ops::attrs::rwx_string(mode),
                    mode
                ))
            }
            Err(_) => QString::default(),
        }
    }

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
        self.sharing_work.cancel.cancel();
        if let Some(job) = self.job.as_ref() {
            job.cancel();
        }
    }

    fn poll_job(mut self: Pin<&mut Self>) -> QString {
        let Some(snap) = self.as_ref().job.as_ref().map(|j| j.snapshot()) else {
            return QString::default();
        };

        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let stopping = snap.state == gust_ops::journal::JobState::Running
            && self
                .as_ref()
                .job
                .as_ref()
                .is_some_and(|j| j.cancel_requested());
        let paused = self.as_ref().job.as_ref().is_some_and(|j| j.is_paused());
        // Speed and time left, from the bytes seen on successive polls.
        let (speed, eta) = {
            let mut r = self.as_mut().rust_mut();
            let start = *r.job_clock.get_or_insert_with(std::time::Instant::now);
            r.job_rate
                .observe(start.elapsed().as_secs_f64(), snap.bytes_done);
            let running =
                snap.state == gust_ops::journal::JobState::Running && !stopping && !paused;
            let speed = r
                .job_rate
                .bytes_per_sec()
                .filter(|_| running)
                .map(gust_ops::rate::format_rate)
                .unwrap_or_default();
            let eta = r
                .job_rate
                .eta_secs(snap.bytes_total.saturating_sub(snap.bytes_done))
                .filter(|_| running && snap.bytes_total > 0)
                .map(gust_ops::rate::format_eta)
                .unwrap_or_default();
            (speed, eta)
        };
        let json = format!(
            concat!(
                "{{\"id\":\"{}\",\"state\":\"{:?}\",\"done\":{},\"total\":{},",
                "\"bytes_done\":{},\"bytes_total\":{},\"message\":\"{}\",\"refused\":{},",
                "\"bytes_text\":\"{}\",\"speed\":\"{}\",\"eta\":\"{}\",\"stopping\":{},\"paused\":{}}}"
            ),
            esc(&snap.id),
            snap.state,
            snap.entries_done,
            snap.entries_total,
            snap.bytes_done,
            snap.bytes_total,
            esc(&snap.message),
            snap.refused.len(),
            if snap.bytes_total > 0 {
                format!(
                    "{} of {}",
                    human_size(snap.bytes_done),
                    human_size(snap.bytes_total)
                )
            } else {
                String::new()
            },
            speed,
            eta,
            stopping,
            paused,
        );

        if snap.is_finished() || snap.state == gust_ops::journal::JobState::Conflict {
            // Keep the record of a CLEANLY finished job so Ctrl+Z can ask for
            // its inverse. A cancelled or failed job is not offered an undo:
            // half an operation has no well-defined opposite.
            let record = self.as_ref().job.as_ref().map(|j| j.record());
            // Anything this job wrote inside a cloud mount is in the local
            // VFS cache, NOT uploaded. Record it before the handle is
            // dropped, so a later disconnect cannot discard it silently.
            if let Some(rec) = record.as_ref() {
                Self::track_cloud_writes(rec);
                self.as_mut().invalidate_job_dir_sizes(rec);
            }
            {
                let mut r = self.as_mut().rust_mut();
                r.job = None;
                r.job_rate = gust_ops::rate::RateMeter::new();
                r.job_clock = None;
                r.last_job = match record {
                    Some(rec) if rec.offers_undo() => Some(rec),
                    _ => None,
                };
                if r.last_job.is_some() {
                    r.last_link = None;
                    r.last_modes.clear();
                    r.last_extra = UndoExtra::None;
                }
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
            ROLE_TYPE_TEXT => QVariant::from(&QString::from(&gust_core::entry::kind_text(e))),
            ROLE_SIZE => {
                let s = match e.meta {
                    Some(m) if e.kind != EntryKind::Dir => human_size(m.size),
                    // A folder shows its contents' total once measured; "≥"
                    // when something inside could not be read or counted.
                    Some(_) => match self.dir_sizes.get(&e.name) {
                        Some(d) if d.complete => human_size(d.bytes),
                        Some(d) => format!("≥ {}", human_size(d.bytes)),
                        None => String::new(),
                    },
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
            // "copy" / "cut" when Ctrl+C / Ctrl+X marked this row (Flea #169).
            ROLE_CLIP => {
                let marked = !self.clip_set.is_empty()
                    && self
                        .key
                        .as_path()
                        .is_some_and(|d| self.clip_set.contains(&d.join(&e.name)));
                let s = match (&self.clipboard, marked) {
                    (Some((_, ClipMode::Cut)), true) => "cut",
                    (Some((_, ClipMode::Copy)), true) => "copy",
                    _ => "",
                };
                QVariant::from(&QString::from(s))
            }
            // What the entry looks like, for the icon theme to resolve
            // (DESIGN §6.21). Pure: name, kind and details already read.
            ROLE_ICON_KEY => QVariant::from(&QString::from(&self.icon_key_of(e))),
            ROLE_EMBLEMS => QVariant::from(&QString::from(
                &gust_core::icons::emblems_for(e.kind, e.meta.as_ref()).join(","),
            )),
            ROLE_TAGS => QVariant::from(&QString::from(
                &gust_core::tags::keys_of(e.meta.map(|m| m.tags).unwrap_or(0)).join(","),
            )),
            ROLE_THUMB => {
                // The complete versioned URL was validated on a worker.
                // This role performs no I/O.
                let url = self
                    .thumbs
                    .get(&e.name)
                    .map(QString::from)
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
        h.insert(ROLE_TYPE_TEXT, QByteArray::from("typeText"));
        h.insert(ROLE_CLIP, QByteArray::from("clipMark"));
        h.insert(ROLE_ICON_KEY, QByteArray::from("iconKey"));
        h.insert(ROLE_EMBLEMS, QByteArray::from("emblems"));
        h.insert(ROLE_TAGS, QByteArray::from("tagKeys"));
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
    revision: i32,
    icons_rev: i32,
    /// File families and special folders, read on a worker.
    icons: gust_core::icons::Looks,
    /// Newest icon load asked for; an older one finishing later is dropped.
    icons_gen: u64,
    tokens: Tokens,
    /// Watches on the Omarchy state folders; dropped with the object.
    watches: Vec<gust_core::watch::DirWatch>,
}

impl Default for ThemeTokensRust {
    fn default() -> Self {
        let t = gust_core::theme::load_current();
        ThemeTokensRust {
            name: QString::from(&t.name),
            is_light: t.is_light(),
            revision: 0,
            icons_rev: 0,
            icons: gust_core::icons::Looks::default(),
            icons_gen: 0,
            tokens: t,
            watches: Vec::new(),
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
        self.as_mut().install(t);
        // Icons are drawn in theme colours, so a theme switch needs no
        // reload here: the colour bindings follow `revision`.
    }

    fn icon_look(&self, key: &QString) -> QString {
        QString::from(&self.icons.look(&key.to_string()).wire())
    }

    /// Read the MIME database and user folders on a worker, then publish
    /// them on the GUI thread (DESIGN §6.21).
    fn load_icons(mut self: Pin<&mut Self>) {
        let gen = self.as_ref().icons_gen + 1;
        self.as_mut().rust_mut().icons_gen = gen;
        let thread = <qobject::ThemeTokens as cxx_qt::Threading>::qt_thread(&self.as_ref());
        std::thread::Builder::new()
            .name("gust-icons".into())
            .spawn(move || {
                let set = gust_core::icons::Looks::load_current();
                let _ = thread.queue(move |mut tokens| {
                    if tokens.as_ref().icons_gen != gen {
                        return;
                    }
                    tokens.as_mut().rust_mut().icons = set;
                    let rev = tokens.as_ref().icons_rev.wrapping_add(1);
                    tokens.as_mut().set_icons_rev(rev);
                });
            })
            .ok();
    }

    fn load_theme_dir(self: Pin<&mut Self>, dir: &QString, name: &QString) {
        let t = gust_core::theme::load_from_theme_dir(
            std::path::Path::new(&dir.to_string()),
            &name.to_string(),
        );
        self.install(t);
    }

    /// Adopt a token set. Only a real change repaints: the state folder also
    /// holds unrelated files (clipboard history, notifications) whose writes
    /// wake the watch.
    fn install(mut self: Pin<&mut Self>, t: Tokens) {
        if t == self.as_ref().tokens {
            return;
        }
        let name = QString::from(&t.name);
        let light = t.is_light();
        self.as_mut().rust_mut().tokens = t;
        self.as_mut().set_name(name);
        self.as_mut().set_is_light(light);
        let rev = self.as_ref().revision.wrapping_add(1);
        self.as_mut().set_revision(rev);
    }

    fn watch_theme(mut self: Pin<&mut Self>) {
        // File looks are wanted whether or not there is an Omarchy theme.
        if self.as_ref().icons_gen == 0 {
            self.as_mut().load_icons();
        }
        let Some(state) = gust_core::theme::default_state_dir() else {
            return;
        };
        let thread = <qobject::ThemeTokens as cxx_qt::Threading>::qt_thread(&self.as_ref());
        let mut watches = Vec::new();
        for dir in gust_core::theme::theme_watch_dirs(&state) {
            let thread = thread.clone();
            let state = state.clone();
            let w = gust_core::watch::DirWatch::start(&dir, move |change| {
                let state = state.clone();
                let _ = thread.queue(move |mut tokens| match change {
                    // Mid-switch there is no theme at all: wait for the move
                    // that completes it rather than flash the fallback.
                    gust_core::watch::Change::Contents => {
                        if gust_core::theme::theme_ready(&state) {
                            tokens.as_mut().reload();
                        }
                    }
                    // A watched folder was replaced: watch the new one.
                    gust_core::watch::Change::Gone => tokens.as_mut().watch_theme(),
                });
            });
            if let Ok(w) = w {
                watches.push(w);
            }
        }
        self.as_mut().rust_mut().watches = watches;
    }
}

/// Which entries are shown, in order, for a filter and the hidden-files
/// setting. O(n): this used to build a `Vec` of kept pointers and call
/// `contains` for every entry, which is O(n²) — a 100k directory spent
/// minutes here. Index arithmetic keeps it linear. Pure — no I/O.
fn visible_indices(all: &[Entry], needle: &str, show_hidden: bool) -> Vec<usize> {
    if needle.is_empty() {
        return all
            .iter()
            .enumerate()
            .filter(|(_, e)| show_hidden || !e.is_hidden())
            .map(|(i, _)| i)
            .collect();
    }
    // `filter_entries` returns borrows into `all`; turn each one back into
    // its index by pointer offset rather than searching for it.
    let base = all.as_ptr();
    let mut idx: Vec<usize> = filter_entries(all, needle)
        .into_iter()
        .filter(|e| show_hidden || !e.is_hidden())
        .map(|e| {
            // Safety: every element is a reference into `all`, so the
            // offset is in bounds and well defined.
            unsafe { (e as *const Entry).offset_from(base) as usize }
        })
        .collect();
    idx.sort_unstable();
    idx
}

/// Plain words for a folder that could not be listed. `error` is the
/// io::Error text, e.g. "Permission denied (os error 13)".
fn empty_reason_for(error: &str) -> String {
    let e = error.split(" (os error").next().unwrap_or(error);
    match e {
        "No such file or directory" => "This folder no longer exists".into(),
        "Permission denied" => "You don't have permission to open this folder".into(),
        _ => format!("Can't open this folder: {e}"),
    }
}

#[cfg(test)]
mod empty_reason_tests {
    use super::empty_reason_for;

    #[test]
    fn a_folder_that_could_not_be_read_never_claims_to_be_empty() {
        assert_eq!(
            empty_reason_for("No such file or directory (os error 2)"),
            "This folder no longer exists"
        );
        assert_eq!(
            empty_reason_for("Permission denied (os error 13)"),
            "You don't have permission to open this folder"
        );
        assert_eq!(
            empty_reason_for("Input/output error (os error 5)"),
            "Can't open this folder: Input/output error"
        );
    }
}

/// The copy is held by a `gust --clip-serve` helper so it outlives the
/// window; in-process only if this executable cannot be found.
fn clip_holder() -> gust_core::clipboard::Holder {
    match std::env::current_exe() {
        Ok(exe) => gust_core::clipboard::Holder::Helper(exe),
        Err(_) => gust_core::clipboard::Holder::InProcess,
    }
}

/// The detached child waiting to report that its window is up.
static DETACH_PENDING: std::sync::Mutex<
    Option<(std::process::Child, gust_core::handoff::ReadyWait)>,
> = std::sync::Mutex::new(None);

/// The view a detached window should start in (from its handoff).
static HANDOFF_VIEW: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// `YYYY-MM-DDThh:mm:ss` (UTC) to Unix seconds.
fn parse_iso_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    // days_from_civil (H. Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod iso_tests {
    #[test]
    fn iso_round_trips_through_full_time() {
        let t = super::parse_iso_utc("2021-03-04T05:06:07").unwrap();
        assert_eq!(gust_core::properties::full_time(t, 0), "2021-03-04 05:06");
        assert_eq!(super::parse_iso_utc("1970-01-01T00:00:00"), Some(0));
        assert_eq!(super::parse_iso_utc("junk"), None);
    }
}

/// "1 folder with 1,204 files · 3.2 GB" — what a delete really removes.
fn delete_total_sentence(folders: u64, files: u64, bytes: u64, complete: bool) -> String {
    fn grouped(n: u64) -> String {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        out
    }
    let plural = |n: u64, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            format!("{} {many}", grouped(n))
        }
    };
    let at_least = if complete { "" } else { "at least " };
    let files_s = if files == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", grouped(files))
    };
    let head = match folders {
        0 => format!("{at_least}{files_s}"),
        f => format!(
            "{} with {at_least}{files_s}",
            plural(f, "1 folder", "folders")
        ),
    };
    format!("{head} · {}", human_size(bytes))
}

#[cfg(test)]
mod delete_total_tests {
    use super::delete_total_sentence as s;
    #[test]
    fn says_what_is_really_inside() {
        assert_eq!(
            s(1, 1204, 3_435_973_837, true),
            "1 folder with 1,204 files · 3.2 GB"
        );
        assert_eq!(s(0, 1, 10, true), "1 file · 10 B");
        assert_eq!(s(2, 0, 0, true), "2 folders with 0 files · 0 B");
        assert_eq!(
            s(1, 500_000, 1024, false),
            "1 folder with at least 500,000 files · 1.0 KB"
        );
    }
}
