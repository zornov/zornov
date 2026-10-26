// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Calendar dates without a time-zone library: days since 1970-01-01.

use std::fmt;

/// A calendar day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub i32);

fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i32 - 719_468
}

fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl Date {
    /// Reads `YYYY-MM-DD`.
    pub fn parse(s: &str) -> Option<Date> {
        let mut it = s.trim().split('-');
        let y: i32 = it.next()?.parse().ok()?;
        let m: u32 = it.next()?.parse().ok()?;
        let d: u32 = it.next()?.parse().ok()?;
        if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        let date = Date(days_from_civil(y, m, d));
        // Reject 31 February and friends.
        (date.ymd() == (y, m, d)).then_some(date)
    }

    /// Year, month, day.
    pub fn ymd(self) -> (i32, u32, u32) {
        civil_from_days(self.0)
    }

    /// 0 = Monday … 6 = Sunday.
    pub fn weekday(self) -> u8 {
        (self.0 + 3).rem_euclid(7) as u8
    }

    /// Three-letter weekday.
    pub fn weekday_name(self) -> &'static str {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][self.weekday() as usize]
    }

    /// Adds days (negative goes back).
    pub fn plus(self, days: i32) -> Date {
        Date(self.0 + days)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

/// `HH:MM` to minutes after midnight.
pub fn parse_clock(s: &str) -> Option<i32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Minutes after midnight to `HH:MM` (wraps past midnight).
pub fn clock(mins: i32) -> String {
    let m = mins.rem_euclid(24 * 60);
    format!("{:02}:{:02}", m / 60, m % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip_and_weekdays_are_right() {
        let d = Date::parse("2026-10-24").unwrap();
        assert_eq!(d.to_string(), "2026-10-24");
        assert_eq!(d.weekday_name(), "Sat");
        assert_eq!(Date::parse("1970-01-01"), Some(Date(0)));
        assert_eq!(Date::parse("2024-02-29").unwrap().plus(1).to_string(), "2024-03-01");
        assert_eq!(Date::parse("2026-02-29"), None);
        assert_eq!(Date::parse("nonsense"), None);
        assert_eq!(parse_clock("07:30"), Some(450));
        assert_eq!(clock(-30), "23:30");
    }
}
