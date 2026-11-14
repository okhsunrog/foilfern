//! Port of libs/librepcb/core/types/version.{h,cpp}.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use super::Error;
use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};

/// Maximum number of segments.
const MAX_SEGMENTS: usize = 10;
/// Maximum value of a segment.
const MAX_NUMBER: u32 = 99_999;

/// A version number like `"1.42.7"`.
///
/// Rules: 1..10 numbers, each 0..99999. Leading zeros of numbers and
/// trailing zero numbers are ignored (`"002.0005.0"` equals `"2.5"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    /// Guaranteed to contain 1..=10 numbers, without trailing zeros (except
    /// the first number).
    numbers: Vec<u32>,
}

impl Version {
    /// Returns whether `s` is a valid version number.
    pub fn is_valid(s: &str) -> bool {
        s.parse::<Version>().is_ok()
    }

    /// Returns the numbers (major version first).
    pub fn numbers(&self) -> &[u32] {
        &self.numbers
    }

    /// Returns whether all segments of `self` are the leading segments of
    /// `other` (e.g. `"1.2"` is a prefix of `"1.2.0.1"`).
    pub fn is_prefix_of(&self, other: &Version) -> bool {
        other.numbers.starts_with(&self.numbers)
    }

    /// Returns the version with at least `min_seg_count` and at most
    /// `max_seg_count` segments (padded with zeros, or truncated).
    pub fn to_pretty_str(&self, min_seg_count: usize, max_seg_count: usize) -> String {
        let count = self.numbers.len().max(min_seg_count).min(max_seg_count);
        (0..count)
            .map(|i| self.numbers.get(i).copied().unwrap_or(0).to_string())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Returns the version as a string which can be compared by simple string
    /// comparison: 10 segments with 5 digits each, e.g.
    /// `"00001.00002.00000.00000.00000.00000.00000.00000.00000.00000"`.
    pub fn to_comparable_str(&self) -> String {
        self.padded()
            .iter()
            .map(|n| format!("{n:05}"))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn padded(&self) -> [u32; MAX_SEGMENTS] {
        let mut numbers = [0; MAX_SEGMENTS];
        numbers[..self.numbers.len()].copy_from_slice(&self.numbers);
        numbers
    }
}

impl Ord for Version {
    /// Numeric comparison, equivalent to comparing
    /// [`to_comparable_str()`](Version::to_comparable_str).
    fn cmp(&self, other: &Self) -> Ordering {
        self.padded().cmp(&other.padded())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    /// Formats the version without trailing zeros, e.g. `"1.2"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_pretty_str(0, MAX_SEGMENTS))
    }
}

// Serde: serialized as the string form, e.g. `"1.2"`.
crate::utils::serde_string::serde_string!(Version);

impl FromStr for Version {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidVersion(s.to_owned());
        let mut numbers = s
            .split('.')
            .map(|n| n.parse::<u32>().ok().filter(|&n| n <= MAX_NUMBER))
            .collect::<Option<Vec<u32>>>()
            .ok_or_else(invalid)?;
        while numbers.len() > 1 && numbers.last() == Some(&0) {
            numbers.pop();
        }
        if numbers.len() > MAX_SEGMENTS {
            return Err(invalid());
        }
        Ok(Self { numbers })
    }
}

impl ToSExpression for Version {
    fn to_sexpression(&self) -> SExpression {
        SExpression::string(self.to_string())
    }
}

impl FromSExpression for Version {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}
