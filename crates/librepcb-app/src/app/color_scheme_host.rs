//! Shows the color scheme editor (`ColorSchemeEditor` global of
//! `ui/colorschemedialog.slint`) for a user defined color scheme of the
//! open workspace settings dialog.
//!
//! Port of `WorkspaceSettingsDialog::execColorSchemeDialog()` and of
//! libs/librepcb/editor/modelview/colorschememodel.{h,cpp}: the rows are the
//! roles of the scheme's base with the (overridden) primary and secondary
//! colors; the UI writes HSV or hex colors, or "restore default", into the
//! rows. Upstream opens a separate window; here it is an overlay of the main
//! window above the settings dialog.

use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::types::Uuid;
use slint::ComponentHandle;

use super::{State, deferred};
use crate::color_schemes::{self, ColorSchemes, RoleColors, SchemeKind};
use crate::dialogs::workspace_settings::WorkspaceSettingsDialog;
use crate::models::{UiModel, model_rc};

/// The scheme being edited.
pub struct ColorSchemeEdit {
    kind: SchemeKind,
    uuid: Uuid,
    roles: Vec<String>,
    model: Rc<UiModel<ui::ColorSchemeItemData>>,
}

fn color_data(color: u32, overridden: bool) -> ui::ColorSchemeColorData {
    let (hue, saturation, value, alpha) = color_schemes::to_hsva(color);
    ui::ColorSchemeColorData {
        hue,
        saturation,
        value,
        alpha,
        hex: color_schemes::format_rgba(color).into(),
        overridden,
        action: ui::ColorSchemeColorAction::None,
    }
}

fn row(r: &RoleColors) -> ui::ColorSchemeItemData {
    ui::ColorSchemeItemData {
        name: r.name.as_str().into(),
        primary: color_data(r.primary, r.primary_overridden),
        secondary: color_data(r.secondary, r.secondary_overridden),
    }
}

/// The color written by the UI: `Some(Some(c))` to set, `Some(None)` to
/// restore the default, `None` for no change (upstream `toColor()`).
fn written_color(d: &ui::ColorSchemeColorData) -> Option<Option<u32>> {
    match d.action {
        ui::ColorSchemeColorAction::SetHsv => Some(Some(color_schemes::from_hsva(
            d.hue,
            d.saturation,
            d.value,
            d.alpha,
        ))),
        ui::ColorSchemeColorAction::SetHex => color_schemes::parse_rgba(&d.hex).map(Some),
        ui::ColorSchemeColorAction::RestoreDefault => Some(None),
        _ => None,
    }
}

/// Binds the callbacks of the `ColorSchemeEditor` global.
pub fn bind(window: &ui::AppWindow, weak: &std::rc::Weak<std::cell::RefCell<State>>) {
    let e = window.global::<ui::ColorSchemeEditor>();
    let w = weak.clone();
    e.on_close_requested(move || deferred(&w, State::close_color_scheme_editor));
    let w = weak.clone();
    e.on_copy_to_clipboard(move || {
        let text = w.upgrade().and_then(|s| {
            s.try_borrow_mut()
                .ok()
                .and_then(|mut s| s.color_scheme_text())
        });
        match text {
            Some(text) => crate::clipboard::with_clipboard(|c| {
                use librepcb_editor::fsm::Clipboard as _;
                crate::clipboard::ensure_opened(c);
                c.set("text/plain", text.into_bytes());
                true
            }),
            None => false,
        }
    });
}

impl State {
    fn settings_dialog(&mut self) -> Option<&mut WorkspaceSettingsDialog> {
        self.form_dialog
            .as_mut()?
            .dialog
            .as_any_mut()?
            .downcast_mut::<WorkspaceSettingsDialog>()
    }

    fn edited_schemes(&mut self) -> Option<(&mut ColorSchemes, Uuid)> {
        let (kind, uuid) = self.color_scheme_edit.as_ref().map(|e| (e.kind, e.uuid))?;
        Some((self.settings_dialog()?.color_schemes_mut(kind), uuid))
    }

