//! Timestamps (rules.md "Timestamps"): F1, F2 and F3 to epoch milliseconds.

pub const SECOND: i64 = 1_000;
pub const MINUTE: i64 = 60 * SECOND;
pub const HOUR: i64 = 60 * MINUTE;
pub const DAY: i64 = 24 * HOUR;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant).
#[inline]
pub const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
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

/// The plain, obviously correct parser; `parse` must agree with it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_simple(bytes: &[u8]) -> Option<i64> {
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

/// First year of the month-start table; dates outside it take the slow path.
const TABLE_FIRST_YEAR: u32 = 1900;
const TABLE_YEARS: usize = 300;

/// Days since the epoch of the 1st of each month from 1900-01 on, plus one
/// extra entry, so a month's length is the difference of two neighbours.
static MONTH_START: [i32; TABLE_YEARS * 12 + 1] = {
    let mut table = [0i32; TABLE_YEARS * 12 + 1];
    let mut index = 0;
    while index < table.len() {
        let year = TABLE_FIRST_YEAR as i64 + (index / 12) as i64;
        let month = (index % 12) as i64 + 1;
        table[index] = days_from_civil(year, month, 1) as i32;
        index += 1;
    }
    table
};

/// `YYYY-MM-DDTHH:MM:SS` in the first 19 bytes, as milliseconds if UTC:
/// all 14 digit bytes checked at once, the date from the month-start table.
#[inline(always)]
fn civil_fast(bytes: &[u8]) -> Option<i64> {
    let b: &[u8; 19] = bytes[..19].try_into().unwrap();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let d = |i: usize| b[i].wrapping_sub(b'0') as u32;
    let digits = [d(0), d(1), d(2), d(3), d(5), d(6), d(8), d(9), d(11), d(12), d(14), d(15), d(17), d(18)];
    let mut worst = 0;
    for digit in digits {
        worst |= (digit > 9) as u32;
    }
    if worst != 0 {
        return None;
    }
    let year = digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3];
    let month = digits[4] * 10 + digits[5];
    let day = digits[6] * 10 + digits[7];
    let hour = digits[8] * 10 + digits[9];
    let minute = digits[10] * 10 + digits[11];
    let second = digits[12] * 10 + digits[13];
    if hour > 23 || minute > 59 || second > 59 || month.wrapping_sub(1) > 11 || day == 0 {
        return None;
    }
    let year_index = year.wrapping_sub(TABLE_FIRST_YEAR) as usize;
    if year_index >= TABLE_YEARS {
        return civil(bytes); // rare: the general path
    }
    let index = year_index * 12 + month as usize - 1;
    let start = MONTH_START[index];
    if day as i32 > MONTH_START[index + 1] - start {
        return None; // Feb 30, Feb 29 outside a leap year, Apr 31, ...
    }
    let days = (start + day as i32 - 1) as i64;
    Some(((days * 24 + hour as i64) * 60 + minute as i64) * MINUTE + second as i64 * SECOND)
}

/// Eight ASCII digits (first digit in the lowest byte) to their value, or
/// `None` if any byte is not a digit.
#[inline(always)]
fn eight_digits(chunk: u64) -> Option<u64> {
    const ZEROS: u64 = 0x3030_3030_3030_3030;
    const HIGH: u64 = 0xF0F0_F0F0_F0F0_F0F0;
    if chunk & HIGH != ZEROS || (chunk + 0x0606_0606_0606_0606) & HIGH != ZEROS {
        return None;
    }
    let chunk = chunk - ZEROS;
    let chunk = ((chunk & 0x0F00_0F00_0F00_0F00) >> 8) + (chunk & 0x000F_000F_000F_000F) * 10;
    let chunk = ((chunk & 0x00FF_0000_00FF_0000) >> 16) + (chunk & 0x0000_00FF_0000_00FF) * 100;
    Some(((chunk & 0x0000_FFFF_0000_0000) >> 32) + (chunk & 0x0000_0000_0000_FFFF) * 10_000)
}

