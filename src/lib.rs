//! A small, dependency-free iterator over Unix cron schedules.
//!
//! The crate parses the five traditional cron fields (minute, hour, day of
//! month, month, day of week) and walks forward through matching instants one
//! minute at a time. That is deliberately simple: a naive minute-by-minute
//! scan is easy to reason about, has no hidden state, and is fast enough for
//! the usual case of finding the next few runs of a schedule. The trade-off is
//! that schedules with sparse matches (for example, `0 0 29 2 *`) can require
//! many iterations before a match is found.
//!
//! The iterator is bounded: it refuses to search beyond a fixed horizon of
//! four years. This keeps a bad expression from hanging forever and makes the
//! failure mode explicit (`None`).

use std::fmt;

/// Number of minutes in the search horizon. Four years is long enough to
/// include a leap day, which is the sparsest schedule the five-field syntax
/// can express.
const HORIZON_MINUTES: i64 = 4 * 366 * 24 * 60;

/// An error produced while parsing a cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        ParseError {
            message: message.into(),
        }
    }

    /// A human-readable description of what went wrong.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// A parsed cron expression.
///
/// Fields are stored as sorted, deduplicated vectors of allowed values. The
/// day-of-month and day-of-week fields are combined with the traditional cron
/// OR rule: a date matches if either field matches. The one exception is when
/// both fields are unrestricted (`*`), in which case every day matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: Vec<u32>,
    hours: Vec<u32>,
    days_of_month: Vec<u32>,
    months: Vec<u32>,
    days_of_week: Vec<u32>,
    dom_restricted: bool,
    dow_restricted: bool,
}

impl CronSchedule {
    /// Parses a five-field cron expression.
    ///
    /// Accepted syntax for each field: `*`, a single number, a range `a-b`,
    /// a step `*/n` or `a-b/n`, and comma-separated lists of those. Named
    /// months and weekdays are not supported; this is a numeric-only parser.
    pub fn parse(expr: &str) -> Result<CronSchedule, ParseError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(ParseError::new(format!(
                "expected 5 fields, found {}",
                fields.len()
            )));
        }

        let minutes = parse_field(fields[0], 0, 59, "minute")?;
        let hours = parse_field(fields[1], 0, 23, "hour")?;
        let days_of_month = parse_field(fields[2], 1, 31, "day of month")?;
        let months = parse_field(fields[3], 1, 12, "month")?;
        let days_of_week = parse_field(fields[4], 0, 6, "day of week")?;

        let dom_restricted = fields[2] != "*";
        let dow_restricted = fields[4] != "*";

        Ok(CronSchedule {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            dom_restricted,
            dow_restricted,
        })
    }

    /// Returns true when the given minute-aligned Unix timestamp matches.
    ///
    /// Timestamps are interpreted as UTC. The caller is responsible for
    /// passing a minute-aligned value; non-aligned values are handled by
    /// truncating to the containing minute, which is what the iterator does
    /// internally anyway.
    pub fn matches(&self, timestamp: i64) -> bool {
        let (year, month, day, hour, minute, weekday) = civil_from_timestamp(timestamp);
        let _ = year; // year is not part of the five-field cron syntax
        if !self.minutes.contains(&minute) {
            return false;
        }
        if !self.hours.contains(&hour) {
            return false;
        }
        if !self.months.contains(&month) {
            return false;
        }
        let dom_match = self.days_of_month.contains(&day);
        let dow_match = self.days_of_week.contains(&weekday);
        if self.dom_restricted && self.dow_restricted {
            dom_match || dow_match
        } else if self.dom_restricted {
            dom_match
        } else if self.dow_restricted {
            dow_match
        } else {
            true
        }
    }

    /// Returns an iterator over matching instants at or after `start`.
    ///
    /// The iterator yields Unix timestamps in UTC, one per match, advancing
    /// minute by minute. It stops yielding after the search horizon is
    /// exhausted, which is reported as `None`.
    pub fn iter_from(&self, start: i64) -> CronIterator<'_> {
        // Align to the containing minute so a caller passing an arbitrary
        // second does not accidentally skip the current minute.
        let aligned = start - start.rem_euclid(60);
        CronIterator {
            schedule: self,
            current: aligned,
            remaining: HORIZON_MINUTES,
        }
    }
}

