//! Ports of tests/unittests/core/export/*.cpp (except the graphics export
//! tests, which are not ported yet), plus tests of the BOM.

mod bom_csv_writer_test;
mod d356_netlist_generator_test;
mod excellon_generator_test;
mod gerber_aperture_list_test;
mod gerber_attribute_test;
mod gerber_attribute_writer_test;
mod gerber_generator_test;
mod interactive_html_bom_test;
mod pick_place_csv_writer_test;

use chrono::{FixedOffset, NaiveDate, TimeZone};
use librepcb_core::export::Timestamp;
use librepcb_core::types::{Angle, Length, PositiveLength, UnsignedLength};

/// Upstream `PositiveLength(nm)`.
pub fn pos(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).unwrap()
}

/// Upstream `UnsignedLength(nm)`.
pub fn uns(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).unwrap()
}

/// Upstream `Angle(microdegrees)`.
pub fn deg(microdegrees: i32) -> Angle {
    Angle::new(microdegrees)
}

/// Upstream `QDateTime(QDate(y, m, d), QTime(h, min, s, ms),
/// Qt::OffsetFromUTC, 3600)`.
pub fn date_utc_plus_1(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32, ms: u32) -> Timestamp {
    let naive = NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_milli_opt(h, min, s, ms)
        .unwrap();
    Timestamp::from(
        FixedOffset::east_opt(3600)
            .unwrap()
            .from_local_datetime(&naive)
            .unwrap(),
    )
}
