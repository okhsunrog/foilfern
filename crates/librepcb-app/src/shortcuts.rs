//! Keyboard shortcuts: the editor commands with their default shortcuts
//! and the user's overrides from the workspace settings.
//!
//! Port of the shortcut handling of libs/librepcb/editor/editorcommand.cpp
//! (`EditorCommand::getKeySequences()` with the overrides of
//! `WorkspaceSettings::keyboardShortcuts`) and of the data of
//! libs/librepcb/editor/modelview/keyboardshortcutsmodel.{h,cpp}.
//!
//! The commands are read from the generated `ui/api/editorcommandset.slint`
//! (the `.slint` command set is what the UI uses, with one default shortcut
//! per command; upstream's C++ command set can have several and groups the
//! commands into categories, which are not available here). Overrides are
//! key sequences in Qt's portable text format (`"Ctrl+Shift+S"`); only the
//! first chord of multi-chord sequences is used. [`is_shortcut()`] (the
//! `Backend.is-shortcut` callback) compares key events against the
//! overrides of a command if there are any, else against its default.

use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, PoisonError, RwLock};

use librepcb_app_ui as ui;
use slint::platform::Key;

/// The translation context of the command texts.
const CTX: &str = "EditorCommandSet";

/// An editor command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    /// Identifier (e.g. `"save"`), the key of the workspace settings.
    pub id: String,
    /// Text (untranslated).
    pub text: String,
    /// Status tip (untranslated).
    pub status_tip: String,
    /// The default shortcut (Qt portable text, empty: none).
    pub default_shortcut: String,
}

impl CommandInfo {
    /// The translated text without mnemonic and ellipsis.
    pub fn text_tr(&self) -> String {
        librepcb_i18n::translate(CTX, &self.text)
            .replace('&', "")
            .trim_end_matches("...")
            .to_owned()
    }
}

/// All editor commands of the `.slint` command set, in their order.
pub fn commands() -> &'static [CommandInfo] {
    static COMMANDS: OnceLock<Vec<CommandInfo>> = OnceLock::new();
    COMMANDS.get_or_init(|| parse_command_set(include_str!("../ui/api/editorcommandset.slint")))
}

/// Reads the commands of the generated `editorcommandset.slint`.
fn parse_command_set(source: &str) -> Vec<CommandInfo> {
    let value = |block: &str, key: &str| -> String {
        block
            .lines()
            .find_map(|l| l.trim().strip_prefix(&format!("{key}: \"")))
            .and_then(|rest| rest.strip_suffix("\","))
            .map(|v| v.replace("\\\"", "\"").replace("\\\\", "\\"))
            .unwrap_or_default()
    };
    source
        .split("in property <EditorCommand>")
        .skip(1)
        .map(|block| CommandInfo {
            id: value(block, "id"),
            text: value(block, "text"),
            status_tip: value(block, "status-tip"),
            default_shortcut: value(block, "shortcut"),
        })
        .filter(|c| !c.id.is_empty())
        .collect()
}

/// A key combination (the first chord of a Qt key sequence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombination {
    /// Control modifier.
    pub control: bool,
    /// Alt modifier.
    pub alt: bool,
    /// Shift modifier.
    pub shift: bool,
    /// Meta modifier.
    pub meta: bool,
    /// The key as Slint reports it in `KeyEvent::text`.
    pub key: String,
}

/// Converts a Qt key name to the text of Slint key events.
fn key_text(name: &str) -> Option<String> {
    let special = match name.to_ascii_lowercase().as_str() {
        "return" => Some(Key::Return),
        "enter" => Some(Key::Return),
        "esc" | "escape" => Some(Key::Escape),
        "tab" => Some(Key::Tab),
        "backtab" => Some(Key::Backtab),
        "backspace" => Some(Key::Backspace),
        "del" | "delete" => Some(Key::Delete),
        "ins" | "insert" => Some(Key::Insert),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pgup" | "pageup" => Some(Key::PageUp),
        "pgdown" | "pagedown" => Some(Key::PageDown),
        "left" => Some(Key::LeftArrow),
        "right" => Some(Key::RightArrow),
        "up" => Some(Key::UpArrow),
        "down" => Some(Key::DownArrow),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        _ => None,
    };
    if let Some(key) = special {
        return Some(slint::SharedString::from(key).to_string());
    }
    let text = match name.to_ascii_lowercase().as_str() {
        "space" => " ".to_owned(),
        "period" => ".".to_owned(),
        "comma" => ",".to_owned(),
        "plus" => "+".to_owned(),
        "minus" => "-".to_owned(),
        "slash" => "/".to_owned(),
        "asterisk" => "*".to_owned(),
        _ if name.chars().count() == 1 => name.to_lowercase(),
        _ => return None,
    };
    Some(text)
}

