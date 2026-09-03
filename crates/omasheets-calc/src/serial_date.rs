//! The Excel 1900 serial-date boundary for the owned M0 engine.
//!
//! The M0 value model has no typed date. A date is a plain [`crate::Value::Number`]
//! holding an Excel serial: the number of days since the fictitious
//! 1900-01-00, with any fraction being the time of day. That is exactly what
//! `.xlsx` files store, so import parity needs no conversion, and it keeps the
//! typed date decision recorded in `docs/ADR-0004-SERIAL-DATE-BOUNDARY.md`
//! open instead of guessing at it here.
//!
//! Every conversion between a serial and a civil `(year, month, day)` triple
//! goes through this module so the compatibility quirks live in one place:
//!
//! - serial `0` is Excel's "1900-01-00";
//! - serial `60` is Excel's fictitious 1900-02-29 (Lotus 1-2-3 treated 1900 as
//!   a leap year and Excel preserved the bug), so serials `1..=59` are one day
//!   behind the real calendar and serials `>= 61` are aligned with it;
//! - serials below `0` or above `2_958_465` (9999-12-31) are `#NUM!` errors,
//!   surfaced as [`CalcError::InvalidNumber`];
//! - only the 1900 date system is supported; 1904-epoch workbooks are
//!   rejected explicitly by the importer rather than silently offset.
//!
//! Nothing here reads a clock. `TODAY` and `NOW` stay out of the engine until
//! explicit tick-event semantics exist, because reopening a workbook must not
//! silently change its values.

use crate::CalcError;

/// Name of the only date system the owned engine evaluates.
pub const DATE_SYSTEM: &str = "1900";
/// Excel's "1900-01-00".
pub const MIN_SERIAL: i64 = 0;
/// 9999-12-31 in the 1900 date system.
pub const MAX_SERIAL: i64 = 2_958_465;
/// The fictitious 1900-02-29.
pub const LEAP_BUG_SERIAL: i64 = 60;
/// 1970-01-01 in the 1900 date system.
const UNIX_EPOCH_SERIAL: i64 = 25_569;
/// Largest magnitude accepted for a `DATE`/`EDATE`/`EOMONTH` component before
/// truncation, keeping the `i64` calendar arithmetic far from overflow.
const MAX_COMPONENT: f64 = 1.0e9;

/// A civil date as Excel reports it, including the two fictitious days.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CivilDate {
    pub year: i64,
    pub month: u32,
    /// `0` only for serial `0`, Excel's "1900-01-00".
    pub day: u32,
}

/// Truncates a numeric serial the way `YEAR`, `MONTH`, `DAY`, `EDATE`,
/// `EOMONTH` and `WEEKDAY` do, rejecting negative and out-of-range values.
pub fn serial_from_number(value: f64) -> Result<i64, CalcError> {
    if !value.is_finite() || value < MIN_SERIAL as f64 || value >= (MAX_SERIAL + 1) as f64 {
        return Err(CalcError::InvalidNumber);
    }
    Ok(value.trunc() as i64)
}

/// Splits a serial into Excel's civil date, applying the 1900 leap-year quirk.
pub fn civil_from_serial(serial: i64) -> Result<CivilDate, CalcError> {
    if !(MIN_SERIAL..=MAX_SERIAL).contains(&serial) {
        return Err(CalcError::InvalidNumber);
    }
    if serial == MIN_SERIAL {
        return Ok(CivilDate {
            year: 1900,
            month: 1,
            day: 0,
        });
    }
    if serial == LEAP_BUG_SERIAL {
        return Ok(CivilDate {
            year: 1900,
            month: 2,
            day: 29,
        });
    }
    let unix_days = if serial < LEAP_BUG_SERIAL {
        serial - (UNIX_EPOCH_SERIAL - 1)
    } else {
        serial - UNIX_EPOCH_SERIAL
    };
    Ok(civil_from_days(unix_days))
}