/// Iterator returned by [`CronSchedule::iter_from`].
#[derive(Debug, Clone)]
pub struct CronIterator<'a> {
    schedule: &'a CronSchedule,
    current: i64,
    remaining: i64,
}

impl<'a> Iterator for CronIterator<'a> {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        while self.remaining >= 0 {
            if self.schedule.matches(self.current) {
                let value = self.current;
                self.current += 60;
                self.remaining -= 1;
                return Some(value);
            }
            self.current += 60;
            self.remaining -= 1;
        }
        None
    }
}

fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    name: &str,
) -> Result<Vec<u32>, ParseError> {
    if field.is_empty() {
        return Err(ParseError::new(format!("empty {} field", name)));
    }
    let mut values = Vec::new();
    for part in field.split(',') {
        if part.is_empty() {
            return Err(ParseError::new(format!(
                "empty element in {} field",
                name
            )));
        }
        parse_part(part, min, max, name, &mut values)?;
    }
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        return Err(ParseError::new(format!("{} field matches nothing", name)));
    }
    Ok(values)
}

fn parse_part(
    part: &str,
    min: u32,
    max: u32,
    name: &str,
    out: &mut Vec<u32>,
) -> Result<(), ParseError> {
    let (range_part, step) = match part.split_once('/') {
        Some((r, s)) => {
            let step: u32 = s.parse().map_err(|_| {
                ParseError::new(format!("invalid step {:?} in {} field", s, name))
            })?;
            if step == 0 {
                return Err(ParseError::new(format!(
                    "step must be greater than zero in {} field",
                    name
                )));
            }
            (r, step)
        }
        None => (part, 1),
    };

    let (start, end) = if range_part == "*" {
        (min, max)
    } else if let Some((a, b)) = range_part.split_once('-') {
        let a = parse_number(a, min, max, name)?;
        let b = parse_number(b, min, max, name)?;
        if a > b {
            return Err(ParseError::new(format!(
                "range start {} is greater than end {} in {} field",
                a, b, name
            )));
        }
        (a, b)
    } else {
        let v = parse_number(range_part, min, max, name)?;
        (v, v)
    };

    let mut value = start;
    while value <= end {
        out.push(value);
        value += step;
    }
    Ok(())
}

fn parse_number(s: &str, min: u32, max: u32, name: &str) -> Result<u32, ParseError> {
    let value: u32 = s
        .parse()
        .map_err(|_| ParseError::new(format!("invalid number {:?} in {} field", s, name)))?;
    if value < min || value > max {
        return Err(ParseError::new(format!(
            "value {} out of range {}-{} in {} field",
            value, min, max, name
        )));
    }
    Ok(value)
}