/// Parses a key sequence in Qt's portable text format (e.g.
/// `"Ctrl+Shift+S"`, `"Ctrl++"`, `"F5"`); only the first chord is used.
/// `None` if it is empty or invalid.
pub fn parse_key_sequence(text: &str) -> Option<KeyCombination> {
    let chord = text.split(',').next().unwrap_or("").trim();
    // A comma as key: "Ctrl+,".
    let chord = if chord.ends_with('+') && text.trim().ends_with(",") {
        text.trim()
    } else {
        chord
    };
    if chord.is_empty() {
        return None;
    }
    let (mods, key) = if chord.len() > 1 && chord.ends_with("++") {
        (&chord[..chord.len() - 2], "+")
    } else if chord == "+" {
        ("", "+")
    } else {
        match chord.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", chord),
        }
    };
    let mut combination = KeyCombination {
        control: false,
        alt: false,
        shift: false,
        meta: false,
        key: key_text(key.trim())?,
    };
    for m in mods.split('+').map(str::trim).filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" => combination.control = true,
            "alt" => combination.alt = true,
            "shift" => combination.shift = true,
            "meta" => combination.meta = true,
            _ => return None,
        }
    }
    Some(combination)
}

fn overrides() -> &'static RwLock<HashMap<String, Vec<KeyCombination>>> {
    static OVERRIDES: OnceLock<RwLock<HashMap<String, Vec<KeyCombination>>>> = OnceLock::new();
    OVERRIDES.get_or_init(RwLock::default)
}

/// Sets the overrides of the workspace settings (command identifier → key
/// sequences; an empty list disables the shortcut).
pub fn set_overrides(map: &BTreeMap<String, Vec<String>>) {
    let parsed = map
        .iter()
        .map(|(id, seqs)| {
            (
                id.clone(),
                seqs.iter().filter_map(|s| parse_key_sequence(s)).collect(),
            )
        })
        .collect();
    *overrides().write().unwrap_or_else(PoisonError::into_inner) = parsed;
}

/// Whether a key event matches a key combination. Letters are compared
/// case-insensitively since Slint reports the shifted character; Shift is
/// part of the text for symbols (e.g. "+"), so it is only compared for
/// letters and special keys.
fn matches(event: &slint::language::KeyEvent, c: &KeyCombination) -> bool {
    let m = &event.modifiers;
    let modifiers_match = m.control == c.control && m.alt == c.alt && m.meta == c.meta;
    let text_matches = event.text.as_str() == c.key
        || (event.text.chars().count() == 1 && event.text.to_lowercase() == c.key.to_lowercase());
    let shift_matches = m.shift == c.shift
        || !event
            .text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphabetic());
    modifiers_match && text_matches && shift_matches
}

/// `Backend.is-shortcut`: whether a key event matches the shortcut of an
/// editor command (the user's overrides if there are any, else the
/// default of the command set).
pub fn is_shortcut(event: &slint::language::KeyEvent, command: &ui::EditorCommand) -> bool {
    if let Some(list) = overrides()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(command.id.as_str())
    {
        return list.iter().any(|c| matches(event, c));
    }
    if command.key.is_empty() {
        return false;
    }
    let c = &command.modifiers;
    matches(
        event,
        &KeyCombination {
            control: c.control,
            alt: c.alt,
            shift: c.shift,
            meta: c.meta,
            key: command.key.to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_set() {
        let cmds = commands();
        assert!(cmds.len() > 150);
        let save = cmds.iter().find(|c| c.id == "save").unwrap();
        assert_eq!(save.text, "Save");
        assert_eq!(save.default_shortcut, "Ctrl+S");
    }

    #[test]
    fn key_sequences() {
        let c = parse_key_sequence("Ctrl+Shift+S").unwrap();
        assert!(c.control && c.shift && !c.alt);
        assert_eq!(c.key, "s");
        assert_eq!(parse_key_sequence("Ctrl++").unwrap().key, "+");
        assert_eq!(parse_key_sequence("+").unwrap().key, "+");
        let f5 = parse_key_sequence("F5").unwrap();
        assert_eq!(f5.key, slint::SharedString::from(Key::F5).as_str());
        assert!(parse_key_sequence("").is_none());
        assert!(parse_key_sequence("Hyper+X").is_none());
        assert_eq!(parse_key_sequence("Ctrl+K, Ctrl+C").unwrap().key, "k");
        assert_eq!(parse_key_sequence("Ctrl+,").unwrap().key, ",");
    }

    #[test]
    fn overrides_are_honored() {
        let mut event = slint::language::KeyEvent::default();
        event.text = "s".into();
        event.modifiers.control = true;
        let command = ui::EditorCommand {
            id: "test_command_x".into(),
            key: "s".into(),
            modifiers: event.modifiers,
            ..Default::default()
        };
        assert!(is_shortcut(&event, &command));
        let mut map = BTreeMap::new();
        map.insert("test_command_x".to_owned(), vec!["Ctrl+Shift+X".to_owned()]);
        set_overrides(&map);
        assert!(!is_shortcut(&event, &command));
        event.text = "X".into();
        event.modifiers.shift = true;
        assert!(is_shortcut(&event, &command));
        // An empty list disables the shortcut.
        map.insert("test_command_x".to_owned(), Vec::new());
        set_overrides(&map);
        assert!(!is_shortcut(&event, &command));
        set_overrides(&BTreeMap::new());
    }
}
