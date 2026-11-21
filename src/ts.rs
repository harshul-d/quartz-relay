//! Timestamps (rules.md "Timestamps"): F1, F2 and F3 to epoch milliseconds.

pub const SECOND: i64 = 1_000;
pub const MINUTE: i64 = 60 * SECOND;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant).
#[inline]
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

#[inline(always)]
fn two_digits(bytes: &[u8], at: usize) -> Option<i64> {
    let high = bytes[at].wrapping_sub(b'0');
    let low = bytes[at + 1].wrapping_sub(b'0');
    if high < 10 && low < 10 {
        Some((high * 10 + low) as i64)
    } else {
        None
    }
}

#[inline(always)]
fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// `YYYY-MM-DDTHH:MM:SS` in the first 19 bytes, as milliseconds if UTC.
#[inline]
fn civil(bytes: &[u8]) -> Option<i64> {
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year = two_digits(bytes, 0)? * 100 + two_digits(bytes, 2)?;
    let month = two_digits(bytes, 5)?;
    let day = two_digits(bytes, 8)?;
    let hour = two_digits(bytes, 11)?;
    let minute = two_digits(bytes, 14)?;
    let second = two_digits(bytes, 17)?;
    if year < 1 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let month_days = match month {
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day < 1 || day > month_days {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * MINUTE + second * SECOND)
}

/// Parses a timestamp in any of the three forms. `None` means bad. An F3
/// value too large for 64 bits saturates to `i64::MAX`, which lies outside
/// every window, exactly as rules.md treats it.
#[inline]
pub fn parse(bytes: &[u8]) -> Option<i64> {
    match bytes.len() {
        20 if bytes[19] == b'Z' => civil(bytes),
        25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => {
            let local = civil(bytes)?;
            let offset_hours = two_digits(bytes, 20)?;
            let offset_minutes = two_digits(bytes, 23)?;
            if offset_hours > 23 || offset_minutes > 59 {
                return None;
            }
            let offset = (offset_hours * 60 + offset_minutes) * MINUTE;
            Some(if bytes[19] == b'+' { local - offset } else { local + offset })
        }
        0 => None,
        _ => epoch_millis(bytes),
    }
}

/// F3: ASCII digits only, leading zeros allowed.
#[inline]
fn epoch_millis(bytes: &[u8]) -> Option<i64> {
    let mut value: i64 = 0;
    for &byte in bytes {
        let digit = byte.wrapping_sub(b'0');
        if digit >= 10 {
            return None;
        }
        value = value.saturating_mul(10).saturating_add(digit as i64);
    }
    Some(value)
}

/// A purchase price: ASCII digits only, value 1 to 10,000,000.
#[inline]
pub fn price(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &byte in bytes {
        let digit = byte.wrapping_sub(b'0');
        if digit >= 10 {
            return None;
        }
        value = value * 10 + digit as u64;
        if value > 10_000_000 {
            // Every later digit only keeps it too large, but must still be checked.
            value = 10_000_001;
        }
    }
    if (1..=10_000_000).contains(&value) { Some(value as u32) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        let f1 = parse(b"2026-10-03T23:58:10Z").unwrap();
        assert_eq!(parse(b"2026-10-03T19:58:10-04:00"), Some(f1));
        assert_eq!(parse(b"1791071890000"), Some(f1));
        assert_eq!(parse(b"0001791071890000"), Some(f1));
        assert_eq!(parse(b"2026-10-03T23:58:10-00:00"), Some(f1));
        assert_eq!(parse(b"2026-10-03T23:58:10z"), None);
        assert_eq!(parse(b"2026-10-03t23:58:10Z"), None);
        assert_eq!(parse(b"2026-10-03 23:58:10Z"), None);
        assert_eq!(parse(b"2026-10-03T23:58:10"), None);
        assert_eq!(parse(b"2026-10-03T23:58:10.000Z"), None);
        assert_eq!(parse(b"2026-13-03T23:58:10Z"), None);
        assert_eq!(parse(b"2026-02-30T23:58:10Z"), None);
        assert_eq!(parse(b"2027-02-29T00:00:00Z"), None);
        assert!(parse(b"2028-02-29T00:00:00Z").is_some());
        assert_eq!(parse(b"2026-10-03T24:00:00Z"), None);
        assert_eq!(parse(b"2026-10-03T23:59:60Z"), None);
        assert_eq!(parse(b"2026-10-03T23:58:10+24:00"), None);
        assert_eq!(parse(b"2026-10-03T23:58:10+05:60"), None);
        assert_eq!(parse(b"2026-10-03T23:58:10+0200"), None);
        assert_eq!(parse(b"1791071890000.0"), None);
        assert_eq!(parse(b"-5"), None);
        assert_eq!(parse(b"179102160000000000000"), Some(i64::MAX));
        assert_eq!(parse(b""), None);
        assert_eq!(price(b"0250"), Some(250));
        assert_eq!(price(b"10000000"), Some(10_000_000));
        assert_eq!(price(b"10000001"), None);
        assert_eq!(price(b"0"), None);
        assert_eq!(price(b"19.99"), None);
        assert_eq!(price(b"000000000000000000000000000250"), Some(250));
        assert_eq!(price(b"99999999999999999999999"), None);
    }
}