/// F3 with 8 digits at a time; values beyond 18 significant digits
/// saturate (they are outside every window).
#[inline(always)]
fn epoch_millis_fast(bytes: &[u8]) -> Option<i64> {
    let mut value: u64 = 0;
    let mut at = 0;
    let mut significant = 0; // digits after leading zeros, capped well above 18
    while at + 8 <= bytes.len() {
        let chunk = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let eight = eight_digits(chunk)?;
        if significant > 0 || eight != 0 {
            // Leading zeros inside the chunk do not count.
            significant += if significant > 0 { 8 } else { 8 - leading_zero_digits(chunk) };
        }
        value = value.wrapping_mul(100_000_000).wrapping_add(eight);
        at += 8;
    }
    while at < bytes.len() {
        let digit = bytes[at].wrapping_sub(b'0');
        if digit >= 10 {
            return None;
        }
        if significant > 0 || digit != 0 {
            significant += 1;
        }
        value = value.wrapping_mul(10).wrapping_add(digit as u64);
        at += 1;
    }
    if significant > 18 { Some(i64::MAX) } else { Some(value as i64) }
}

/// Count of leading `'0'` bytes in an 8-digit chunk (first digit lowest).
#[inline(always)]
fn leading_zero_digits(chunk: u64) -> u32 {
    let nonzero = chunk ^ 0x3030_3030_3030_3030;
    if nonzero == 0 { 8 } else { nonzero.trailing_zeros() / 8 }
}

/// Parses a timestamp in any of the three forms. `None` means bad. An F3
/// value too large for 64 bits saturates to `i64::MAX`, which lies outside
/// every window, exactly as rules.md treats it.
#[inline(always)]
pub fn parse(bytes: &[u8]) -> Option<i64> {
    match bytes.len() {
        20 if bytes[19] == b'Z' => civil_fast(bytes),
        25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => {
            let local = civil_fast(bytes)?;
            let offset_hours = two_digits(bytes, 20)?;
            let offset_minutes = two_digits(bytes, 23)?;
            if offset_hours > 23 || offset_minutes > 59 {
                return None;
            }
            let offset = (offset_hours * 60 + offset_minutes) * MINUTE;
            Some(if bytes[19] == b'+' { local - offset } else { local + offset })
        }
        0 => None,
        _ => epoch_millis_fast(bytes),
    }
}

/// `parse` with a per-thread cache of recently seen dates: a window spans a
/// few dozen distinct `YYYY-MM-DD`s, so the date part is validated and
/// converted once per distinct date, keyed by its exact 10 bytes. The time
/// part is parsed with SWAR. Same results as `parse` for every input.
pub struct Parser {
    /// (first 8 bytes, bytes 8-9) of a valid date; zero (impossible: the
    /// bytes are ASCII digits and dashes) marks an empty entry.
    keys: Vec<(u64, u16)>,
    days: Vec<i64>,
}

const DATE_CACHE: usize = 256;

impl Default for Parser {
    fn default() -> Parser {
        Parser { keys: vec![(0, 0); DATE_CACHE], days: vec![0; DATE_CACHE] }
    }
}

impl Parser {
    #[inline(always)]
    pub fn parse(&mut self, bytes: &[u8]) -> Option<i64> {
        let local = match bytes.len() {
            20 if bytes[19] == b'Z' => return self.civil(bytes),
            25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => self.civil(bytes)?,
            0 => return None,
            _ => return epoch_millis_fast(bytes),
        };
        let offset_hours = two_digits(bytes, 20)?;
        let offset_minutes = two_digits(bytes, 23)?;
        if offset_hours > 23 || offset_minutes > 59 {
            return None;
        }
        let offset = (offset_hours * 60 + offset_minutes) * MINUTE;
        Some(if bytes[19] == b'+' { local - offset } else { local + offset })
    }

