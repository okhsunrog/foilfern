//! Port of libs/librepcb/core/types/uuid.{h,cpp}.

use std::fmt;
use std::str::FromStr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// A random (version 4, DCE variant) UUID, always represented in lowercase
/// hyphenated form (e.g. `"d2c30518-5cd1-4ce9-a569-44f783a3f66a"`).
///
/// Only such UUIDs are accepted; the ordering is identical to the ordering of
/// the string representations.
///
/// Serde: serialized as the hyphenated string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid(uuid::Uuid);

crate::utils::serde_string::serde_string!(Uuid);

impl Uuid {
    /// Generates a new random UUID.
    pub fn new_random() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Returns whether `s` is a valid UUID string (lowercase, hyphenated,
    /// version 4, DCE variant).
    pub fn is_valid(s: &str) -> bool {
        let bytes = s.as_bytes();
        bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, &b)| match i {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
            })
            && bytes[14] == b'4' // Version: random
            && matches!(bytes[19], b'8' | b'9' | b'a' | b'b') // Variant: DCE
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

impl FromStr for Uuid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        if Self::is_valid(s) {
            uuid::Uuid::parse_str(s)
                .map(Self)
                .map_err(|_| Error::InvalidUuid(s.to_owned()))
        } else {
            Err(Error::InvalidUuid(s.to_owned()))
        }
    }
}

impl ToSExpression for Uuid {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_string())
    }
}

impl FromSExpression for Uuid {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

impl ToSExpression for Option<Uuid> {
    /// Serializes `None` as token `none`.
    fn to_sexpression(&self) -> SExpression {
        match self {
            Some(uuid) => uuid.to_sexpression(),
            None => SExpression::token("none"),
        }
    }
}

impl FromSExpression for Option<Uuid> {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        match node.value()? {
            "none" => Ok(None),
            _ => Uuid::from_sexpression(node).map(Some),
        }
    }
}