/// Builds a serial from a civil triple with Excel's overflow rules: months
/// outside `1..=12` roll the year, and days outside the month roll forward or
/// backward by plain serial arithmetic (so day `0` is the previous month's last
/// day). The year is used as given; see [`date_serial`] for `DATE`'s
/// two-digit-year adjustment.
pub fn serial_from_civil(year: i64, month: i64, day: i64) -> Result<i64, CalcError> {
    for component in [year, month, day] {
        if component.unsigned_abs() > MAX_COMPONENT as u64 {
            return Err(CalcError::InvalidNumber);
        }
    }
    let total_months = year * 12 + (month - 1);
    let year = total_months.div_euclid(12);
    let month = total_months.rem_euclid(12) + 1;
    let serial = first_of_month_serial(year, month) + (day - 1);
    if (MIN_SERIAL..=MAX_SERIAL).contains(&serial) {
        Ok(serial)
    } else {
        Err(CalcError::InvalidNumber)
    }
}

/// `DATE(year, month, day)`: components are truncated toward zero, years
/// `0..=1899` are offset by 1900, and years outside `0..=9999` are `#NUM!`.
pub fn date_serial(year: f64, month: f64, day: f64) -> Result<i64, CalcError> {
    let year = component(year)?;
    let month = component(month)?;
    let day = component(day)?;
    let year = match year {
        0..=1899 => year + 1900,
        1900..=9999 => year,
        _ => return Err(CalcError::InvalidNumber),
    };
    serial_from_civil(year, month, day)
}

/// `EDATE(start, months)`: the same day of the month `months` away, clamped to
/// that month's last day.
pub fn add_months(serial: i64, months: i64) -> Result<i64, CalcError> {
    let date = civil_from_serial(serial)?;
    if months.unsigned_abs() > MAX_COMPONENT as u64 {
        return Err(CalcError::InvalidNumber);
    }
    let total_months = date.year * 12 + i64::from(date.month) - 1 + months;
    let year = total_months.div_euclid(12);
    let month = total_months.rem_euclid(12) + 1;
    let day = i64::from(date.day).min(i64::from(days_in_month(year, month)));
    serial_from_civil(year, month, day)
}

/// `EOMONTH(start, months)`: the last day of the month `months` away.
pub fn end_of_month(serial: i64, months: i64) -> Result<i64, CalcError> {
    let date = civil_from_serial(serial)?;
    if months.unsigned_abs() > MAX_COMPONENT as u64 {
        return Err(CalcError::InvalidNumber);
    }
    serial_from_civil(date.year, i64::from(date.month) + months + 1, 0)
}

/// `WEEKDAY(serial, return_type)` for return types 1, 2 and 3. Excel derives
/// the weekday from the serial alone, which is why serials `1..=59` report the
/// weekday of the following real day and 1900-01-01 is a "Sunday".
pub fn weekday(serial: i64, return_type: i64) -> Result<i64, CalcError> {
    if !(MIN_SERIAL..=MAX_SERIAL).contains(&serial) {
        return Err(CalcError::InvalidNumber);
    }
    match return_type {
        1 => Ok((serial - 1).rem_euclid(7) + 1),
        2 => Ok((serial - 2).rem_euclid(7) + 1),
        3 => Ok((serial - 2).rem_euclid(7)),
        _ => Err(CalcError::InvalidNumber),
    }
}

