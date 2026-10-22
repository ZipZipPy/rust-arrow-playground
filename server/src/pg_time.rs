//! Raw PostgreSQL `TIMESTAMPTZ` and `DATE` values.
//!
//! Postgres sends timestamps as microseconds and dates as days since
//! 2000-01-01. These types shift that to the Unix epoch, which is exactly what
//! Arrow's `Timestamp(Microsecond)` and `Date32` store, so conversion is one
//! integer addition, and they format themselves as RFC 3339 / ISO 8601 text
//! for JSON without allocating.

use std::error::Error;

use bytes::{BufMut, BytesMut};
use chrono::{Datelike, NaiveDate};
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type, to_sql_checked};

type BoxError = Box<dyn Error + Sync + Send>;

/// Microseconds from 1970-01-01 to 2000-01-01.
const PG_EPOCH_MICROS: i64 = 946_684_800_000_000;
/// Days from 1970-01-01 to 2000-01-01.
const PG_EPOCH_DAYS: i32 = 10_957;
/// Days from 0001-01-01 (chrono's "CE" day 1) to 1970-01-01.
const UNIX_EPOCH_DAYS_FROM_CE: i32 = 719_163;

pub const MICROS_PER_DAY: i64 = 86_400_000_000;

/// Length of `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
pub const RFC3339_LEN: usize = 27;
/// Length of `YYYY-MM-DD`.
pub const DATE_LEN: usize = 10;

/// A `TIMESTAMPTZ` as microseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// A `DATE` as days since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("date or timestamp outside the years 0000..=9999")]
pub struct OutOfRange;

impl Timestamp {
    pub fn date(self) -> Date {
        Date(self.0.div_euclid(MICROS_PER_DAY) as i32)
    }

    /// Writes `YYYY-MM-DDTHH:MM:SS.ffffffZ` into `buf` and returns it as text.
    pub fn write_rfc3339(self, buf: &mut [u8; RFC3339_LEN]) -> Result<&str, OutOfRange> {
        let micros_of_day = self.0.rem_euclid(MICROS_PER_DAY);
        let (date, time) = buf.split_at_mut(DATE_LEN);
        self.date().write_into(date)?;

        let seconds_of_day = micros_of_day / 1_000_000;
        time[0] = b'T';
        write_digits(&mut time[1..3], seconds_of_day / 3600);
        time[3] = b':';
        write_digits(&mut time[4..6], seconds_of_day / 60 % 60);
        time[6] = b':';
        write_digits(&mut time[7..9], seconds_of_day % 60);
        time[9] = b'.';
        write_digits(&mut time[10..16], micros_of_day % 1_000_000);
        time[16] = b'Z';
        Ok(ascii(buf))
    }
}

impl Date {
    /// Writes `YYYY-MM-DD` into `buf` and returns it as text.
    pub fn write_iso(self, buf: &mut [u8; DATE_LEN]) -> Result<&str, OutOfRange> {
        self.write_into(buf)?;
        Ok(ascii(buf))
    }

    fn write_into(self, out: &mut [u8]) -> Result<(), OutOfRange> {
        let date = self
            .0
            .checked_add(UNIX_EPOCH_DAYS_FROM_CE)
            .and_then(NaiveDate::from_num_days_from_ce_opt)
            .filter(|date| (0..=9999).contains(&date.year()))
            .ok_or(OutOfRange)?;
        write_digits(&mut out[0..4], i64::from(date.year()));
        out[4] = b'-';
        write_digits(&mut out[5..7], i64::from(date.month()));
        out[7] = b'-';
        write_digits(&mut out[8..10], i64::from(date.day()));
        Ok(())
    }
}

/// Writes `value` as zero-padded decimal filling all of `out`.
fn write_digits(out: &mut [u8], mut value: i64) {
    for digit in out.iter_mut().rev() {
        *digit = b'0' + (value % 10) as u8;
        value /= 10;
    }
}

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("formatter writes ASCII only")
}

impl From<Timestamp> for i64 {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}

impl From<Date> for i32 {
    fn from(value: Date) -> Self {
        value.0
    }
}

fn fixed<const N: usize>(raw: &[u8]) -> Result<[u8; N], BoxError> {
    raw.try_into()
        .map_err(|_| format!("expected {N} bytes, got {}", raw.len()).into())
}

