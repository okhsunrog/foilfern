//! (De)serialization of primitive and third-party types (the non-member
//! `serialize<T>()`/`deserialize<T>()` specializations at the end of
//! libs/librepcb/core/serialization/sexpression.cpp).
//!
//! `QString` maps to [`String`], `uint`/`int`/`qlonglong` to
//! [`u32`]/[`i32`]/[`i64`], `QDateTime` to [`chrono::DateTime<Utc>`] and
//! `QColor` to [`Color`]. Like upstream, floating point numbers can only be
//! deserialized (their serialization would not be exact).
//!
//! Numbers are parsed with [`str::parse()`] instead of `QString::toInt()`
//! etc. (see COMPAT.md): the accepted set only differs for surrounding
//! whitespace and non-finite or underflowing floats.

use std::str::FromStr;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use super::{Error, FromSExpression, Result, SExpression, ToSExpression};
use crate::types::Color;

impl ToSExpression for str {
    fn to_sexpression(&self) -> SExpression {
        SExpression::string(self)
    }
}

impl ToSExpression for String {
    fn to_sexpression(&self) -> SExpression {
        SExpression::string(self.as_str())
    }
}

impl FromSExpression for String {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        node.value().map(str::to_owned)
    }
}

macro_rules! impl_integer {
    ($t:ty, $err:ident) => {
        impl ToSExpression for $t {
            fn to_sexpression(&self) -> SExpression {
                SExpression::token(self.to_string())
            }
        }

        impl FromSExpression for $t {
            fn from_sexpression(node: &SExpression) -> Result<Self> {
                let value = node.value()?;
                value.parse().map_err(|_| Error::$err(value.to_owned()))
            }
        }
    };
}

impl_integer!(u32, InvalidUnsignedInteger);
impl_integer!(i32, InvalidInteger);
impl_integer!(i64, InvalidLongLong);

/// Parses a finite floating point number (overflow to infinity is rejected
/// like upstream; `inf`/`nan` are rejected too).
fn parse_finite<T: FromStr + Into<f64> + Copy>(value: &str) -> Option<T> {
    value.parse::<T>().ok().filter(|v| (*v).into().is_finite())
}

impl FromSExpression for f32 {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        let value = node.value()?;
        parse_finite(value).ok_or_else(|| Error::InvalidFloat(value.to_owned()))
    }
}

impl FromSExpression for f64 {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        let value = node.value()?;
        parse_finite(value).ok_or_else(|| Error::InvalidDouble(value.to_owned()))
    }
}

impl ToSExpression for bool {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(if *self { "true" } else { "false" })
    }
}

impl FromSExpression for bool {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        match node.value()? {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(Error::InvalidBoolean(other.to_owned())),
        }
    }
}

impl ToSExpression for Color {
    /// Serializes as string in the format `#aarrggbb`.
    fn to_sexpression(&self) -> SExpression {
        SExpression::string(self.to_string())
    }
}

impl FromSExpression for Color {
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

impl ToSExpression for DateTime<Utc> {
    /// Serializes as token in ISO 8601 format with seconds resolution, e.g.
    /// `2019-10-21T19:13:38Z`.
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.format("%Y-%m-%dT%H:%M:%SZ").to_string())
    }
}

impl FromSExpression for DateTime<Utc> {
    /// Parses an ISO 8601 date/time like `QDateTime::fromString(Qt::ISODate)`.
    /// Date/times without timezone are interpreted as local time.
    fn from_sexpression(node: &SExpression) -> Result<Self> {
        let value = node.value()?;
        parse_iso_date_time(value).ok_or_else(|| Error::InvalidDateTime(value.to_owned()))
    }
}

