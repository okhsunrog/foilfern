//! Port of libs/librepcb/core/library/cmp/cmpsigpindisplaytype.{h,cpp}.

use std::fmt;
use std::str::FromStr;

use foilfern_i18n::tr;

use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};
use crate::types::Error;

/// What text is displayed at a symbol pin in schematics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CmpSigPinDisplayType {
    /// No text.
    None,
    /// The name of the symbol pin.
    PinName,
    /// The name of the component signal (default).
    #[default]
    ComponentSignal,
    /// The name of the connected net.
    NetSignal,
}

impl CmpSigPinDisplayType {
    /// All display types in UI order.
    pub const ALL: [Self; 4] = [
        Self::None,
        Self::PinName,
        Self::ComponentSignal,
        Self::NetSignal,
    ];

    /// Returns the serialization token (never change these values!).
    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PinName => "pin",
            Self::ComponentSignal => "signal",
            Self::NetSignal => "net",
        }
    }

    /// Returns the translated, human readable name.
    pub fn name_tr(self) -> String {
        match self {
            Self::None => tr!("CmpSigPinDisplayType", "None (no text)"),
            Self::PinName => tr!("CmpSigPinDisplayType", "Symbol pin name"),
            Self::ComponentSignal => tr!("CmpSigPinDisplayType", "Component signal name"),
            Self::NetSignal => tr!("CmpSigPinDisplayType", "Schematic net name"),
        }
    }
}

impl fmt::Display for CmpSigPinDisplayType {
    /// Formats the serialization token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for CmpSigPinDisplayType {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|t| t.to_str() == s)
            .ok_or_else(|| Error::InvalidCmpSigPinDisplayType(s.to_owned()))
    }
}

impl ToSExpression for CmpSigPinDisplayType {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for CmpSigPinDisplayType {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}
