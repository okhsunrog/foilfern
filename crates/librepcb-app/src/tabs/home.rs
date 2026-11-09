//! The home tab.
//!
//! Port of libs/librepcb/editor/hometab.{h,cpp}. The home tab has no state
//! of its own: its content (workspace, recent projects, folder tree) comes
//! from the `Data` globals.

use librepcb_app_ui as ui;

/// The home tab's `TabData`.
pub fn ui_data() -> ui::TabData {
    ui::TabData {
        r#type: ui::TabType::Home,
        ..Default::default()
    }
}
