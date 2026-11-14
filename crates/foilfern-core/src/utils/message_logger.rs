//! Port of libs/librepcb/core/utils/messagelogger.{h,cpp}.
//!
//! A thread-safe logger collecting (and forwarding) messages of long running
//! operations like library imports, so they can be shown to the user
//! afterwards.
//!
//! Differences to upstream:
//! - No Qt signal: a top-level logger can get a listener callback
//!   ([`MessageLogger::with_listener()`]) which is called for every message.
//! - Top-level loggers forward messages to the [`log`] crate instead of Qt's
//!   message handler.
//! - Child loggers borrow their parent (upstream: `QPointer`), so they can't
//!   outlive it.
//! - `toRichText()` is not ported (UI specific); [`LogMessage::message`] is
//!   the plain text.

use std::fmt;
use std::sync::Mutex;

/// Severity of a [`LogMessage`] (upstream `QtMsgType`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LogLevel {
    /// Debug message.
    Debug,
    /// Informational message.
    Info,
    /// Warning.
    Warning,
    /// Error (upstream "critical").
    Critical,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        })
    }
}

/// A logged message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogMessage {
    /// Severity.
    pub level: LogLevel,
    /// Message text (plain text, may contain line breaks).
    pub message: String,
}

type Listener = Box<dyn Fn(&LogMessage) + Send + Sync>;

/// A message logger, either top-level or a child logger which prefixes its
/// messages with a group name (`"[group] "`) and forwards them to its parent.
pub struct MessageLogger<'a> {
    parent: Option<&'a MessageLogger<'a>>,
    prefix: String,
    record: bool,
    messages: Mutex<Vec<LogMessage>>,
    listener: Option<Listener>,
}

static_assertions::assert_impl_all!(MessageLogger<'static>: Send, Sync);

impl Default for MessageLogger<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MessageLogger<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageLogger")
            .field("prefix", &self.prefix)
            .field("record", &self.record)
            .field("messages", &self.messages())
            .finish_non_exhaustive()
    }
}

impl<'a> MessageLogger<'a> {
    /// Creates a top-level logger recording all messages.
    pub fn new() -> Self {
        Self {
            parent: None,
            prefix: String::new(),
            record: true,
            messages: Mutex::new(Vec::new()),
            listener: None,
        }
    }

    /// Creates a top-level logger recording all messages and calling
    /// `listener` for each message (upstream signal `msgEmitted()`).
    pub fn with_listener(listener: impl Fn(&LogMessage) + Send + Sync + 'static) -> Self {
        Self {
            listener: Some(Box::new(listener)),
            ..Self::new()
        }
    }

    /// Creates a child logger forwarding its messages to `parent`, prefixed
    /// with `"[group] "` if `group` is not empty. The child logger itself
    /// records its (unprefixed) messages too.
    pub fn child(parent: &'a MessageLogger<'a>, group: &str) -> Self {
        Self {
            parent: Some(parent),
            prefix: if group.is_empty() {
                String::new()
            } else {
                format!("[{group}] ")
            },
            record: true,
            messages: Mutex::new(Vec::new()),
            listener: None,
        }
    }

    /// Returns whether messages were recorded.
    pub fn has_messages(&self) -> bool {
        !self.lock().is_empty()
    }

    /// Returns the recorded messages.
    pub fn messages(&self) -> Vec<LogMessage> {
        self.lock().clone()
    }

    /// Returns the texts of the recorded messages.
    pub fn messages_plain(&self) -> Vec<String> {
        self.lock().iter().map(|m| m.message.clone()).collect()
    }

    /// Removes all recorded messages.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// Logs a message.
    pub fn log(&self, level: LogLevel, msg: &str) {
        if let Some(parent) = self.parent {
            parent.log(level, &format!("{}{}", self.prefix, msg));
        } else {
            match level {
                LogLevel::Debug => log::debug!("{msg}"),
                LogLevel::Info => log::info!("{msg}"),
                LogLevel::Warning => log::warn!("{msg}"),
                LogLevel::Critical => log::error!("{msg}"),
            }
        }
        let obj = LogMessage {
            level,
            message: msg.to_owned(),
        };
        if let Some(listener) = &self.listener {
            listener(&obj);
        }
        if self.record {
            self.lock().push(obj);
        }
    }

    /// Logs a debug message.
    pub fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }

    /// Logs an informational message.
    pub fn info(&self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }

    /// Logs a warning.
    pub fn warning(&self, msg: &str) {
        self.log(LogLevel::Warning, msg);
    }

    /// Logs an error.
    pub fn critical(&self, msg: &str) {
        self.log(LogLevel::Critical, msg);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<LogMessage>> {
        // A panic while holding the lock can't leave the list inconsistent.
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_child_prefixes_messages() {
        let root = MessageLogger::new();
        {
            let child = MessageLogger::child(&root, "sym");
            let grandchild = MessageLogger::child(&child, "");
            grandchild.warning("foo");
            assert_eq!(child.messages_plain(), vec!["foo".to_owned()]);
        }
        assert_eq!(root.messages_plain(), vec!["[sym] foo".to_owned()]);
        assert_eq!(root.messages()[0].level, LogLevel::Warning);
        root.clear();
        assert!(!root.has_messages());
    }
}