    /// Opens the color scheme editor for the active scheme of a kind of the
    /// open workspace settings dialog (if it is user defined).
    pub fn show_color_scheme_editor(&mut self, kind: SchemeKind) {
        let Some(dialog) = self.settings_dialog() else {
            return;
        };
        let schemes = dialog.color_schemes(kind);
        let uuid = schemes.active();
        if !schemes.is_user(uuid) {
            return;
        }
        let name = schemes.name(uuid);
        let colors = schemes.colors(uuid);
        let model = UiModel::shared(colors.iter().map(row).collect());
        let w = self.this.clone();
        model.set_handler(move |index, data| {
            deferred(&w, move |s| s.color_scheme_row_written(index, &data));
        });
        if let Some(win) = self.window() {
            let e = win.global::<ui::ColorSchemeEditor>();
            e.set_title(librepcb_i18n::tr!("ColorSchemeDialog", "Color Scheme").into());
            e.set_name(name.into());
            e.set_model(model_rc(&model));
            e.set_shown(true);
        }
        self.color_scheme_edit = Some(ColorSchemeEdit {
            kind,
            uuid,
            roles: colors.into_iter().map(|c| c.role).collect(),
            model,
        });
    }

    /// The open color scheme editor's scheme (tests): kind and UUID.
    pub fn color_scheme_editor(&self) -> Option<(SchemeKind, Uuid)> {
        self.color_scheme_edit.as_ref().map(|e| (e.kind, e.uuid))
    }

    fn color_scheme_row_written(&mut self, index: usize, data: &ui::ColorSchemeItemData) {
        let Some(role) = self
            .color_scheme_edit
            .as_ref()
            .and_then(|e| e.roles.get(index).cloned())
        else {
            return;
        };
        let Some((schemes, uuid)) = self.edited_schemes() else {
            return;
        };
        if let Some(color) = written_color(&data.primary) {
            schemes.set_color(uuid, &role, false, color);
        }
        if let Some(color) = written_color(&data.secondary) {
            schemes.set_color(uuid, &role, true, color);
        }
        let updated = schemes.colors(uuid).get(index).map(row);
        if let (Some(updated), Some(edit)) = (updated, &self.color_scheme_edit) {
            edit.model.set(index, updated);
        }
    }

    /// Upstream `applyChanges` of the color scheme dialog: renames the
    /// scheme and closes the editor.
    pub fn close_color_scheme_editor(&mut self) {
        let name = self
            .window()
            .map(|w| {
                w.global::<ui::ColorSchemeEditor>()
                    .get_name()
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        if !name.is_empty()
            && let Some((schemes, uuid)) = self.edited_schemes()
        {
            schemes.rename(uuid, &name);
        }
        if let Some(dialog) = self.settings_dialog() {
            dialog.update_color_schemes();
        }
        self.color_scheme_edit = None;
        if let Some(w) = self.window() {
            let e = w.global::<ui::ColorSchemeEditor>();
            e.set_shown(false);
            e.set_model(crate::models::vec_model(Vec::new()));
        }
        self.refresh_form_dialog(false);
    }

    /// Upstream "Copy all Colors Into Clipboard": `"<primary>",
    /// "<secondary>", <role>` per role.
    fn color_scheme_text(&mut self) -> Option<String> {
        let (schemes, uuid) = self.edited_schemes()?;
        let lines: Vec<String> = schemes
            .colors(uuid)
            .iter()
            .map(|r| {
                format!(
                    "\"{}\", \"{}\", {}",
                    color_schemes::format_argb(r.primary),
                    color_schemes::format_argb(r.secondary),
                    r.role
                )
            })
            .collect();
        let max = lines.iter().map(String::len).max().unwrap_or(0);
        Some(
            lines
                .into_iter()
                .map(|l| format!("{l:max$}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}