    /// `YYYY-MM-DDTHH:MM:SS` in the first 19 bytes, as UTC milliseconds.
    #[inline(always)]
    fn civil(&mut self, bytes: &[u8]) -> Option<i64> {
        let b: &[u8; 19] = bytes[..19].try_into().unwrap();
        if b[10] != b'T' {
            return None;
        }
        // Time: "HH:MM:SS" as one little-endian word; XOR maps each digit to
        // 0-9 (and only digits) and each ':' to 0.
        let word = u64::from_le_bytes(b[11..19].try_into().unwrap()) ^ 0x3030_3A30_303A_3030;
        if ((word.wrapping_add(0x7676_7676_7676_7676) | word) & 0x8080_8080_8080_8080) != 0
            || word & 0x0000_FF00_00FF_0000 != 0
        {
            return None;
        }
        let lane = |k: u32| ((word >> (8 * k)) & 0xFF) as i64;
        let (hour, minute, second) = (lane(0) * 10 + lane(1), lane(3) * 10 + lane(4), lane(6) * 10 + lane(7));
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let low = u64::from_le_bytes(b[..8].try_into().unwrap());
        let high = u16::from_le_bytes([b[8], b[9]]);
        let slot = ((low ^ (high as u64) << 48).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as usize;
        let days = if self.keys[slot] == (low, high) {
            self.days[slot]
        } else {
            let days = date_days(b)?;
            self.keys[slot] = (low, high);
            self.days[slot] = days;
            days
        };
        Some(((days * 24 + hour) * 60 + minute) * MINUTE + second * SECOND)
    }
}

/// Days since the epoch of a valid `YYYY-MM-DD` in the first 10 bytes.
#[cold]
fn date_days(b: &[u8; 19]) -> Option<i64> {
    // `civil` validates the date and the time; the time was valid already.
    let mut midnight = *b;
    midnight[11..19].copy_from_slice(b"00:00:00");
    civil(&midnight).map(|ms| ms.div_euclid(DAY))
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

    /// Values of 19+ significant digits are outside every window either way.
    fn normalized(value: Option<i64>) -> Option<i64> {
        value.map(|v| if v >= 1_000_000_000_000_000_000 { i64::MAX } else { v })
    }

    /// The fast parser agrees with the plain one on random near-valid input.
    #[test]
    fn fast_matches_simple() {
        let mut state = 0xDEAD_BEEF_1234_5678u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let noise = b"0123456789-+:TZtz .eE\r";
        let mut cached = Parser::default();
        for round in 0..400_000 {
            let mut text: Vec<u8> = match next() % 4 {
                0 | 1 => {
                    let year = match next() % 4 {
                        0 => next() % 10_000,
                        _ => 1890 + next() % 330,
                    };
                    let mut t = format!(
                        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                        year,
                        next() % 14,
                        next() % 33,
                        next() % 26,
                        next() % 62,
                        next() % 62
                    )
                    .into_bytes();
                    if next() % 2 == 0 {
                        t.push(b'Z');
                    } else {
                        let sign = if next() % 2 == 0 { "+" } else { "-" };
                        t.extend(format!("{sign}{:02}:{:02}", next() % 26, next() % 62).bytes());
                    }
                    t
                }
                2 => {
                    let zeros = "0".repeat((next() % 6) as usize);
                    format!("{zeros}{}", next() % 10u64.pow((next() % 20) as u32 + 1)).into_bytes()
                }
                _ => (0..next() % 30).map(|_| noise[(next() % noise.len() as u64) as usize]).collect(),
            };
            if next() % 4 == 0 && !text.is_empty() {
                let at = (next() % text.len() as u64) as usize;
                text[at] = noise[(next() % noise.len() as u64) as usize];
            }
            assert_eq!(
                normalized(parse(&text)),
                normalized(parse_simple(&text)),
                "round {round}: {:?}",
                String::from_utf8_lossy(&text)
            );
            // The cached parser, cold and warm (the same text twice).
            for _ in 0..2 {
                assert_eq!(
                    normalized(cached.parse(&text)),
                    normalized(parse_simple(&text)),
                    "cached, round {round}: {:?}",
                    String::from_utf8_lossy(&text)
                );
            }
        }
        for text in [&b"99999999999999999999999"[..], b"0000000000000000000000001791071890000", b"00000000"] {
            assert_eq!(normalized(parse(text)), normalized(parse_simple(text)));
        }
    }

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
