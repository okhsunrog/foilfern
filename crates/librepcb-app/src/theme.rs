//! UI themes.
//!
//! Port of libs/librepcb/core/workspace/uitheme.{h,cpp} (without the
//! `QPalette`, which only the remaining Qt widgets use) and the conversion
//! to the Slint `Theme` struct (libs/librepcb/editor/utils/uihelpers.cpp).
//! The workspace setting `ui_theme` selects a theme by its identifier.

use librepcb_app_ui as ui;
use librepcb_i18n::tr;

use crate::helpers::parse_hex_color;

/// A UI theme: identifier, name and the colors of the Slint `Theme` struct
/// (in its member order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiTheme {
    /// Identifier stored in the workspace settings.
    pub id: &'static str,
    /// Untranslated name (translation context `UiTheme`).
    pub name: &'static str,
    colors: [&'static str; 33],
}

impl UiTheme {
    /// The light theme.
    pub const LIGHT: Self = Self {
        id: "light",
        name: "Light",
        colors: [
            "#f0f0f0", // window
            "#ffffff", // base
            "#d0d0d0", // base-border
            "#9e9e9e", // base-text-disabled
            "#7c7c7c", // base-text-muted
            "#292929", // base-text
            "#000000", // base-text-hovered
            "#0059ff", // base-text-info
            "#008f00", // base-text-success
            "#e24800", // base-text-warning
            "#ff0000", // base-text-error
            "#fafafa", // control
            "#e4e4e4", // control-disabled
            "#e8e8e8", // control-hovered
            "#d0d0d0", // control-checked
            "#c0c0c0", // control-border
            "#dddddd", // control-border-disabled
            "#9e9e9e", // control-text-disabled
            "#7c7c7c", // control-text-muted
            "#000000", // control-text
            "#29d682", // selection
            "#000000", // selection-text
            "#fffbc5", // tooltip
            "#b9b9b9", // tooltip-border
            "#3a3a3a", // tooltip-text
            "#29d682", // accent
            "#000000", // accent-text
            "#00ccff", // info
            "#000000", // info-text
            "#ffe659", // warning
            "#000000", // warning-text
            "#ff3b3b", // error
            "#000000", // error-text
        ],
    };

    /// The dark theme (upstream default).
    pub const DARK: Self = Self {
        id: "dark",
        name: "Dark",
        colors: [
            "#353535", // window
            "#2a2a2a", // base
            "#505050", // base-border
            "#808080", // base-text-disabled
            "#adadad", // base-text-muted
            "#c4c4c4", // base-text
            "#e0e0e0", // base-text-hovered
            "#00ccff", // base-text-info
            "#00ff00", // base-text-success
            "#ffff00", // base-text-warning
            "#ff2020", // base-text-error
            "#303030", // control
            "#1a1a1a", // control-disabled
            "#404040", // control-hovered
            "#505050", // control-checked
            "#606060", // control-border
            "#414141", // control-border-disabled
            "#707070", // control-text-disabled
            "#909090", // control-text-muted
            "#c4c4c4", // control-text
            "#29d682", // selection
            "#000000", // selection-text
            "#fffbc5", // tooltip
            "#535353", // tooltip-border
            "#3a3a3a", // tooltip-text
            "#29d682", // accent
            "#000000", // accent-text
            "#00ccff", // info
            "#000000", // info-text
            "#ffe659", // warning
            "#000000", // warning-text
            "#ff4343", // error
            "#000000", // error-text
        ],
    };

    /// All themes (upstream order).
    pub const ALL: [Self; 2] = [Self::LIGHT, Self::DARK];

    /// The theme with the given identifier.
    pub fn find(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id == id)
    }

    /// The theme selected by the workspace setting, falling back to the
    /// default (dark) theme for unknown identifiers.
    pub fn from_setting(id: &str) -> Self {
        Self::find(id).unwrap_or(Self::DARK)
    }

    /// The translated name.
    pub fn name_tr(&self) -> String {
        tr!("librepcb::UiTheme", self.name)
    }

    /// The next theme (for `Backend.toggle-theme`).
    pub fn next(&self) -> Self {
        let i = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// Converts to the Slint struct.
    pub fn to_ui(&self) -> ui::Theme {
        let c = |i: usize| parse_hex_color(self.colors[i]).unwrap_or_default();
        ui::Theme {
            window: c(0),
            base: c(1),
            base_border: c(2),
            base_text_disabled: c(3),
            base_text_muted: c(4),
            base_text: c(5),
            base_text_hovered: c(6),
            base_text_info: c(7),
            base_text_success: c(8),
            base_text_warning: c(9),
            base_text_error: c(10),
            control: c(11),
            control_disabled: c(12),
            control_hovered: c(13),
            control_checked: c(14),
            control_border: c(15),
            control_border_disabled: c(16),
            control_text_disabled: c(17),
            control_text_muted: c(18),
            control_text: c(19),
            selection: c(20),
            selection_text: c(21),
            tooltip: c(22),
            tooltip_border: c(23),
            tooltip_text: c(24),
            accent: c(25),
            accent_text: c(26),
            info: c(27),
            info_text: c(28),
            warning: c(29),
            warning_text: c(30),
            error: c(31),
            error_text: c(32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes() {
        assert_eq!(UiTheme::from_setting("light"), UiTheme::LIGHT);
        assert_eq!(UiTheme::from_setting("foo"), UiTheme::DARK);
        assert_eq!(UiTheme::DARK.next(), UiTheme::LIGHT);
        for theme in UiTheme::ALL {
            assert!(theme.colors.iter().all(|c| parse_hex_color(c).is_some()));
        }
        let t = UiTheme::DARK.to_ui();
        assert_eq!(t.accent, slint::Color::from_rgb_u8(0x29, 0xd6, 0x82));
    }
}
