//! Messages of the library element checks (upstream: all subclasses of
//! `RuleCheckMessage` in libs/librepcb/core/library/**/*checkmessages.h).
//!
//! Each upstream `*checkmessages.h` file maps to one enum with one variant
//! per message class; [`LibraryCheckMessage`] combines them, so every
//! element's check returns a `Vec<LibraryCheckMessage>`. The generic part
//! (severity, texts, approval) is available through
//! [`LibraryCheckMessage::to_message()`].

use super::cat::LibraryCategoryCheckMessage;
use super::cmp::ComponentCheckMessage;
use super::dev::DeviceCheckMessage;
use super::library_base_element_check_messages::LibraryBaseElementCheckMessage;
use super::library_element_check_messages::LibraryElementCheckMessage;
use super::pkg::PackageCheckMessage;
use super::sym::SymbolCheckMessage;
use crate::rule_check::RuleCheckMessage;

/// A message of a library element check.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LibraryCheckMessage {
    /// Check common to all elements.
    BaseElement(LibraryBaseElementCheckMessage),
    /// Check common to symbols, packages, components and devices.
    Element(LibraryElementCheckMessage),
    /// Category check.
    Category(LibraryCategoryCheckMessage),
    /// Symbol check.
    Symbol(SymbolCheckMessage),
    /// Component check.
    Component(ComponentCheckMessage),
    /// Package check.
    Package(PackageCheckMessage),
    /// Device check.
    Device(DeviceCheckMessage),
}

impl LibraryCheckMessage {
    /// Returns the generic rule check message (severity, translated texts,
    /// approval node).
    pub fn to_message(&self) -> RuleCheckMessage {
        match self {
            Self::BaseElement(m) => m.to_message(),
            Self::Element(m) => m.to_message(),
            Self::Category(m) => m.to_message(),
            Self::Symbol(m) => m.to_message(),
            Self::Component(m) => m.to_message(),
            Self::Package(m) => m.to_message(),
            Self::Device(m) => m.to_message(),
        }
    }
}

macro_rules! impl_from {
    ($($variant:ident($ty:ty)),* $(,)?) => {
        $(
            impl From<$ty> for LibraryCheckMessage {
                fn from(msg: $ty) -> Self {
                    Self::$variant(msg)
                }
            }
        )*
    };
}

impl_from!(
    BaseElement(LibraryBaseElementCheckMessage),
    Element(LibraryElementCheckMessage),
    Category(LibraryCategoryCheckMessage),
    Symbol(SymbolCheckMessage),
    Component(ComponentCheckMessage),
    Package(PackageCheckMessage),
    Device(DeviceCheckMessage),
);

impl From<&LibraryCheckMessage> for RuleCheckMessage {
    fn from(msg: &LibraryCheckMessage) -> Self {
        msg.to_message()
    }
}
