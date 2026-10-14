use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

fn main() {
    // QML lives in the repo's ui/ directory (AGENTS.md). `crates/app/ui` is a
    // symlink to it so the paths handed to cxx-qt-build stay INSIDE the crate:
    // the path string becomes the resource alias verbatim, and a "../../ui/x"
    // alias would resolve to a bogus qrc URL that fails to load at runtime.
    let views = [
        "ui/Main.qml",
        "ui/Pane.qml",
        "ui/QuickLook.qml",
        "ui/RowsView.qml",
        "ui/IconGridView.qml",
        "ui/ColumnsView.qml",
        "ui/Sidebar.qml",
        "ui/Breadcrumbs.qml",
        "ui/HintStrip.qml",
        "ui/CommandPalette.qml",
        "ui/ConfirmDialog.qml",
        "ui/ScrollIndicator.qml",
        "ui/TextPrompt.qml",
        "ui/JobDrawer.qml",
        "ui/ContextMenu.qml",
        "ui/ConflictDialog.qml",
        "ui/InterruptedNotice.qml",
        "ui/SettingsPanel.qml",
        "ui/ChooserBar.qml",
        "ui/TrashPanel.qml",
        "ui/RecentPanel.qml",
        "ui/KeyMapPanel.qml",
        "ui/RubberBand.qml",
        "ui/WheelScroll.qml",
        "ui/FileIcon.qml",
        "ui/Emblem.qml",
        "ui/DragTarget.qml",
        "ui/ConnectPanel.qml",
        "ui/RenameCard.qml",
    ];

    let mut module = QmlModule::new("com.gust.app").qml_files(views);
    // Tokens is a singleton: one theme source for the whole UI.
    // QmlFile is constructed via From<AsRef<Path>>, not an inherent `new`.
    module = module.qml_file(QmlFile::from("ui/Tokens.qml").singleton(true));

    CxxQtBuilder::new_qml_module(module)
        .files(["src/bridge.rs"])
        .qt_module("Quick")
        .qt_module("Gui")
        .build();
}
