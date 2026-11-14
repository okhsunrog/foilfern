//! Port of libs/librepcb/core/types/signalrole.{h,cpp}.

use std::fmt;
use std::str::FromStr;

use foilfern_i18n::tr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// Electrical role of a signal (e.g. of a component signal).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SignalRole {
    /// Passive (default).
    #[default]
    Passive,
    /// Power.
    Power,
    /// Input.
    Input,
    /// Output.
    Output,
    /// Input/output.
    InOut,
    /// Open drain.
    OpenDrain,
}

impl SignalRole {
    /// All roles in UI order.
    pub const ALL: [SignalRole; 6] = [
        Self::Passive,
        Self::Power,
        Self::Input,
        Self::Output,
        Self::InOut,
        Self::OpenDrain,
    ];

    /// Returns the serialization token (never change these values!).
    pub fn to_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Power => "power",
            Self::Input => "input",
            Self::Output => "output",
            Self::InOut => "inout",
            Self::OpenDrain => "opendrain",
        }
    }

    /// Returns the translated, human readable name.
    pub fn name_tr(self) -> String {
        match self {
            Self::Passive => tr!("SignalRole", "Passive"),
            Self::Power => tr!("SignalRole", "Power"),
            Self::Input => tr!("SignalRole", "Input"),
            Self::Output => tr!("SignalRole", "Output"),
            Self::InOut => tr!("SignalRole", "I/O"),
            Self::OpenDrain => tr!("SignalRole", "Open Drain"),
        }
    }
}

impl fmt::Display for SignalRole {
    /// Formats the serialization token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for SignalRole {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|r| r.to_str() == s)
            .ok_or_else(|| Error::UnknownSignalRole(s.to_owned()))
    }
}

impl ToSExpression for SignalRole {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for SignalRole {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}