/// Converts a Unix timestamp (UTC) into civil date components.
///
/// The algorithm is Howard Hinnant's `civil_from_days`, which is exact for
/// the proleptic Gregorian calendar and avoids pulling in a date crate.
/// Weekday is 0 for Sunday, matching cron's convention.
fn civil_from_timestamp(timestamp: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = timestamp.div_euclid(86_400);
    let seconds_of_day = timestamp.rem_euclid(86_400);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    if month <= 2 {
        year += 1;
    }

    // 1970-01-01 was a Thursday. Sunday is 0, so Thursday is 4.
    let weekday = (days + 4).rem_euclid(7) as u32;

    (year, month, day, hour, minute, weekday)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        // Compute a Unix timestamp from civil components using the inverse of
        // the civil algorithm. This keeps the tests independent of the
        // implementation under test.
        let y = if mo <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = if mo > 2 { mo - 3 } else { mo + 9 } as i64;
        let doy = (153 * mp + 2) / 5 + d as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + h as i64 * 3_600 + mi as i64 * 60
    }

    #[test]
    fn parses_wildcards() {
        let s = CronSchedule::parse("* * * * *").unwrap();
        assert_eq!(s.minutes.len(), 60);
        assert_eq!(s.hours.len(), 24);
        assert_eq!(s.days_of_month.len(), 31);
        assert_eq!(s.months.len(), 12);
        assert_eq!(s.days_of_week.len(), 7);
    }

    #[test]
    fn parses_lists_and_ranges() {
        let s = CronSchedule::parse("0,15,30-32 9-10 * * *").unwrap();
        assert_eq!(s.minutes, vec![0, 15, 30, 31, 32]);
        assert_eq!(s.hours, vec![9, 10]);
    }

    #[test]
    fn parses_steps() {
        let s = CronSchedule::parse("*/15 * * * *").unwrap();
        assert_eq!(s.minutes, vec![0, 15, 30, 45]);

        let s = CronSchedule::parse("10-20/3 * * * *").unwrap();
        assert_eq!(s.minutes, vec![10, 13, 16, 19]);
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(CronSchedule::parse("* * * *").is_err());
        assert!(CronSchedule::parse("* * * * * *").is_err());
    }

    #[test]
    fn rejects_out_of_range_and_bad_syntax() {
        assert!(CronSchedule::parse("60 * * * *").is_err());
        assert!(CronSchedule::parse("* 24 * * *").is_err());
        assert!(CronSchedule::parse("* * 0 * *").is_err());
        assert!(CronSchedule::parse("* * * 13 *").is_err());
        assert!(CronSchedule::parse("* * * * 7").is_err());
        assert!(CronSchedule::parse("*/0 * * * *").is_err());
        assert!(CronSchedule::parse("5-1 * * * *").is_err());
        assert!(CronSchedule::parse("a * * * *").is_err());
    }

    #[test]
    fn matches_exact_minute() {
        let s = CronSchedule::parse("30 14 * * *").unwrap();
        assert!(s.matches(ts(2024, 1, 15, 14, 30)));
        assert!(!s.matches(ts(2024, 1, 15, 14, 31)));
        assert!(!s.matches(ts(2024, 1, 15, 15, 30)));
    }

    #[test]
    fn dom_and_dow_use_or_rule() {
        // 15th of the month OR Friday.
        let s = CronSchedule::parse("0 0 15 * 5").unwrap();
        // 2024-03-15 is a Friday, so it matches both; pick a non-Friday 15th.
        assert!(s.matches(ts(2024, 4, 15, 0, 0)));
        // A Friday that is not the 15th.
        assert!(s.matches(ts(2024, 3, 1, 0, 0)));
        // Neither.
        assert!(!s.matches(ts(2024, 4, 16, 0, 0)));
    }

    #[test]
    fn dom_restricted_dow_wildcard_uses_dom_only() {
        let s = CronSchedule::parse("0 0 15 * *").unwrap();
        assert!(s.matches(ts(2024, 4, 15, 0, 0)));
        assert!(!s.matches(ts(2024, 4, 16, 0, 0)));
    }

    #[test]
    fn dow_restricted_dom_wildcard_uses_dow_only() {
        let s = CronSchedule::parse("0 0 * * 1").unwrap();
        // 2024-04-15 is a Monday.
        assert!(s.matches(ts(2024, 4, 15, 0, 0)));
        assert!(!s.matches(ts(2024, 4, 16, 0, 0)));
    }

    #[test]
    fn iterator_yields_successive_matches() {
        let s = CronSchedule::parse("*/15 * * * *").unwrap();
        let start = ts(2024, 1, 1, 0, 0);
        let got: Vec<i64> = s.iter_from(start).take(4).collect();
        assert_eq!(
            got,
            vec![
                ts(2024, 1, 1, 0, 0),
                ts(2024, 1, 1, 0, 15),
                ts(2024, 1, 1, 0, 30),
                ts(2024, 1, 1, 0, 45),
            ]
        );
    }

    #[test]
    fn iterator_aligns_to_containing_minute() {
        let s = CronSchedule::parse("0 * * * *").unwrap();
        // 00:00:30 should still see 00:00 as the first match.
        let start = ts(2024, 1, 1, 0, 0) + 30;
        let first = s.iter_from(start).next().unwrap();
        assert_eq!(first, ts(2024, 1, 1, 0, 0));
    }

    #[test]
    fn iterator_finds_leap_day() {
        // February 29th exists only in leap years. 2024 is a leap year.
        let s = CronSchedule::parse("0 0 29 2 *").unwrap();
        let start = ts(2024, 1, 1, 0, 0);
        let first = s.iter_from(start).next().unwrap();
        assert_eq!(first, ts(2024, 2, 29, 0, 0));
    }

    #[test]
    fn iterator_horizon_returns_none() {
        // February 30th never exists, so the iterator must exhaust its
        // horizon and stop rather than looping forever.
        let s = CronSchedule::parse("0 0 30 2 *").unwrap();
        let start = ts(2024, 1, 1, 0, 0);
        assert!(s.iter_from(start).next().is_none());
    }
}