/// `YEARFRAC(start, end, basis)` for bases 0 to 4, following Excel's
/// documented day-count conventions: US 30/360, actual/actual, actual/360,
/// actual/365 and European 30/360. Dates are swapped so the result is never
/// negative.
pub fn year_fraction(start: i64, end: i64, basis: i64) -> Result<f64, CalcError> {
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    let first = civil_from_serial(start)?;
    let last = civil_from_serial(end)?;
    let actual = (end - start) as f64;
    match basis {
        0 => {
            let (mut d1, mut d2) = (i64::from(first.day), i64::from(last.day));
            let first_is_end_of_february =
                first.month == 2 && first.day == days_in_month(first.year, 2);
            let last_is_end_of_february =
                last.month == 2 && last.day == days_in_month(last.year, 2);
            if first_is_end_of_february && last_is_end_of_february {
                d2 = 30;
            }
            if first_is_end_of_february {
                d1 = 30;
            }
            if d2 == 31 && d1 >= 30 {
                d2 = 30;
            }
            if d1 == 31 {
                d1 = 30;
            }
            Ok(days_360_between(
                first.year,
                i64::from(first.month),
                d1,
                last.year,
                i64::from(last.month),
                d2,
            ) as f64
                / 360.0)
        }
        1 => {
            let year_length = if first.year == last.year {
                days_in_year(first.year)
            } else if last.year == first.year + 1
                && (first.month, first.day) >= (last.month, last.day)
            {
                // Within one year: 366 only when a 29 February lies inside.
                let february_29_in_first =
                    is_leap_year(first.year) && (first.month, first.day) <= (2, 29);
                let february_29_in_last =
                    is_leap_year(last.year) && (last.month, last.day) >= (2, 29);
                if february_29_in_first || february_29_in_last {
                    366.0
                } else {
                    365.0
                }
            } else {
                let years = (first.year..=last.year).map(days_in_year).sum::<f64>();
                years / (last.year - first.year + 1) as f64
            };
            Ok(actual / year_length)
        }
        2 => Ok(actual / 360.0),
        3 => Ok(actual / 365.0),
        4 => {
            let d1 = if first.day == 31 {
                30
            } else {
                i64::from(first.day)
            };
            let d2 = if last.day == 31 {
                30
            } else {
                i64::from(last.day)
            };
            Ok(days_360_between(
                first.year,
                i64::from(first.month),
                d1,
                last.year,
                i64::from(last.month),
                d2,
            ) as f64
                / 360.0)
        }
        _ => Err(CalcError::InvalidNumber),
    }
}

/// `DAYS360(start, end, method)`: the US (NASD) convention unless
/// `european` is set. Negative when `end` precedes `start`, as in Excel.
pub fn days_360(start: i64, end: i64, european: bool) -> Result<i64, CalcError> {
    let first = civil_from_serial(start)?;
    let last = civil_from_serial(end)?;
    let (mut d1, mut d2) = (i64::from(first.day), i64::from(last.day));
    if european {
        if d1 == 31 {
            d1 = 30;
        }
        if d2 == 31 {
            d2 = 30;
        }
    } else {
        if d1 == 31 || (first.month == 2 && first.day == days_in_month(first.year, 2)) {
            d1 = 30;
        }
        if d2 == 31 && d1 >= 30 {
            d2 = 30;
        }
    }
    Ok(days_360_between(
        first.year,
        i64::from(first.month),
        d1,
        last.year,
        i64::from(last.month),
        d2,
    ))
}

fn days_360_between(y1: i64, m1: i64, d1: i64, y2: i64, m2: i64, d2: i64) -> i64 {
    (y2 - y1) * 360 + (m2 - m1) * 30 + (d2 - d1)
}

fn days_in_year(year: i64) -> f64 {
    if year == 1900 || is_leap_year(year) {
        366.0
    } else {
        365.0
    }
}

fn is_weekend(serial: i64) -> bool {
    // Return type 1: 1 is Sunday, 7 is Saturday.
    matches!((serial - 1).rem_euclid(7) + 1, 1 | 7)
}

/// `NETWORKDAYS`: whole working days between two serials inclusive, minus
/// the holidays that fall on working days; negative when `end` precedes
/// `start`.
pub fn network_days(start: i64, end: i64, holidays: &std::collections::HashSet<i64>) -> i64 {
    let (low, high, sign) = if start <= end {
        (start, end, 1)
    } else {
        (end, start, -1)
    };
    let span = high - low + 1;
    let full_weeks = span / 7;
    let mut count = full_weeks * 5;
    for serial in low + full_weeks * 7..=high {
        if !is_weekend(serial) {
            count += 1;
        }
    }
    let excluded = holidays
        .iter()
        .filter(|holiday| (low..=high).contains(holiday) && !is_weekend(**holiday))
        .count() as i64;
    sign * (count - excluded)
}

/// `WORKDAY`: the serial `days` working days after (or before) `start`.
pub fn work_day(
    start: i64,
    days: i64,
    holidays: &std::collections::HashSet<i64>,
) -> Result<i64, CalcError> {
    if days.abs() > 1_000_000 {
        return Err(CalcError::InvalidNumber);
    }
    let step = days.signum();
    let mut remaining = days.abs();
    let mut serial = start;
    while remaining > 0 {
        serial += step;
        if !(MIN_SERIAL..=MAX_SERIAL).contains(&serial) {
            return Err(CalcError::InvalidNumber);
        }
        if !is_weekend(serial) && !holidays.contains(&serial) {
            remaining -= 1;
        }
    }
    Ok(serial)
}

