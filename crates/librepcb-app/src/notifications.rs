//! Notifications shown in the notifications popup of the status bar.
//!
//! Port of libs/librepcb/editor/notificationsmodel.{h,cpp} and
//! libs/librepcb/editor/notification.{h,cpp}: notifications are sorted by
//! unread state and severity, can be dismissed, and those with a dismiss key
//! can be suppressed permanently ("don't show again", stored in the
//! workspace setting `dismissed_messages`).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use librepcb_app_ui as ui;

use crate::models::UiModel;

/// A notification to push.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// Severity (or progress).
    pub kind: ui::NotificationType,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Key for "don't show again" (empty: not supported).
    pub dismiss_key: String,
    /// Whether the popup opens automatically.
    pub auto_popup: bool,
    /// The button of the notification (text and action), if any.
    pub button: Option<NotificationButton>,
}

/// The button of a notification (upstream `Notification::buttonClicked()`).
#[derive(Clone)]
pub struct NotificationButton {
    /// The (translated) text.
    pub text: String,
    /// Called (deferred) when the button is clicked.
    pub action: Rc<dyn Fn()>,
}

impl std::fmt::Debug for NotificationButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationButton")
            .field("text", &self.text)
            .finish_non_exhaustive()
    }
}

impl PartialEq for NotificationButton {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && Rc::ptr_eq(&self.action, &other.action)
    }
}

impl Notification {
    /// A notification without "don't show again" support.
    pub fn new(
        kind: ui::NotificationType,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            title: title.into(),
            description: description.into(),
            dismiss_key: String::new(),
            auto_popup: false,
            button: None,
        }
    }

    fn to_ui(&self) -> ui::NotificationData {
        ui::NotificationData {
            r#type: self.kind,
            title: self.title.as_str().into(),
            description: self.description.as_str().into(),
            button_text: self
                .button
                .as_ref()
                .map(|b| b.text.as_str().into())
                .unwrap_or_default(),
            progress: 0,
            supports_dont_show_again: !self.dismiss_key.is_empty(),
            unread: true,
            button_clicked: false,
            dismissed: false,
            dont_show_again: false,
        }
    }
}

/// Severity rank for sorting (the enum is ordered by severity).
fn severity(t: ui::NotificationType) -> u8 {
    match t {
        ui::NotificationType::Progress => 0,
        ui::NotificationType::Tip => 1,
        ui::NotificationType::Info => 2,
        ui::NotificationType::Warning => 3,
        ui::NotificationType::Critical => 4,
    }
}

/// The identifier of a pushed notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationId(u64);

/// Summary for the `Data` globals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotificationsState {
    /// Unread notifications (without progress notifications).
    pub unread: i32,
    /// Index of the first progress notification, -1 if none.
    pub progress_index: i32,
}

/// Callback with a dismiss key.
type KeyFn = Rc<dyn Fn(&str)>;

/// The notifications model (`Data.notifications`).
pub struct Notifications {
    items: Vec<Notification>,
    /// Identifiers of the items (same order).
    ids: Vec<NotificationId>,
    next_id: u64,
    model: Rc<UiModel<ui::NotificationData>>,
    dismissed_keys: Vec<String>,
    on_changed: Option<Rc<dyn Fn(NotificationsState, bool)>>,
    /// Keys the user chose not to show again, to store in the settings.
    on_dont_show_again: Option<KeyFn>,
}

impl Notifications {
    /// Creates an empty model; `dismissed_keys` come from the workspace
    /// settings.
    pub fn new(dismissed_keys: Vec<String>) -> Rc<RefCell<Self>> {
        let this = Rc::new(RefCell::new(Self {
            items: Vec::new(),
            ids: Vec::new(),
            next_id: 1,
            model: UiModel::shared(Vec::new()),
            dismissed_keys,
            on_changed: None,
            on_dont_show_again: None,
        }));
        let weak: Weak<RefCell<Self>> = Rc::downgrade(&this);
        this.borrow().model.set_handler(move |row, data| {
            if let Some(n) = weak.upgrade() {
                n.borrow_mut().row_written(row, data);
            }
        });
        this
    }

    /// Sets the callback for state changes (the flag requests the popup).
    pub fn set_on_changed(&mut self, f: impl Fn(NotificationsState, bool) + 'static) {
        self.on_changed = Some(Rc::new(f));
    }

    /// Sets the callback for "don't show again".
    pub fn set_on_dont_show_again(&mut self, f: impl Fn(&str) + 'static) {
        self.on_dont_show_again = Some(Rc::new(f));
    }

    /// The model for `Data.notifications`.
    pub fn model(&self) -> &Rc<UiModel<ui::NotificationData>> {
        &self.model
    }

