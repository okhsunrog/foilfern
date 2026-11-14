//! Port of libs/librepcb/core/types/boundedunsignedratio.{h,cpp}.

use super::{Error, Length, UnsignedLength, UnsignedRatio};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};

/// A ratio whose resulting value is limited to a range given by min/max
/// lengths, e.g. a pad radius of "25%, but at most 0.25mm".
///
/// Serde: `{"ratio": <ppm>, "min": <nm>, "max": <nm>}`, `min <= max` is
/// checked when deserializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "BoundedUnsignedRatioFields")]
pub struct BoundedUnsignedRatio {
    ratio: UnsignedRatio,
    min: UnsignedLength,
    max: UnsignedLength,
}

/// Unvalidated serde representation of [`BoundedUnsignedRatio`].
#[derive(serde::Deserialize)]
struct BoundedUnsignedRatioFields {
    ratio: UnsignedRatio,
    min: UnsignedLength,
    max: UnsignedLength,
}

impl TryFrom<BoundedUnsignedRatioFields> for BoundedUnsignedRatio {
    type Error = Error;
    fn try_from(f: BoundedUnsignedRatioFields) -> Result<Self, Error> {
        Self::new(f.ratio, f.min, f.max)
    }
}

impl BoundedUnsignedRatio {
    /// Creates the object, or returns an error if `min > max`.
    pub fn new(
        ratio: UnsignedRatio,
        min: UnsignedLength,
        max: UnsignedLength,
    ) -> Result<Self, Error> {
        if min > max {
            return Err(Error::BoundsMinGreaterThanMax);
        }
        Ok(Self { ratio, min, max })
    }

    /// Returns the ratio.
    pub fn ratio(&self) -> UnsignedRatio {
        self.ratio
    }

    /// Returns the minimum value.
    pub fn min_value(&self) -> UnsignedLength {
        self.min
    }

    /// Returns the maximum value.
    pub fn max_value(&self) -> UnsignedLength {
        self.max
    }

    /// Returns `input` scaled by the ratio, clamped to the min/max values.
    pub fn calc_value(&self, input: Length) -> UnsignedLength {
        let value = input
            .scaled(self.ratio.to_normalized())
            .min(*self.max)
            .max(*self.min);
        // Invariant: 0 <= min <= value.
        UnsignedLength::new(value).unwrap_or(self.min)
    }
}

impl SerializeObject for BoundedUnsignedRatio {
    fn serialize(&self, root: &mut List) {
        root.append_child("ratio", &self.ratio);
        root.append_child("min", &self.min);
        root.append_child("max", &self.max);
    }
}

impl DeserializeObject for BoundedUnsignedRatio {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(
            node.child_value("ratio/@0")?,
            node.child_value("min/@0")?,
            node.child_value("max/@0")?,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::Mode;
    use crate::types::Ratio;

    fn ul(nm: i64) -> UnsignedLength {
        UnsignedLength::new(Length::new(nm)).unwrap()
    }

    #[test]
    fn calc_value() {
        let r = BoundedUnsignedRatio::new(
            UnsignedRatio::new(Ratio::from_percent(50)).unwrap(),
            ul(100),
            ul(1000),
        )
        .unwrap();
        assert_eq!(r.calc_value(Length::new(10)), ul(100));
        assert_eq!(r.calc_value(Length::new(1000)), ul(500));
        assert_eq!(r.calc_value(Length::new(10000)), ul(1000));
        assert_eq!(r.calc_value(Length::new(-10000)), ul(100));
    }

    #[test]
    fn min_greater_than_max() {
        let ratio = UnsignedRatio::new(Ratio::ZERO).unwrap();
        assert_eq!(
            BoundedUnsignedRatio::new(ratio, ul(2), ul(1)),
            Err(Error::BoundsMinGreaterThanMax)
        );
    }

    #[test]
    fn serialization() {
        let input = b"(test (ratio 0.25) (min 0.0) (max 0.5))\n";
        let node = SExpression::parse(input, None, Mode::LibrePcb).unwrap();
        let r = BoundedUnsignedRatio::deserialize(&node).unwrap();
        let mut root = List::new("test");
        r.serialize(&mut root);
        assert_eq!(
            SExpression::from(root)
                .to_byte_array(Mode::LibrePcb)
                .unwrap(),
            input
        );

        let invalid = b"(test (ratio 0.25) (min 0.6) (max 0.5))";
        let node = SExpression::parse(invalid, None, Mode::LibrePcb).unwrap();
        assert!(BoundedUnsignedRatio::deserialize(&node).is_err());
    }
}