/// Parses `yyyy-MM-dd[(T| )HH:mm[:ss[.zzz]][Z|±HH[:mm]|±HHmm]]`.
fn parse_iso_date_time(s: &str) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    let rest = &s[10..];
    if rest.is_empty() {
        return to_utc(date.and_time(NaiveTime::MIN), None);
    }
    let rest = rest.strip_prefix(['T', ' '])?;
    // Split off the timezone designator.
    let (time, tz) = match rest.find(['Z', '+', '-']) {
        Some(idx) => (&rest[..idx], Some(&rest[idx..])),
        None => (rest, None),
    };
    let time = ["%H:%M:%S%.f", "%H:%M"]
        .iter()
        .find_map(|fmt| NaiveTime::parse_from_str(time, fmt).ok())?;
    let offset = match tz {
        None => None,
        Some("Z") => Some(FixedOffset::east_opt(0)?),
        Some(tz) => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let digits: String = tz[1..].chars().filter(|c| *c != ':').collect();
            if !matches!(digits.len(), 2 | 4) || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let hours: i32 = digits[..2].parse().ok()?;
            let minutes: i32 = digits.get(2..).map_or(Some(0), |m| m.parse().ok())?;
            Some(FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))?)
        }
    };
    to_utc(date.and_time(time), offset)
}

fn to_utc(naive: NaiveDateTime, offset: Option<FixedOffset>) -> Option<DateTime<Utc>> {
    let dt = match offset {
        Some(offset) => offset
            .from_local_datetime(&naive)
            .single()?
            .with_timezone(&Utc),
        None => Local
            .from_local_datetime(&naive)
            .earliest()?
            .with_timezone(&Utc),
    };
    Some(dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(s: &str) -> SExpression {
        SExpression::token(s)
    }

    #[test]
    fn integers() {
        assert_eq!(u32::from_sexpression(&token("42")).unwrap(), 42);
        assert!(u32::from_sexpression(&token("-1")).is_err());
        assert_eq!(i32::from_sexpression(&token("-1")).unwrap(), -1);
        assert!(i32::from_sexpression(&token("1.0")).is_err());
        assert_eq!(i64::MIN.to_sexpression(), token("-9223372036854775808"));
        assert_eq!(i32::from_sexpression(&token("+42")).unwrap(), 42);
        assert!(i32::from_sexpression(&token("2147483648")).is_err());
        assert!(u32::from_sexpression(&token("-0")).is_err());
    }

    #[test]
    fn floats() {
        assert_eq!(f64::from_sexpression(&token("1e3")).unwrap(), 1000.0);
        assert_eq!(f64::from_sexpression(&token("+.5")).unwrap(), 0.5);
        assert!(f64::from_sexpression(&token("1e400")).is_err());
        assert!(f64::from_sexpression(&token("inf")).is_err());
        assert!(f64::from_sexpression(&token("1,5")).is_err());
        assert!(f32::from_sexpression(&token("1e39")).is_err());
        assert_eq!(f32::from_sexpression(&token("-1.5")).unwrap(), -1.5);
    }

    #[test]
    fn booleans() {
        assert_eq!(true.to_sexpression(), token("true"));
        assert!(!bool::from_sexpression(&token("false")).unwrap());
        assert!(bool::from_sexpression(&token("True")).is_err());
    }

    #[test]
    fn strings() {
        assert_eq!("foo".to_sexpression(), SExpression::string("foo"));
        assert_eq!(String::from_sexpression(&token("foo")).unwrap(), "foo");
        assert!(String::from_sexpression(&SExpression::list("foo")).is_err());
    }

    #[test]
    fn date_time() {
        let dt = DateTime::<Utc>::from_sexpression(&token("2019-10-21T19:13:38Z")).unwrap();
        assert_eq!(dt.to_sexpression(), token("2019-10-21T19:13:38Z"));
        let dt =
            DateTime::<Utc>::from_sexpression(&token("2019-10-21T21:13:38.123+02:00")).unwrap();
        assert_eq!(dt.to_sexpression(), token("2019-10-21T19:13:38Z"));
        assert!(DateTime::<Utc>::from_sexpression(&token("2019-10-21X")).is_err());
        assert!(DateTime::<Utc>::from_sexpression(&token("")).is_err());
    }

    #[test]
    fn color() {
        let c = Color::from_sexpression(&SExpression::string("#80ff0000")).unwrap();
        assert_eq!(c, Color::rgba(255, 0, 0, 128));
        assert_eq!(c.to_sexpression(), SExpression::string("#80ff0000"));
        assert!(Color::from_sexpression(&SExpression::string("")).is_err());
    }
}