    /// The current state.
    pub fn state(&self) -> NotificationsState {
        let rows = self.model.to_vec();
        NotificationsState {
            unread: rows
                .iter()
                .filter(|r| r.unread && r.r#type != ui::NotificationType::Progress)
                .count() as i32,
            progress_index: rows
                .iter()
                .position(|r| r.r#type == ui::NotificationType::Progress)
                .map_or(-1, |i| i as i32),
        }
    }

    /// Adds a notification (unless suppressed by its dismiss key).
    pub fn push(&mut self, n: Notification) {
        self.push_with_id(n);
    }

    /// Adds a notification and returns its identifier (for updating or
    /// dismissing it later, e.g. progress notifications), `None` if it is
    /// suppressed by its dismiss key.
    pub fn push_with_id(&mut self, n: Notification) -> Option<NotificationId> {
        if !n.dismiss_key.is_empty() && self.dismissed_keys.contains(&n.dismiss_key) {
            return None;
        }
        let id = NotificationId(self.next_id);
        self.next_id += 1;
        let popup = n.auto_popup;
        // Existing rows keep their state (e.g. read).
        let mut rows: Vec<(NotificationId, Notification, ui::NotificationData)> = self
            .ids
            .drain(..)
            .zip(self.items.drain(..))
            .zip(self.model.to_vec())
            .map(|((id, n), d)| (id, n, d))
            .collect();
        let data = n.to_ui();
        rows.insert(0, (id, n, data));
        rows.sort_by(|(_, _, a), (_, _, b)| {
            b.unread
                .cmp(&a.unread)
                .then_with(|| severity(b.r#type).cmp(&severity(a.r#type)))
        });
        self.ids = rows.iter().map(|(id, _, _)| *id).collect();
        self.items = rows.iter().map(|(_, n, _)| n.clone()).collect();
        self.model
            .replace_all(rows.into_iter().map(|(_, _, d)| d).collect());
        self.changed(popup);
        Some(id)
    }

    /// Updates a notification (e.g. the progress in percent or the
    /// description of a progress notification), if it still exists
    /// (upstream `Notification::setProgress()`, `setDescription()`).
    pub fn update(&mut self, id: NotificationId, f: impl FnOnce(&mut ui::NotificationData)) {
        if let Some(row) = self.ids.iter().position(|i| *i == id) {
            self.model.update(row, f);
            self.changed(false);
        }
    }

    /// Whether a notification still exists.
    pub fn contains(&self, id: NotificationId) -> bool {
        self.ids.contains(&id)
    }

    /// Removes a notification (upstream `Notification::dismiss()`).
    pub fn dismiss(&mut self, id: NotificationId) {
        if let Some(row) = self.ids.iter().position(|i| *i == id) {
            self.ids.remove(row);
            self.items.remove(row);
            self.model.remove(row);
            self.changed(false);
        }
    }

    fn row_written(&mut self, row: usize, data: ui::NotificationData) {
        if data.button_clicked {
            self.model.update(row, |d| d.button_clicked = false);
            if let Some(button) = self.items.get(row).and_then(|n| n.button.clone()) {
                crate::models::defer(move || (button.action)());
            }
        }
        if data.dont_show_again
            && let Some(n) = self.items.get(row)
            && !n.dismiss_key.is_empty()
        {
            self.dismissed_keys.push(n.dismiss_key.clone());
            if let Some(f) = &self.on_dont_show_again {
                f(&n.dismiss_key);
            }
        }
        if data.dismissed && row < self.items.len() {
            self.ids.remove(row);
            self.items.remove(row);
            self.model.remove(row);
        }
        self.changed(false);
    }

    fn changed(&self, popup: bool) {
        if let Some(f) = &self.on_changed {
            f(self.state(), popup);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_by_severity() {
        let n = Notifications::new(vec!["X".into()]);
        let mut m = n.borrow_mut();
        m.push(Notification::new(ui::NotificationType::Info, "a", ""));
        m.push(Notification::new(ui::NotificationType::Critical, "b", ""));
        m.push(Notification::new(ui::NotificationType::Tip, "c", ""));
        let mut suppressed = Notification::new(ui::NotificationType::Warning, "d", "");
        suppressed.dismiss_key = "X".into();
        m.push(suppressed);
        let titles: Vec<_> = m
            .model()
            .to_vec()
            .iter()
            .map(|r| r.title.to_string())
            .collect();
        assert_eq!(titles, ["b", "a", "c"]);
        assert_eq!(
            m.state(),
            NotificationsState {
                unread: 3,
                progress_index: -1
            }
        );
    }
}