/// Days in a month as Excel counts them: February 1900 has 29.
fn days_in_month(year: i64, month: i64) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year == 1900 || is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!("month is normalised to 1..=12"),
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn component(value: f64) -> Result<i64, CalcError> {
    if !value.is_finite() || value.abs() > MAX_COMPONENT {
        return Err(CalcError::InvalidNumber);
    }
    Ok(value.trunc() as i64)
}

fn first_of_month_serial(year: i64, month: i64) -> i64 {
    let unix_days = days_from_civil(year, month, 1);
    if unix_days >= days_from_civil(1900, 3, 1) {
        unix_days + UNIX_EPOCH_SERIAL
    } else {
        unix_days + UNIX_EPOCH_SERIAL - 1
    }
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of [`days_from_civil`] (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> CivilDate {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    CivilDate {
        year: if month <= 2 { year + 1 } else { year },
        month: month as u32,
        day: day as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn civil(year: i64, month: u32, day: u32) -> CivilDate {
        CivilDate { year, month, day }
    }

    #[test]
    fn maps_the_1900_boundary_serials_exactly_as_excel_does() {
        assert_eq!(civil_from_serial(0), Ok(civil(1900, 1, 0)));
        assert_eq!(civil_from_serial(1), Ok(civil(1900, 1, 1)));
        assert_eq!(civil_from_serial(59), Ok(civil(1900, 2, 28)));
        assert_eq!(civil_from_serial(60), Ok(civil(1900, 2, 29)));
        assert_eq!(civil_from_serial(61), Ok(civil(1900, 3, 1)));
        assert_eq!(civil_from_serial(25_569), Ok(civil(1970, 1, 1)));
        assert_eq!(civil_from_serial(45_292), Ok(civil(2024, 1, 1)));
        assert_eq!(civil_from_serial(45_351), Ok(civil(2024, 2, 29)));
        assert_eq!(civil_from_serial(MAX_SERIAL), Ok(civil(9999, 12, 31)));
        assert_eq!(civil_from_serial(-1), Err(CalcError::InvalidNumber));
        assert_eq!(
            civil_from_serial(MAX_SERIAL + 1),
            Err(CalcError::InvalidNumber)
        );
    }

    #[test]
    fn round_trips_every_serial_through_the_civil_boundary() {
        let mut previous = civil_from_serial(0).unwrap();
        for serial in 1..=MAX_SERIAL {
            let date = civil_from_serial(serial).unwrap();
            assert_eq!(
                serial_from_civil(date.year, i64::from(date.month), i64::from(date.day)),
                Ok(serial),
                "{date:?}"
            );
            let month_length = days_in_month(date.year, i64::from(date.month));
            assert!(date.day >= 1 && date.day <= month_length, "{date:?}");
            let consecutive = if previous.month == date.month {
                date.day == previous.day + 1
            } else {
                previous.day == days_in_month(previous.year, i64::from(previous.month))
                    && date.day == 1
                    && (date.year * 12 + i64::from(date.month))
                        == (previous.year * 12 + i64::from(previous.month) + 1)
            };
            assert!(consecutive, "{previous:?} -> {date:?}");
            previous = date;
        }
    }

    #[test]
    fn builds_date_serials_with_excel_overflow_and_year_rules() {
        assert_eq!(date_serial(1900.0, 1.0, 1.0), Ok(1));
        assert_eq!(date_serial(1900.0, 1.0, 0.0), Ok(0));
        assert_eq!(date_serial(1900.0, 2.0, 29.0), Ok(60));
        assert_eq!(date_serial(1900.0, 2.0, 30.0), Ok(61));
        assert_eq!(date_serial(1900.0, 3.0, 0.0), Ok(60));
        assert_eq!(date_serial(1900.0, 1.0, 61.0), Ok(61));
        assert_eq!(date_serial(2024.0, 2.0, 29.0), Ok(45_351));
        assert_eq!(date_serial(2023.0, 14.0, 29.0), Ok(45_351));
        assert_eq!(date_serial(2024.0, 3.0, 0.0), Ok(45_351));
        assert_eq!(date_serial(2025.0, -10.0, 29.0), Ok(45_351));
        assert_eq!(date_serial(2024.9, 2.9, 29.9), Ok(45_351));
        assert_eq!(
            date_serial(24.0, 2.0, 29.0),
            Ok(date_serial(1924.0, 2.0, 29.0).unwrap())
        );
        assert_eq!(date_serial(0.0, 1.0, 1.0), Ok(1));
        assert_eq!(date_serial(9999.0, 12.0, 31.0), Ok(MAX_SERIAL));
        assert_eq!(
            date_serial(9999.0, 12.0, 32.0),
            Err(CalcError::InvalidNumber)
        );
        assert_eq!(
            date_serial(1900.0, 1.0, -1.0),
            Err(CalcError::InvalidNumber)
        );
        assert_eq!(date_serial(-1.0, 1.0, 1.0), Err(CalcError::InvalidNumber));
        assert_eq!(
            date_serial(10_000.0, 1.0, 1.0),
            Err(CalcError::InvalidNumber)
        );
        assert_eq!(
            date_serial(2024.0, f64::NAN, 1.0),
            Err(CalcError::InvalidNumber)
        );
        assert_eq!(
            date_serial(2024.0, 1.0, 1.0e12),
            Err(CalcError::InvalidNumber)
        );
    }

    #[test]
    fn shifts_months_with_end_of_month_clamping() {
        let jan_31_2024 = date_serial(2024.0, 1.0, 31.0).unwrap();
        assert_eq!(add_months(jan_31_2024, 1), Ok(45_351));
        assert_eq!(add_months(jan_31_2024, 13), date_serial(2025.0, 2.0, 28.0));
        assert_eq!(add_months(jan_31_2024, -2), date_serial(2023.0, 11.0, 30.0));
        assert_eq!(add_months(jan_31_2024, 0), Ok(jan_31_2024));
        assert_eq!(add_months(59, 1), Ok(88));
        assert_eq!(add_months(60, 12), date_serial(1901.0, 2.0, 28.0));
        assert_eq!(add_months(0, 1), Ok(31));
        assert_eq!(end_of_month(jan_31_2024, 0), Ok(jan_31_2024));
        assert_eq!(end_of_month(jan_31_2024, 1), Ok(45_351));
        assert_eq!(end_of_month(45_292, -1), date_serial(2023.0, 12.0, 31.0));
        assert_eq!(end_of_month(1, 0), Ok(31));
        assert_eq!(end_of_month(32, 0), Ok(60));
        assert_eq!(end_of_month(MAX_SERIAL, 1), Err(CalcError::InvalidNumber));
        assert_eq!(add_months(-1, 1), Err(CalcError::InvalidNumber));
    }

    #[test]
    fn reports_weekdays_from_the_serial_alone() {
        assert_eq!(weekday(0, 1), Ok(7));
        assert_eq!(weekday(1, 1), Ok(1));
        assert_eq!(weekday(61, 1), Ok(5));
        assert_eq!(weekday(45_292, 1), Ok(2));
        assert_eq!(weekday(45_292, 2), Ok(1));
        assert_eq!(weekday(45_292, 3), Ok(0));
        assert_eq!(weekday(45_291, 2), Ok(7));
        assert_eq!(weekday(45_291, 3), Ok(6));
        assert_eq!(weekday(45_292, 11), Err(CalcError::InvalidNumber));
        assert_eq!(weekday(-1, 1), Err(CalcError::InvalidNumber));
    }

    #[test]
    fn truncates_serial_numbers_and_rejects_the_out_of_range_ones() {
        assert_eq!(serial_from_number(45_292.75), Ok(45_292));
        assert_eq!(serial_from_number(0.5), Ok(0));
        assert_eq!(serial_from_number(2_958_465.999), Ok(MAX_SERIAL));
        assert_eq!(serial_from_number(-0.5), Err(CalcError::InvalidNumber));
        assert_eq!(
            serial_from_number(2_958_466.0),
            Err(CalcError::InvalidNumber)
        );
        assert_eq!(
            serial_from_number(f64::INFINITY),
            Err(CalcError::InvalidNumber)
        );
    }
}
