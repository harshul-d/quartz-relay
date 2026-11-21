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
    if day < 1 || day > month_length(year, month) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * MINUTE + second * SECOND)
}

/// Days in a month (1 to 12) of a proleptic Gregorian year.
#[inline(always)]
fn month_length(year: i64, month: i64) -> i64 {
    match month {
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
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

/// The hot parser; same results as `parse` for every input.
///
/// Default (AVX2 builds): F1/F2 check all 20 or 25 bytes at once with two
/// overlapping 16-byte loads that stay inside the field (`simd::civil`), the
/// date from the month-start table; F3 of 8 to 16 digits is validated and
/// converted with SSE (`simd::epoch_millis`); other lengths take the scalar
/// code. `PIPELINE_TS=old` (A/B runs) selects the previous path instead: a
/// per-thread cache of recently seen dates (a window spans a few dozen
/// distinct `YYYY-MM-DD`s, so the date part is validated and converted once
/// per distinct date, keyed by its exact 10 bytes) and a SWAR time.
pub struct Parser {
    /// `PIPELINE_TS=old`: the date-cache path.
    old: bool,
    /// (first 8 bytes, bytes 8-9) of a valid date; zero (impossible: the
    /// bytes are ASCII digits and dashes) marks an empty entry.
    keys: Vec<(u64, u16)>,
    days: Vec<i64>,
}

const DATE_CACHE: usize = 256;

/// `PIPELINE_TS=old` selects the date-cache parser (read once per process).
fn old_parser() -> bool {
    static OLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OLD.get_or_init(|| std::env::var_os("PIPELINE_TS").is_some_and(|v| v == "old"))
}

impl Default for Parser {
    fn default() -> Parser {
        Parser { old: old_parser(), keys: vec![(0, 0); DATE_CACHE], days: vec![0; DATE_CACHE] }
    }
}

impl Parser {
    #[inline(always)]
    pub fn parse(&mut self, bytes: &[u8]) -> Option<i64> {
        if self.old { self.parse_old(bytes) } else { parse_new(bytes) }
    }

    /// The date-cache path (`PIPELINE_TS=old`).
    #[inline(always)]
    fn parse_old(&mut self, bytes: &[u8]) -> Option<i64> {
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

/// The default `Parser` path: `parse` with SIMD for F1, F2 and F3 of 8 to 16
/// digits. The dispatch on length and zone bytes is exactly `parse`'s.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
fn parse_new(bytes: &[u8]) -> Option<i64> {
    match bytes.len() {
        20 if bytes[19] == b'Z' => simd::civil(bytes, &simd::F1).map(|(local, _)| local),
        25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => {
            let (local, offset_minutes) = simd::civil(bytes, &simd::F2)?;
            let offset = offset_minutes * MINUTE;
            Some(if bytes[19] == b'+' { local - offset } else { local + offset })
        }
        8..=16 => simd::epoch_millis(bytes),
        0 => None,
        _ => epoch_millis_fast(bytes),
    }
}

/// Without AVX2 the default path is the scalar `parse`.
#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
#[inline(always)]
fn parse_new(bytes: &[u8]) -> Option<i64> {
    parse(bytes)
}

/// Days since the epoch of `year-month-day`, `None` if the day is past the
/// end of the month or the year is 0; `month` is 1 to 12 and `day` 1 to 31.
#[cfg_attr(not(all(target_arch = "x86_64", target_feature = "avx2")), allow(dead_code))]
#[inline(always)]
fn days_of(year: u32, month: u32, day: u32) -> Option<i64> {
    let year_index = year.wrapping_sub(TABLE_FIRST_YEAR) as usize;
    if year_index >= TABLE_YEARS {
        return days_slow(year as i64, month as i64, day as i64);
    }
    let index = year_index * 12 + month as usize - 1;
    let start = MONTH_START[index];
    if day as i32 > MONTH_START[index + 1] - start {
        return None; // Feb 30, Feb 29 outside a leap year, Apr 31, ...
    }
    Some((start + day as i32 - 1) as i64)
}

/// `days_of` for years outside the table, with `civil`'s checks.
#[cfg_attr(not(all(target_arch = "x86_64", target_feature = "avx2")), allow(dead_code))]
#[cold]
fn days_slow(year: i64, month: i64, day: i64) -> Option<i64> {
    if year < 1 || day > month_length(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
mod simd {
    use super::{MINUTE, SECOND, days_of};
    use std::arch::x86_64::*;

    /// How the 32 bytes `field[0..16] ++ field[len - 16..len]` of an F1 or F2
    /// field look. Each byte is checked as `(byte ^ template) <= limit`
    /// (unsigned): a digit has template `'0'` and limit 9, a literal has
    /// itself and 0, a byte checked elsewhere 0 and 255. `gather` collects
    /// the digit pairs of each 16-byte lane (-1 gives 0).
    pub struct Form {
        template: [u8; 32],
        limit: [u8; 32],
        gather: [i8; 32],
    }

    /// `d` a digit, `?` any byte, anything else itself.
    const fn form(pattern: &[u8; 32], gather_high: [i8; 16]) -> Form {
        let mut template = [0u8; 32];
        let mut limit = [0u8; 32];
        let mut gather = [-1i8; 32];
        let mut i = 0;
        while i < 32 {
            (template[i], limit[i]) = match pattern[i] {
                b'd' => (b'0', 9),
                b'?' => (0, 255),
                literal => (literal, 0),
            };
            i += 1;
        }
        // Low lane (bytes 0-15): YYYY, MM, DD, HH, MM.
        let low: [i8; 12] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15];
        let mut k = 0;
        while k < 12 {
            gather[k] = low[k];
            k += 1;
        }
        let mut k = 0;
        while k < 16 {
            gather[16 + k] = gather_high[k];
            k += 1;
        }
        Form { template, limit, gather }
    }

    const NONE: i8 = -1;

    /// F1 (20 bytes): the high lane is bytes 4-19; seconds at 13-14.
    pub const F1: Form = form(
        b"dddd-dd-ddTdd:dd-dd-ddTdd:dd:ddZ",
        [13, 14, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE],
    );

    /// F2 (25 bytes): the high lane is bytes 9-24; seconds at 8-9, offset
    /// hours at 11-12 and minutes at 14-15. The sign (byte 19) is checked by
    /// the caller.
    pub const F2: Form = form(
        b"dddd-dd-ddTdd:dddTdd:dd:dd?dd:dd",
        [8, 9, 11, 12, 14, 15, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE],
    );

    #[inline(always)]
    fn load(bytes: &[u8; 32]) -> __m256i {
        // SAFETY: reads exactly the 32 bytes of the array.
        unsafe { _mm256_loadu_si256(bytes.as_ptr() as *const __m256i) }
    }

    /// `YYYY-MM-DDTHH:MM:SS` plus the zone of `form` (20 or 25 bytes, the
    /// caller checked the length): the UTC milliseconds of the local time
    /// and, for F2, the offset in minutes (without its sign); `None` if any
    /// byte or calendar value is bad.
    #[inline(always)]
    pub fn civil(bytes: &[u8], form: &Form) -> Option<(i64, i64)> {
        debug_assert!(bytes.len() == 20 || bytes.len() == 25);
        // SAFETY: AVX2 is enabled at compile time; both loads lie inside
        // `bytes`, which holds at least 20 bytes.
        let (date, rest) = unsafe {
            let low = _mm_loadu_si128(bytes.as_ptr() as *const __m128i);
            let high = _mm_loadu_si128(bytes.as_ptr().add(bytes.len() - 16) as *const __m128i);
            let digits = _mm256_xor_si256(_mm256_set_m128i(high, low), load(&form.template));
            let limit = load(&form.limit);
            let bytes_ok = _mm256_cmpeq_epi8(_mm256_max_epu8(digits, limit), limit);
            // Two-digit values as 16-bit lanes: [YY, YY, MM, DD, HH, MM, 0, 0]
            // and [SS, offset HH, offset MM, 0, ...].
            let gather = _mm256_loadu_si256(form.gather.as_ptr() as *const __m256i);
            let pairs = _mm256_maddubs_epi16(_mm256_shuffle_epi8(digits, gather), _mm256_set1_epi16(0x010A));
            let most = _mm256_setr_epi16(99, 99, 12, 31, 23, 59, 0, 0, 59, 23, 59, 0, 0, 0, 0, 0);
            let least = _mm256_setr_epi16(0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
            let out_of_range = _mm256_or_si256(_mm256_cmpgt_epi16(pairs, most), _mm256_cmpgt_epi16(least, pairs));
            if _mm256_movemask_epi8(_mm256_andnot_si256(out_of_range, bytes_ok)) != -1 {
                return None;
            }
            let packed = _mm256_packus_epi16(pairs, pairs);
            (_mm256_extract_epi64::<0>(packed) as u64, _mm256_extract_epi64::<2>(packed) as u64)
        };
        let field = |word: u64, k: u32| ((word >> (8 * k)) & 0xFF) as u32;
        let year = field(date, 0) * 100 + field(date, 1);
        let days = days_of(year, field(date, 2), field(date, 3))?;
        let (hour, minute, second) = (field(date, 4) as i64, field(date, 5) as i64, field(rest, 0) as i64);
        let local = ((days * 24 + hour) * 60 + minute) * MINUTE + second * SECOND;
        Some((local, field(rest, 1) as i64 * 60 + field(rest, 2) as i64))
    }

    const POW10: [u64; 9] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000];

    /// F3 of 8 to 16 bytes: the first 8 bytes and the last 8 (whose bytes
    /// shared with the first are replaced by `'0'`) in one SSE register,
    /// checked and converted to two 8-digit values at once.
    #[inline(always)]
    pub fn epoch_millis(bytes: &[u8]) -> Option<i64> {
        let length = bytes.len();
        debug_assert!((8..=16).contains(&length));
        let first = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let last = u64::from_le_bytes(bytes[length - 8..].try_into().unwrap());
        let shared = (16 - length) as u32; // leading bytes of `last` also in `first`
        let keep = u64::MAX.checked_shl(8 * shared).unwrap_or(0);
        let last = (last & keep) | (0x3030_3030_3030_3030 & !keep);
        // SAFETY: SSE4.1 and SSSE3 are enabled at compile time (AVX2 implies them).
        let halves = unsafe {
            let digits = _mm_sub_epi8(_mm_set_epi64x(last as i64, first as i64), _mm_set1_epi8(b'0' as i8));
            let nine = _mm_set1_epi8(9);
            if _mm_movemask_epi8(_mm_cmpeq_epi8(_mm_max_epu8(digits, nine), nine)) != 0xFFFF {
                return None;
            }
            let pairs = _mm_maddubs_epi16(digits, _mm_set1_epi16(0x010A)); // 2-digit u16s
            let quads = _mm_madd_epi16(pairs, _mm_set1_epi32(0x0001_0064)); // 4-digit u32s
            let quads = _mm_packus_epi32(quads, quads);
            let octs = _mm_madd_epi16(quads, _mm_set1_epi32(0x0001_2710)); // 8-digit u32s
            _mm_cvtsi128_si64(octs) as u64
        };
        Some(((halves & 0xFFFF_FFFF) * POW10[length - 8] + (halves >> 32)) as i64)
    }
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
                    // Up to 19 digits (10^19 still fits in a u64).
                    format!("{zeros}{}", next() % 10u64.pow((next() % 19) as u32 + 1)).into_bytes()
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

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    /// `parse_new` (also on the text inside larger buffers, at the start,
    /// middle and end), the date-cache path (cold and warm) and `parse_simple`
    /// all agree with `parse` on `text`. Returns `parse(text)`.
    fn agree(cached: &mut Parser, text: &[u8]) -> Option<i64> {
        let show = || String::from_utf8_lossy(text).into_owned();
        let expected = parse(text);
        assert_eq!(normalized(parse_simple(text)), normalized(expected), "simple: {:?}", show());
        assert_eq!(parse_new(text), expected, "new: {:?}", show());
        for _ in 0..2 {
            assert_eq!(cached.parse_old(text), expected, "old: {:?}", show());
        }
        if text.len() <= 40 {
            for fill in [b'9', b':', b'0', 0xFF] {
                let mut buffer = [fill; 96];
                for at in [0, 37, 96 - text.len()] {
                    buffer[at..at + text.len()].copy_from_slice(text);
                    assert_eq!(parse_new(&buffer[at..at + text.len()]), expected, "new at {at}: {:?}", show());
                    buffer[at..at + text.len()].fill(fill);
                }
            }
        }
        expected
    }

    fn civil_text(year: u64, month: u64, day: u64, time: &str, zone: &str) -> Vec<u8> {
        format!("{year:04}-{month:02}-{day:02}T{time}{zone}").into_bytes()
    }

    /// Every edge of the calendar, the clock and the offset, valid or not.
    #[test]
    fn new_matches_reference_on_edges() {
        let mut cached = Parser::default();
        let years = [0, 1, 4, 100, 400, 1899, 1900, 1969, 1970, 2000, 2024, 2026, 2028, 2100, 2199, 2200, 2400, 9999];
        let zones = ["Z", "+23:59", "-23:59", "-00:00", "+00:00", "+01:30"];
        let mut valid = 0;
        for year in years {
            for month in 0..=13 {
                for day in 0..=32 {
                    for zone in zones {
                        valid += agree(&mut cached, &civil_text(year, month, day, "23:59:59", zone)).is_some() as u32;
                    }
                }
            }
        }
        // 17 valid years x 366 or 365 days x 6 zones.
        assert!(valid > 17 * 365 * 6, "{valid}");
        let mut zones: Vec<String> = ["Z", "z", " ", "", "+", "-", "Z ", "+0200", "+02:0", "+02:000"].map(String::from).to_vec();
        for sign in ["+", "-", " ", "z"] {
            for hours in ["00", "01", "09", "10", "19", "20", "23", "24", "29", "99", "0a", ":0"] {
                for minutes in ["00", "30", "59", "60", "99", "5/"] {
                    zones.push(format!("{sign}{hours}:{minutes}"));
                    zones.push(format!("{sign}{hours}-{minutes}"));
                }
            }
        }
        let dates = [(2024, 2, 29), (2026, 12, 31), (1970, 1, 1), (2100, 2, 28), (2000, 2, 29), (1, 1, 1), (9999, 12, 31)];
        for (year, month, day) in dates {
            for hour in ["00", "09", "10", "19", "20", "23", "24", "29", "99"] {
                for minute in ["00", "59", "60", "99"] {
                    for second in ["00", "59", "60"] {
                        for zone in &zones {
                            agree(&mut cached, &civil_text(year, month, day, &format!("{hour}:{minute}:{second}"), zone));
                        }
                    }
                }
            }
        }
        let f1 = parse(b"2026-10-03T23:58:10Z").unwrap();
        assert_eq!(parse_new(b"2026-10-04T01:28:10+01:30"), Some(f1));
        assert_eq!(parse_new(b"2026-10-03T23:58:10-00:00"), Some(f1));
        assert_eq!(parse_new(b"2026-10-04T23:57:10+23:59"), Some(f1));
        assert_eq!(parse_new(b"2026-10-03T00:00:10-23:59"), Some(f1 + MINUTE));
        assert_eq!(parse_new(b"2024-02-29T00:00:00Z"), Some(days_from_civil(2024, 2, 29) * DAY));
        assert_eq!(parse_new(b"2100-02-29T00:00:00Z"), None);
        assert!(parse_new(b"2000-02-29T00:00:00Z").is_some());
    }

    /// Every bad form rules.md lists, and the valid ones next to them.
    #[test]
    fn new_matches_reference_on_spec_forms() {
        let mut cached = Parser::default();
        let f1 = parse(b"2026-10-03T23:58:10Z").unwrap();
        for good in [
            &b"2026-10-03T23:58:10Z"[..],
            b"2026-10-03T19:58:10-04:00",
            b"2026-10-04T01:28:10+01:30",
            b"2026-10-03T23:58:10-00:00",
            b"1791071890000",
            b"0001791071890000",
        ] {
            assert_eq!(agree(&mut cached, good), Some(f1), "{:?}", String::from_utf8_lossy(good));
        }
        assert_eq!(agree(&mut cached, b"86400000"), Some(DAY));
        for bad in [
            &b"2026-10-03T23:58:10z"[..],
            b"2026-10-03t23:58:10Z",
            b"2026-10-03 23:58:10Z",
            b"2026-10-03T23:58:10",
            b"2026-10-03T23:58:10.000Z",
            b"2026-10-03T23:58:10.5Z",
            b"2026-10-03T23:58:10.000+01:00",
            b"2026-13-03T23:58:10Z",
            b"2026-00-03T23:58:10Z",
            b"2026-02-30T23:58:10Z",
            b"2026-04-31T23:58:10Z",
            b"2026-10-00T23:58:10Z",
            b"2027-02-29T00:00:00Z",
            b"2100-02-29T00:00:00Z",
            b"2026-10-03T24:00:00Z",
            b"2026-10-03T23:60:00Z",
            b"2026-10-03T23:59:60Z",
            b"2026-10-03T23:58:10+24:00",
            b"2026-10-03T23:58:10-24:00",
            b"2026-10-03T23:58:10+05:60",
            b"2026-10-03T23:58:10+0200",
            b"2026-10-03T23:58:10+02:00Z",
            b"2026-10-03T23:58:10Z+02:00",
            b"0000-01-01T00:00:00Z",
            b"1.79e12",
            b"-5",
            b"-1791071890000",
            b"+1791071890000",
            b"1791071890000.0",
            b"1791071890000 ",
            b" 1791071890000",
            b"",
        ] {
            assert_eq!(agree(&mut cached, bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    /// F3 of 1 to 25 digits (overflowing ones saturate), and each with one
    /// non-digit at every position.
    #[test]
    fn new_matches_reference_on_epoch_millis() {
        let mut cached = Parser::default();
        let mut random = Random(0x1234_5678_9ABC_DEF1);
        let noise = b".eE-+ :/Z\0\xFF\x7F\xB0\xB9";
        for length in 1..=25usize {
            let mut texts: Vec<Vec<u8>> = vec![vec![b'9'; length], vec![b'0'; length]];
            texts.push(std::iter::once(b'1').chain(std::iter::repeat_n(b'0', length - 1)).collect());
            for _ in 0..400 {
                let leading = random.below(length as u64 + 1) as usize;
                texts.push((0..length).map(|k| if k < leading && random.below(2) == 0 { b'0' } else { b'0' + random.below(10) as u8 }).collect());
            }
            for text in texts {
                let value = agree(&mut cached, &text);
                assert!(value.is_some());
                for at in 0..length {
                    let mut bad = text.clone();
                    bad[at] = noise[random.below(noise.len() as u64) as usize];
                    assert_eq!(agree(&mut cached, &bad), None);
                }
            }
        }
        assert_eq!(parse_new(b"9999999999999999"), Some(9_999_999_999_999_999));
        assert_eq!(parse_new(b"0000000000000001"), Some(1));
        assert_eq!(parse_new(b"00000000"), Some(0));
        assert_eq!(parse_new(b"179102160000000000000"), Some(i64::MAX));
    }

    /// Valid F1, F2 and F3 strings with 1 to 3 random bytes replaced, deleted
    /// or inserted.
    #[test]
    fn new_matches_reference_on_mutations() {
        let mut cached = Parser::default();
        let mut random = Random(0x0BAD_C0DE_F00D_1234);
        let noise = b"0123456789-+:TZtz .eE/;";
        let mut valid = 0u32;
        for round in 0..300_000u32 {
            let mut text = match random.below(10) {
                0..=5 | 9 => {
                    let year = if random.below(8) == 0 { random.below(10_000) } else { 1900 + random.below(300) };
                    let month = 1 + random.below(12);
                    let day = 1 + random.below(month_length(year.max(1) as i64, month as i64) as u64);
                    let time = format!("{:02}:{:02}:{:02}", random.below(24), random.below(60), random.below(60));
                    let zone = if random.below(3) == 0 {
                        let sign = if random.below(2) == 0 { '+' } else { '-' };
                        format!("{sign}{:02}:{:02}", random.below(24), random.below(60))
                    } else {
                        "Z".to_string()
                    };
                    civil_text(year, month, day, &time, &zone)
                }
                _ => {
                    let length = if random.below(4) == 0 { 1 + random.below(25) } else { 13 };
                    (0..length).map(|_| b'0' + random.below(10) as u8).collect()
                }
            };
            // Round 0 mod 8 keeps the text valid.
            let mutations = if round % 8 == 0 { 0 } else { 1 + random.below(3) };
            for _ in 0..mutations {
                let byte = if random.below(2) == 0 {
                    random.below(256) as u8
                } else {
                    noise[random.below(noise.len() as u64) as usize]
                };
                let at = random.below(text.len() as u64 + 1) as usize;
                match random.below(4) {
                    0 | 1 if at < text.len() => text[at] = byte,
                    2 if at < text.len() => {
                        text.remove(at);
                    }
                    _ => text.insert(at, byte),
                }
            }
            valid += agree(&mut cached, &text).is_some() as u32;
        }
        assert!(valid > 300_000 / 8, "{valid}");
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
