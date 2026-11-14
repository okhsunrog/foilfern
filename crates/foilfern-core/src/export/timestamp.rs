//! Creation timestamps of exported files.
//!
//! Not an upstream class: upstream passes a `QDateTime` and writes it with
//! `QDateTime::toString(Qt::ISODate)`, whose output depends on the time spec
//! of the value (local time without offset, UTC with `Z`, others with
//! `+hh:mm`). [`Timestamp`] makes that distinction explicit so the written
//! bytes are the same.

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, Utc};

/// A point in time as written into exported files (Gerber/Excellon
/// creation date, IPC-D-356A and pick&place generation date).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timestamp {
    /// Local time, written without offset (e.g. `2019-01-02T03:04:05`).
    /// This is what upstream exports (`QDateTime::currentDateTime()`).
    Local(NaiveDateTime),
    /// UTC, written with `Z` suffix (e.g. `2019-01-02T03:04:05Z`).
    Utc(NaiveDateTime),
    /// Time with a fixed UTC offset, written with the offset in hours and
    /// minutes (e.g. `2019-01-02T03:04:05+01:00`). An offset of zero is
    /// written as `Z` (Qt converts such values to UTC).
    Offset(DateTime<FixedOffset>),
}

impl Timestamp {
    /// Returns the current local time (upstream
    /// `QDateTime::currentDateTime()`).
    pub fn now() -> Self {
        Self::Local(Local::now().naive_local())
    }

    /// Returns the date and time as shown on a clock in the time zone of
    /// the value (what `QDateTime::toString(format)` prints).
    pub fn date_time(&self) -> NaiveDateTime {
        match self {
            Self::Local(dt) | Self::Utc(dt) => *dt,
            Self::Offset(dt) => dt.naive_local(),
        }
    }

    /// Returns the ISO 8601 representation with second precision, like
    /// `QDateTime::toString(Qt::ISODate)`.
    pub fn to_iso_string(&self) -> String {
        const FORMAT: &str = "%Y-%m-%dT%H:%M:%S";
        match self {
            Self::Local(dt) => dt.format(FORMAT).to_string(),
            Self::Utc(dt) => format!("{}Z", dt.format(FORMAT)),
            Self::Offset(dt) => {
                let offset = dt.offset().local_minus_utc();
                let mut s = dt.naive_local().format(FORMAT).to_string();
                if offset == 0 {
                    s.push('Z');
                } else {
                    let sign = if offset < 0 { '-' } else { '+' };
                    let abs = offset.unsigned_abs();
                    s.push_str(&format!("{sign}{:02}:{:02}", abs / 3600, (abs / 60) % 60));
                }
                s
            }
        }
    }
}

impl From<DateTime<Local>> for Timestamp {
    fn from(dt: DateTime<Local>) -> Self {
        Self::Local(dt.naive_local())
    }
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(dt: DateTime<Utc>) -> Self {
        Self::Utc(dt.naive_utc())
    }
}

impl From<DateTime<FixedOffset>> for Timestamp {
    fn from(dt: DateTime<FixedOffset>) -> Self {
        Self::Offset(dt)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone};

    use super::*;

    fn naive() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2000, 2, 1)
            .unwrap()
            .and_hms_milli_opt(1, 2, 3, 4)
            .unwrap()
    }

    #[test]
    fn iso_string() {
        assert_eq!(
            Timestamp::Local(naive()).to_iso_string(),
            "2000-02-01T01:02:03"
        );
        assert_eq!(
            Timestamp::Utc(naive()).to_iso_string(),
            "2000-02-01T01:02:03Z"
        );
        let offset = |secs| {
            Timestamp::from(
                FixedOffset::east_opt(secs)
                    .unwrap()
                    .from_local_datetime(&naive())
                    .unwrap(),
            )
            .to_iso_string()
        };
        assert_eq!(offset(3600), "2000-02-01T01:02:03+01:00");
        assert_eq!(offset(-19800), "2000-02-01T01:02:03-05:30");
        assert_eq!(offset(0), "2000-02-01T01:02:03Z");
    }
}
