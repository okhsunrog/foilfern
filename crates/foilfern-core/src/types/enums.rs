//! Port of libs/librepcb/core/types/enums.{h,cpp}.

use std::fmt;
use std::str::FromStr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// Mode for automatic online updates (ordered by "amount of automation").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AutoUpdateMode {
    /// Automatic update check disabled.
    Disabled,
    /// Automatically check for updates, but no notification.
    Check,
    /// Automatically check for updates, then notify the user.
    Notify,
    /// Automatically check for, download, and install updates.
    Install,
}

/// Grid style of the 2D graphics views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridStyle {
    /// No grid.
    None,
    /// Dots.
    Dots,
    /// Lines.
    Lines,
}

macro_rules! token_enum {
    ($name:ident, $err:path, {$($variant:ident => $token:literal),* $(,)?}) => {
        impl $name {
            /// Returns the serialization token.
            pub fn to_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $token,)*
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.to_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self, Error> {
                match s {
                    $($token => Ok(Self::$variant),)*
                    _ => Err($err(s.to_owned())),
                }
            }
        }

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                SExpression::token(self.to_str())
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(node.value()?.parse()?)
            }
        }
    };
}

token_enum!(AutoUpdateMode, Error::UnknownAutoUpdateMode, {
    Disabled => "disabled",
    Check => "check",
    Notify => "notify",
    Install => "install",
});

token_enum!(GridStyle, Error::UnknownGridStyle, {
    None => "none",
    Dots => "dots",
    Lines => "lines",
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for mode in [
            AutoUpdateMode::Disabled,
            AutoUpdateMode::Check,
            AutoUpdateMode::Notify,
            AutoUpdateMode::Install,
        ] {
            assert_eq!(
                AutoUpdateMode::from_sexpression(&mode.to_sexpression()),
                Ok(mode)
            );
        }
        for style in [GridStyle::None, GridStyle::Dots, GridStyle::Lines] {
            assert_eq!(
                GridStyle::from_sexpression(&style.to_sexpression()),
                Ok(style)
            );
        }
        assert!(GridStyle::from_sexpression(&SExpression::token("foo")).is_err());
        assert!(AutoUpdateMode::Disabled < AutoUpdateMode::Install);
    }
}