impl<'a> FromSql<'a> for Timestamp {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        let pg_micros = i64::from_be_bytes(fixed(raw)?);
        // Postgres' +/-infinity are i64::MAX/MIN and do not survive the shift.
        let micros = pg_micros
            .checked_add(PG_EPOCH_MICROS)
            .ok_or("timestamp out of range")?;
        Ok(Self(micros))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::TIMESTAMPTZ | Type::TIMESTAMP)
    }
}

impl<'a> FromSql<'a> for Date {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        let pg_days = i32::from_be_bytes(fixed(raw)?);
        let days = pg_days
            .checked_add(PG_EPOCH_DAYS)
            .ok_or("date out of range")?;
        Ok(Self(days))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::DATE
    }
}

impl ToSql for Timestamp {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        let pg_micros = self
            .0
            .checked_sub(PG_EPOCH_MICROS)
            .ok_or("timestamp out of range")?;
        out.put_i64(pg_micros);
        Ok(IsNull::No)
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::TIMESTAMPTZ | Type::TIMESTAMP)
    }

    to_sql_checked!();
}

impl ToSql for Date {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        let pg_days = self
            .0
            .checked_sub(PG_EPOCH_DAYS)
            .ok_or("date out of range")?;
        out.put_i32(pg_days);
        Ok(IsNull::No)
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::DATE
    }

    to_sql_checked!();
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, SecondsFormat};

    use super::*;

    fn rfc3339(micros: i64) -> String {
        let mut buf = [0; RFC3339_LEN];
        Timestamp(micros)
            .write_rfc3339(&mut buf)
            .unwrap()
            .to_owned()
    }

    fn iso(days: i32) -> String {
        let mut buf = [0; DATE_LEN];
        Date(days).write_iso(&mut buf).unwrap().to_owned()
    }

    #[test]
    fn formats_timestamps_with_microseconds_and_z() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000000Z");
        assert_eq!(
            rfc3339(1_735_732_800_123_456),
            "2025-01-01T12:00:00.123456Z"
        );
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59.999999Z");
        assert_eq!(rfc3339(951_782_400_000_001), "2000-02-29T00:00:00.000001Z");
    }

    #[test]
    fn timestamp_formatting_matches_chrono() {
        let mut micros = -2_208_988_800_000_000; // 1900-01-01
        while micros < 4_102_444_800_000_000 {
            // 2100-01-01
            let expected = DateTime::from_timestamp_micros(micros)
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Micros, true);
            assert_eq!(rfc3339(micros), expected);
            micros += 7_777_777_777_777; // ~90 days, odd remainder
        }
    }

    #[test]
    fn formats_dates() {
        assert_eq!(iso(0), "1970-01-01");
        assert_eq!(iso(20_091), "2025-01-03");
        assert_eq!(iso(-1), "1969-12-31");
        assert_eq!(iso(PG_EPOCH_DAYS), "2000-01-01");
    }

    #[test]
    fn rejects_years_that_rfc3339_cannot_express() {
        let mut buf = [0; DATE_LEN];
        assert_eq!(Date(3_000_000).write_iso(&mut buf), Err(OutOfRange));
        let mut buf = [0; RFC3339_LEN];
        assert_eq!(Timestamp(i64::MAX).write_rfc3339(&mut buf), Err(OutOfRange));
    }

    #[test]
    fn timestamp_knows_its_date() {
        assert_eq!(Timestamp(MICROS_PER_DAY * 3 + 5).date(), Date(3));
        assert_eq!(Timestamp(-1).date(), Date(-1));
    }

    #[test]
    fn round_trips_through_the_postgres_wire_format() {
        let ts = Timestamp(1_735_732_800_123_456);
        let mut buf = BytesMut::new();
        ts.to_sql(&Type::TIMESTAMPTZ, &mut buf).unwrap();
        assert_eq!(Timestamp::from_sql(&Type::TIMESTAMPTZ, &buf).unwrap(), ts);

        let date = Date(20_091);
        let mut buf = BytesMut::new();
        date.to_sql(&Type::DATE, &mut buf).unwrap();
        assert_eq!(Date::from_sql(&Type::DATE, &buf).unwrap(), date);

        let infinity = i64::MAX.to_be_bytes();
        assert!(Timestamp::from_sql(&Type::TIMESTAMPTZ, &infinity).is_err());
    }
}
