//! The fast path, for data whose IDs are shaped like the generator's
//! (tools/gen/src/user.rs and text.rs).
//!
//! The generator writes an ID as a letter and lowercase base 36 of
//! `scramble(number, key)`, a bijection on 40 bits whose two multipliers are
//! fixed and whose key is drawn per dataset:
//!   - user `u`: number = person * 512 + alias (shared devices from 2^39);
//!   - event `e`: number = the event's global index, and one person's events
//!     have consecutive indexes, in time order.
//! The keys are recovered from the data at run time (two linked IDs are
//! usually one person's, so their numbers differ by a little; so do two
//! events of one person). Then:
//!   - every event copy goes, as one 20-byte record, to the shard of its
//!     event number, and dedup is an array indexed by number;
//!   - after dedup a shard's events come out in number order, so each
//!     person's events form one run: no hashing to group them.
//!
//! The output is exact for any input. An ID takes the fast path only if it is
//! the prefix and canonical base 36 (lowercase digits, no leading zero, at
//! most 40 bits), which is exactly the set of strings that re-encoding the
//! decoded number reproduces, so a number stands for one string. Everything
//! else, and every person the fast path cannot finish on its own (several
//! persons joined by links, IDs it cannot decode, events of one person in two
//! shards), goes to the general path (`sessionize`) with its name as there.
//! If the keys cannot be recovered, or too few IDs decode, `prepare` returns
//! None and the general pipeline runs from the start.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use super::*;
use crate::csv::{Files, for_each_record};
use crate::key::{FastMap, Interner, Key, Strings, is_long};
use crate::links::{Linked, Names};

// ------------------------------------------------------------------ IDs

const MASK: u64 = (1 << 40) - 1;
const MUL_A: u64 = 0x9E37_79B9_7F4B;
const MUL_B: u64 = 0xD6E8_FEB8_6659;
/// Shared devices' numbers start here (tools/gen/src/user.rs, KIOSK_BASE).
const KIOSK: u64 = 1 << 39;
/// A user number is person << ALIAS_BITS | alias (ALIAS_SLOTS = 512).
const ALIAS_BITS: u32 = 9;

/// The inverse of odd `a` modulo 2^64 by Newton's method: a * a = 1 mod 8,
/// and each step doubles the number of correct low bits.
const fn inverse(a: u64) -> u64 {
    let mut x = a;
    let mut k = 0;
    while k < 5 {
        x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        k += 1;
    }
    x
}
const INV_A: u64 = inverse(MUL_A) & MASK;
const INV_B: u64 = inverse(MUL_B) & MASK;

/// The generator's `scramble` (tools/gen/src/text.rs).
#[cfg_attr(not(test), allow(dead_code))]
#[inline(always)]
fn scramble(value: u64, key: u64) -> u64 {
    let mut mixed = (value.wrapping_mul(MUL_A) ^ key) & MASK;
    mixed ^= mixed >> 17;
    mixed.wrapping_mul(MUL_B) & MASK
}

/// `A * value ^ key` from `scramble(value, key)`: undoes the last multiply and
/// the xor-shift, which need no key.
#[inline(always)]
fn unshuffle(y: u64) -> u64 {
    let x = y.wrapping_mul(INV_B) & MASK;
    x ^ (x >> 17) ^ (x >> 34)
}

#[inline(always)]
fn unscramble(y: u64, key: u64) -> u64 {
    ((unshuffle(y) ^ key) & MASK).wrapping_mul(INV_A) & MASK
}

/// Lowercase base 36 digits (tools/gen/src/text.rs `base36`).
const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// The key (`key::pack_short`) of `prefix` + base 36 of `y`: the string the
/// generator writes for that scrambled value.
fn encode(prefix: u8, mut y: u64) -> Key {
    debug_assert!(y <= MASK);
    let mut digits = [0u8; 8];
    let mut n = 0;
    loop {
        digits[n] = DIGITS[(y % 36) as usize];
        n += 1;
        y /= 36;
        if y == 0 {
            break;
        }
    }
    let mut bytes = [0u8; 16];
    bytes[0] = prefix;
    for at in 0..n {
        bytes[1 + at] = digits[n - 1 - at];
    }
    bytes[15] = (n + 1) as u8;
    u128::from_be_bytes(bytes)
}

/// Canonical base 36: 1 to 8 digits `0-9a-z`, no leading zero (except "0"
/// itself), at most 40 bits. Exactly the strings `encode` writes.
fn decode_digits(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || digits.len() > 8 || (digits.len() > 1 && digits[0] == b'0') {
        return None;
    }
    let mut value = 0u64;
    for &c in digits {
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'z' => c - b'a' + 10,
            _ => return None,
        };
        value = value * 36 + digit as u64;
    }
    (value <= MASK).then_some(value)
}

/// `field` as `prefix` + canonical base 36, else None.
fn decode_bytes(field: &[u8], prefix: u8) -> Option<u64> {
    if field.first() != Some(&prefix) {
        return None;
    }
    decode_digits(&field[1..])
}

/// The bytes of a short key, else None.
fn key_bytes(key: Key) -> Option<([u8; 16], usize)> {
    if is_long(key) {
        return None;
    }
    let bytes = key.to_be_bytes();
    Some((bytes, bytes[15] as usize))
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// `decode_bytes(field, prefix)` for a `field` inside `chunk`: the 8 bytes
/// ending at the field's end are decoded at once (SWAR).
#[inline(always)]
fn decode_in(chunk: &[u8], field: &[u8], prefix: u8) -> Option<u64> {
    let n = field.len().wrapping_sub(1); // digits
    if n.wrapping_sub(1) >= 8 || field[0] != prefix {
        return None;
    }
    let end = (field.as_ptr() as usize).wrapping_sub(chunk.as_ptr() as usize) + field.len();
    debug_assert!(end <= chunk.len());
    if end < 8 {
        return decode_digits(&field[1..]);
    }
    let word = u64::from_le_bytes(chunk[end - 8..end].try_into().unwrap());
    decode_word(word, n)
}

/// The last `n` (1 to 8) bytes of the little-endian `word` as canonical base 36.
#[inline(always)]
fn decode_word(word: u64, n: usize) -> Option<u64> {
    // The digits are the top n bytes, the most significant lowest; pad the
    // rest with '0' digits.
    let keep = !0u64 << (8 * (8 - n));
    let w = (word & keep) | (0x30 * ONES & !keep);
    if w & HIGH != 0 {
        return None;
    }
    // Per byte b < 0x80: b + (0x80 - lo) has its top bit set iff b >= lo.
    let at_least = |lo: u64| w.wrapping_add((0x80 - lo) * ONES) & HIGH;
    let letter = at_least(0x61) & !at_least(0x7b);
    if (at_least(0x30) & !at_least(0x3a)) | letter != HIGH {
        return None;
    }
    if n > 1 && (w >> (8 * (8 - n))) as u8 == b'0' {
        return None;
    }
    let v = w - 0x30 * ONES - (letter >> 7) * 0x27; // digit values 0..=35
    let pairs = (v & 0x00ff_00ff_00ff_00ff) * 36 + ((v >> 8) & 0x00ff_00ff_00ff_00ff);
    let quads = (pairs & 0x0000_ffff_0000_ffff) * 1296 + ((pairs >> 16) & 0x0000_ffff_0000_ffff);
    let value = (quads & 0xffff_ffff) * 1_679_616 + (quads >> 32);
    (value <= MASK).then_some(value)
}

/// A user number (person << 9 | alias) the fast path handles: an ordinary
/// person's alias with person below `persons`.
#[inline(always)]
fn dense_user(y: Option<u64>, user_key: u64, persons: u64) -> Option<u32> {
    let v = unscramble(y?, user_key);
    (v < KIOSK && (v >> ALIAS_BITS) < persons).then_some(v as u32)
}

// --------------------------------------------------------- key recovery

/// Key bits implied by x1 = A*v1 ^ key and x2 = A*v2 ^ key when v1 - v2 = d
/// (mod 2^40): (mask of the bits it fixes, their values), or None if no key
/// fits. With a = x1 ^ key and b = x2 ^ key, a - b must be T = A*d. The
/// borrow into bit k of a - b is then T_k ^ e_k (e = x1 ^ x2, which needs no
/// key). Where x1 and x2 agree the borrow passes through unchanged (a check);
/// where they differ, the borrow out of bit k is set iff a_k = 0, i.e. iff
/// key_k = x1_k.
#[inline(always)]
fn pair_bits(x1: u64, x2: u64, d: u64) -> Option<(u64, u64)> {
    const LOW39: u64 = (1 << 39) - 1;
    let t = d.wrapping_mul(MUL_A) & MASK;
    let e = x1 ^ x2;
    let c = e ^ t; // borrow into each bit
    if c & 1 != 0 || ((c >> 1) ^ c) & !e & LOW39 != 0 {
        return None;
    }
    let m = e & LOW39;
    Some((m, (x1 ^ !(c >> 1)) & m))
}

/// The key under which pairs (y1, y2) of scrambled values decode to numbers
/// at most `most` apart, best by `score`, with its score; or None.
///
/// One pair cannot tell which of its two numbers is larger: d and -d both
/// fit, with complementary bits. So bits are grown from a seed pair, under
/// each of its two readings: a pair joins when exactly one of its readings
/// agrees with the bits known so far on at least 6 of them. The bits no pair
/// fixed (bit 39 at least) are tried both ways.
fn solve_key(pairs: &[(u64, u64)], most: u64, score: impl Fn(u64) -> f64) -> Option<(u64, f64)> {
    let candidates: Vec<Vec<(u64, u64)>> = pairs
        .iter()
        .filter_map(|&(y1, y2)| {
            let (x1, x2) = (unshuffle(y1), unshuffle(y2));
            if x1 == x2 {
                return None;
            }
            let fits: Vec<(u64, u64)> = (1..=most)
                .flat_map(|d| [d, d.wrapping_neg() & MASK])
                .filter_map(|d| pair_bits(x1, x2, d))
                .collect();
            (!fits.is_empty()).then_some(fits)
        })
        .collect();
    let mut best: Option<(u64, f64)> = None;
    for seed in candidates.iter().filter(|c| c.len() == 2).take(8) {
        for &(seed_mask, seed_bits) in seed {
            let (mut known, mut bits) = (seed_mask, seed_bits);
            loop {
                let mut changed = false;
                for fits in &candidates {
                    let mut pick: Option<(u64, u64)> = None;
                    let mut conflict = false;
                    for &(m, b) in fits {
                        let overlap = m & known;
                        if overlap.count_ones() < 6 || (b ^ bits) & overlap != 0 {
                            continue;
                        }
                        match pick {
                            None => pick = Some((m, b)),
                            Some(x) => conflict |= x != (m, b),
                        }
                    }
                    if let (Some((m, b)), false) = (pick, conflict) {
                        if m & !known != 0 {
                            known |= m;
                            bits |= b;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            let free: Vec<u32> = (0..40).filter(|&k| known >> k & 1 == 0).collect();
            if free.len() > 10 {
                continue;
            }
            for combo in 0u64..1 << free.len() {
                let mut key = bits;
                for (j, &bit) in free.iter().enumerate() {
                    if combo >> j & 1 == 1 {
                        key |= 1 << bit;
                    }
                }
                let s = score(key);
                if s >= 0.99 {
                    return Some((key, s)); // a wrong key scores near zero
                }
                if best.is_none_or(|(_, b)| s > b) {
                    best = Some((key, s));
                }
            }
        }
    }
    best
}

/// A key is accepted when this share of sampled IDs decodes under it (a
/// wrong key decodes almost none).
const KEY_SCORE: f64 = 0.5;
/// The fast path runs only when at least this share of sampled event IDs and
/// user IDs decode; otherwise most rows would take the general path anyway.
const DECODED_SHARE: f64 = 0.5;

struct Keys {
    user: u64,
    event: u64,
    /// Bounds on person and event numbers, for the dense arrays.
    persons: u64,
    events: u64,
    rows_estimate: usize,
}

/// Recovers both keys from samples of the link pairs and the event files.
fn recover_keys(files: &Files, chunks: &[csv::Chunk], linked: &Linked) -> Result<Keys, String> {
    // Event rows from the start of up to 96 chunks across the files.
    let step = (chunks.len() / 96).max(1);
    let mut sample: Vec<(Option<u64>, Option<u64>)> = Vec::new();
    let mut sampled_bytes = 0usize;
    for chunk in chunks.iter().step_by(step) {
        let head = files.head(chunk, 64 << 10);
        sampled_bytes += head.len();
        for_each_record::<7>(&head, |fields| {
            sample.push((decode_bytes(fields[0], b'e'), decode_bytes(fields[1], b'u')));
        });
    }
    if sample.is_empty() {
        return Err("no event rows sampled".into());
    }
    let rows_estimate = (files.total_bytes() as f64 * sample.len() as f64 / sampled_bytes.max(1) as f64) as usize;
    // At least 30 events per user (tools/gen/src/plan.rs), so this leaves room.
    let persons = ((rows_estimate / 20 + 4096) as u64).min(1 << 22);
    let events = (rows_estimate + rows_estimate / 4 + 4096) as u64;
    if events >= 1 << 31 {
        return Err("too many events for 32-bit numbers".into());
    }

    let link_pairs: Vec<(u64, u64)> = linked
        .sample
        .iter()
        .filter_map(|&(a, b)| {
            let decode = |key: Key| key_bytes(key).and_then(|(bytes, n)| decode_bytes(&bytes[..n], b'u'));
            let (a, b) = (decode(a)?, decode(b)?);
            (a != b).then_some((a, b))
        })
        .take(4096)
        .collect();
    // Scored on at most 2048 IDs: every bit no pair fixes doubles the candidates.
    let users: Vec<u64> = link_pairs
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .chain(sample.iter().filter_map(|s| s.1))
        .step_by(7)
        .take(2048)
        .collect();
    let user_score = |key: u64| {
        let fits = users
            .iter()
            .filter(|&&y| {
                let v = unscramble(y, key);
                (v < KIOSK && (v >> ALIAS_BITS) < persons) || (v >= KIOSK && v - KIOSK < 4096)
            })
            .count();
        fits as f64 / users.len().max(1) as f64
    };
    let Some((user, score)) = solve_key(&link_pairs, 8, user_score) else {
        return Err(format!("no user key from {} link pairs", link_pairs.len()));
    };
    if score < KEY_SCORE {
        return Err(format!("user key scores {score:.3}"));
    }

    // Two events of one ordinary person have numbers less than their event
    // count apart. People seen more than 4 times in the sample are skipped:
    // the busiest, whose numbers are far apart.
    let mut by_person: Vec<(u64, u64)> =
        sample.iter().filter_map(|&(e, u)| Some((unscramble(u?, user) >> ALIAS_BITS, e?))).collect();
    by_person.sort_unstable();
    let mut pairs = Vec::new();
    let mut k = 0;
    while k < by_person.len() {
        let mut r = k + 1;
        while r < by_person.len() && by_person[r].0 == by_person[k].0 {
            r += 1;
        }
        if r - k <= 4 {
            for j in k + 1..r {
                if by_person[j - 1].1 != by_person[j].1 {
                    pairs.push((by_person[j - 1].1, by_person[j].1));
                }
            }
        }
        k = r;
    }
    let event_ids: Vec<u64> = sample.iter().filter_map(|s| s.0).step_by(7).take(2048).collect();
    let event_score =
        |key: u64| event_ids.iter().filter(|&&y| unscramble(y, key) < events).count() as f64 / event_ids.len().max(1) as f64;
    let Some((event, score)) = solve_key(&pairs, 64, event_score) else {
        return Err(format!("no event key from {} event pairs", pairs.len()));
    };
    if score < KEY_SCORE {
        return Err(format!("event key scores {score:.3}"));
    }

    // How much of the data the fast path would see.
    let decoded_events =
        sample.iter().filter(|s| s.0.is_some_and(|y| unscramble(y, event) < events)).count() as f64 / sample.len() as f64;
    let decoded_users =
        sample.iter().filter(|s| dense_user(s.1, user, persons).is_some()).count() as f64 / sample.len() as f64;
    if decoded_events < DECODED_SHARE || decoded_users < DECODED_SHARE {
        return Err(format!("only {decoded_events:.3} of event IDs and {decoded_users:.3} of user IDs decode"));
    }
    Ok(Keys { user, event, persons, events, rows_estimate })
}

// -------------------------------------------------------------- records

/// Event numbers per shard: dedup's array (16 bytes a slot) stays in L2.
const SHARD_BITS: u32 = 16;

/// One event copy, in its number's shard. `ts` is ms after window start, or
/// NO_TS for a copy that failed validation (it still takes part in dedup).
#[derive(Clone, Copy)]
#[repr(C)]
struct EventRecord {
    number: u32,
    /// Seconds after `ingest_base`; u32::MAX when ingest_ts does not parse.
    ingest: u32,
    ts: u32,
    /// A dense user number, or OTHER | thread << OTHER_SHIFT | index into
    /// that thread's `others`, or NO_USER.
    user: u32,
    /// price | category << 24 for a purchase, 0 otherwise.
    info: u32,
}

/// An event copy whose event_id does not decode: deduplicated by key.
#[derive(Clone, Copy)]
struct OddEvent {
    event_id: Key,
    ingest: u32,
    ts: u32,
    user: u32,
    info: u32,
}

/// A surviving event, in its shard's number order.
#[derive(Clone, Copy)]
struct Survivor {
    ts: u32,
    user: u32,
    info: u32,
}

/// A valid click of a user whose person is finished in one shard.
#[derive(Clone, Copy)]
struct LocalClick {
    click_id: Key,
    ts: i64,
    user: u32,
    campaign: u32,
}

const NO_TS: u32 = u32::MAX;
const NO_USER: u32 = u32::MAX;
const OTHER: u32 = 1 << 31;
const OTHER_SHIFT: u32 = 26;
const MOST_THREADS: usize = 32;
/// `home` values besides a shard number: no surviving event, or events in
/// several shards or several runs.
const HOME_NONE: u32 = u32::MAX;
const HOME_MULTI: u32 = u32::MAX - 1;
/// `link_root` values besides a root: no linked alias, or not simple.
const LINK_NONE: u32 = u32::MAX;
const LINK_COMPLEX: u32 = u32::MAX - 1;
/// A person key that is a link root (else a user number).
const ROOT: u64 = 1 << 63;

/// Everything the first stage leaves for `finish`.
pub struct Prepared {
    keys: Keys,
    /// Per shard, its survivors in number order.
    survivors: Vec<Mutex<Vec<Survivor>>>,
    /// Survivors whose event_id does not decode.
    odd: Vec<Survivor>,
    /// Per parsing thread, the keys of user IDs that do not decode.
    others: Vec<Vec<Key>>,
    /// Per person: the shard of their surviving events, or HOME_*.
    home: Vec<AtomicU32>,
    /// Per person: the root of their linked aliases, or LINK_*.
    link_root: Vec<u32>,
    /// Per person with a root: which aliases (all below 64) are linked.
    link_aliases: Vec<u64>,
    /// Linked IDs by dense id (for root names).
    linked_keys: Vec<Key>,
    /// `[thread][shard]` clicks of persons finished in a shard.
    local_clicks: Vec<Vec<Vec<LocalClick>>>,
    /// Clicks for the general path, `[partition][thread]`.
    general_clicks: Partitioned<PersonClick>,
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    rows: usize,
    odd_rows: usize,
    other_users: usize,
    clicks_local: usize,
    clicks_general: usize,
}

fn transpose_shards<T>(states: Vec<Vec<Vec<T>>>, shards: usize) -> Vec<Mutex<Vec<Vec<T>>>> {
    let mut by_shard: Vec<Vec<Vec<T>>> = (0..shards).map(|_| Vec::with_capacity(states.len())).collect();
    for state in states {
        for (shard, buffer) in state.into_iter().enumerate() {
            by_shard[shard].push(buffer);
        }
    }
    by_shard.into_iter().map(Mutex::new).collect()
}

fn off(reason: &str) -> Option<Prepared> {
    eprintln!("fast-path off: {reason}");
    None
}

// ---------------------------------------------------------------- stage 1

/// Recovers the keys, then parses and deduplicates the events and routes the
/// clicks. None when the fast path cannot be used (nothing is kept then).
pub fn prepare(
    data: &Path,
    interner: &Interner,
    names: &Names,
    linked: Linked,
    lookups: &Lookups,
    window: &Window,
    threads: usize,
) -> Option<Prepared> {
    if std::env::var_os("PIPELINE_FAST").is_some_and(|v| v == "0") {
        return off("disabled");
    }
    if threads > MOST_THREADS {
        return off("too many threads");
    }
    if window.end <= window.start || window.end - window.start >= NO_TS as i64 {
        return off("window does not fit 32-bit offsets");
    }
    if lookups.categories.len() > 255 {
        return off("too many categories");
    }
    let begin = Instant::now();
    let files = Files::open_folder(&data.join("events"));
    let chunks = files.chunks(chunk_bytes());
    let keys = match recover_keys(&files, &chunks, &linked) {
        Ok(keys) => keys,
        Err(reason) => return off(&reason),
    };
    eprintln!("fast-path keys user=0x{:x} event=0x{:x}", keys.user, keys.event);
    eprintln!(
        "fast-path: ~{} event rows, {} persons, {} event numbers, keys in {:.3}s",
        keys.rows_estimate,
        keys.persons,
        keys.events,
        begin.elapsed().as_secs_f64()
    );
    let persons = keys.persons as usize;

    // ---- Links: which persons are simple (their linked aliases form one
    // set holding nothing else).
    let (link_root, link_aliases) = classify_links(&linked, &keys, threads);
    let linked_keys = linked.keys;

    // ---- Events: every copy into its number's shard.
    let shards = ((keys.events >> SHARD_BITS) + 1) as usize;
    let capacity = keys.rows_estimate / shards / threads.max(1) * 5 / 4 + 64;
    let ingest_base = window.start.div_euclid(1000) - (1 << 31);
    struct EventState {
        thread: usize,
        shards: Vec<Vec<EventRecord>>,
        odd: Vec<OddEvent>,
        others: Vec<Key>,
        dates: ts::Parser,
        rows: usize,
        abort: Option<&'static str>,
    }
    let states = par::run(
        threads,
        chunks.len(),
        |thread| EventState {
            thread,
            shards: (0..shards).map(|_| Vec::with_capacity(capacity)).collect(),
            odd: Vec::new(),
            others: Vec::new(),
            dates: ts::Parser::default(),
            rows: 0,
            abort: None,
        },
        |state, index| {
            files.with_chunk(&chunks[index], |bytes| {
                for_each_record::<7>(bytes, |&[event_id, user, ts_text, ingest_text, event_type, product, price_text]| {
                    if event_id.is_empty() {
                        return;
                    }
                    state.rows += 1;
                    let ingest_ms = state.dates.parse(ingest_text);
                    let ingest = match ingest_ms {
                        // Sorts last, as i64::MAX does in `parse_events`.
                        None => u32::MAX,
                        Some(ms) => {
                            let seconds = ms.div_euclid(1000) - ingest_base;
                            if ms.rem_euclid(1000) != 0 || !(0..u32::MAX as i64).contains(&seconds) {
                                state.abort = Some("an ingest_ts that is not whole seconds near the window");
                                return;
                            }
                            seconds as u32
                        }
                    };
                    let ingest_ms = ingest_ms.unwrap_or(i64::MAX);
                    let (mut ts_offset, mut user_ref, mut info) = (NO_TS, NO_USER, 0u32);
                    // Events 3-4, exactly as `parse_events`.
                    'validate: {
                        if user.is_empty() || product.is_empty() {
                            break 'validate;
                        }
                        let Some(ts) = state.dates.parse(ts_text) else { break 'validate };
                        if ts < window.start || ts >= window.end {
                            break 'validate;
                        }
                        let purchase = match event_type {
                            b"view" | b"cart" => false,
                            b"purchase" => true,
                            _ => break 'validate,
                        };
                        let mut row_info = 0;
                        if purchase {
                            let Some(price) = ts::price(price_text) else { break 'validate };
                            let category =
                                lookups.products.get(product, || interner.key_in(bytes, product)).unwrap_or(lookups.unknown);
                            row_info = price | (category as u32) << 24;
                        }
                        if ingest_ms.saturating_sub(ts) > LATE_LIMIT {
                            break 'validate;
                        }
                        user_ref = match dense_user(decode_in(bytes, user, b'u'), keys.user, keys.persons) {
                            Some(v) => v,
                            None => {
                                if state.others.len() >= (1 << OTHER_SHIFT) - 1 {
                                    state.abort = Some("too many user IDs that do not decode");
                                    return;
                                }
                                state.others.push(interner.key_in(bytes, user));
                                OTHER | (state.thread as u32) << OTHER_SHIFT | (state.others.len() - 1) as u32
                            }
                        };
                        ts_offset = (ts - window.start) as u32;
                        info = row_info;
                    }
                    let number = decode_in(bytes, event_id, b'e').map(|y| unscramble(y, keys.event)).filter(|&i| i < keys.events);
                    match number {
                        Some(number) => state.shards[(number >> SHARD_BITS) as usize].push(EventRecord {
                            number: number as u32,
                            ingest,
                            ts: ts_offset,
                            user: user_ref,
                            info,
                        }),
                        None => state.odd.push(OddEvent {
                            event_id: interner.key_in(bytes, event_id),
                            ingest,
                            ts: ts_offset,
                            user: user_ref,
                            info,
                        }),
                    }
                });
            });
        },
    );
    if let Some(reason) = states.iter().find_map(|state| state.abort) {
        return off(reason);
    }
    let mut stats = Stats {
        rows: states.iter().map(|s| s.rows).sum(),
        odd_rows: states.iter().map(|s| s.odd.len()).sum(),
        other_users: states.iter().map(|s| s.others.len()).sum(),
        ..Stats::default()
    };
    let mut others = Vec::with_capacity(states.len());
    let mut odd_copies = Vec::with_capacity(states.len());
    let mut by_thread = Vec::with_capacity(states.len());
    for state in states {
        others.push(state.others);
        odd_copies.push(state.odd);
        by_thread.push(state.shards);
    }
    let copies = transpose_shards(by_thread, shards);
    eprintln!("{:>8.3}s fast-path events parsed", begin.elapsed().as_secs_f64());

    // ---- Dedup (Events 2) per shard by number, then mark where each
    // person's events are. The kept copy of a number is its smallest ingest;
    // copies with equal ingest_ts are identical (rules.md), so either is kept.
    let home: Vec<AtomicU32> = (0..persons).map(|_| AtomicU32::new(HOME_NONE)).collect();
    let mark = |person: u32, shard: u32| {
        if let Err(held) = home[person as usize].compare_exchange(HOME_NONE, shard, Relaxed, Relaxed) {
            if held != HOME_MULTI {
                home[person as usize].store(HOME_MULTI, Relaxed); // in another shard, or another run of this one
            }
        }
    };
    let survivors: Vec<Mutex<Vec<Survivor>>> = (0..shards).map(|_| Mutex::new(Vec::new())).collect();
    par::run(
        threads,
        shards,
        |_| vec![[u32::MAX; 4]; 1 << SHARD_BITS],
        |slots, shard| {
            let buffers = std::mem::take(&mut *copies[shard].lock().unwrap());
            if buffers.iter().all(Vec::is_empty) {
                return;
            }
            for slot in slots.iter_mut() {
                slot[0] = u32::MAX;
            }
            let base = (shard as u32) << SHARD_BITS;
            for buffer in &buffers {
                for copy in buffer {
                    let slot = &mut slots[(copy.number - base) as usize];
                    // An unparsed ingest_ts (u32::MAX) never wins, and as the
                    // kept copy it would be late, so invalid: the same result.
                    if copy.ingest < slot[0] {
                        *slot = [copy.ingest, copy.ts, copy.user, copy.info];
                    }
                }
            }
            drop(buffers);
            let mut kept = Vec::with_capacity(1 << SHARD_BITS);
            for slot in slots.iter() {
                if slot[0] != u32::MAX && slot[1] != NO_TS {
                    kept.push(Survivor { ts: slot[1], user: slot[2], info: slot[3] });
                }
            }
            let mut last = u32::MAX;
            for survivor in &kept {
                if survivor.user & OTHER != 0 {
                    last = u32::MAX;
                    continue;
                }
                let person = survivor.user >> ALIAS_BITS;
                if person != last {
                    last = person;
                    mark(person, shard as u32);
                }
            }
            *survivors[shard].lock().unwrap() = kept;
        },
    );
    drop(copies);
    // Odd event IDs: dedup by key. Their persons all take the general path.
    let mut odd_best: FastMap<Key, OddEvent> = FastMap::default();
    for list in odd_copies {
        for copy in list {
            match odd_best.entry(copy.event_id) {
                std::collections::hash_map::Entry::Occupied(mut held) => {
                    if copy.ingest < held.get().ingest {
                        *held.get_mut() = copy;
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(copy);
                }
            }
        }
    }
    let odd: Vec<Survivor> = odd_best
        .values()
        .filter(|copy| copy.ingest != u32::MAX && copy.ts != NO_TS)
        .map(|copy| Survivor { ts: copy.ts, user: copy.user, info: copy.info })
        .collect();
    drop(odd_best);
    for survivor in &odd {
        if survivor.user & OTHER == 0 {
            home[(survivor.user >> ALIAS_BITS) as usize].store(HOME_MULTI, Relaxed);
        }
    }
    eprintln!("{:>8.3}s fast-path events deduplicated", begin.elapsed().as_secs_f64());

    // ---- Clicks (1 and 3, as `parse_clicks`): a person finished in one
    // shard gets their clicks there; everyone else's go to the general path
    // under their name.
    let files = Files::open_folder(&data.join("clicks"));
    let chunks = files.chunks(chunk_bytes());
    let local_capacity = (files.total_bytes() / 38) as usize / shards / threads.max(1) + 16;
    struct ClickState {
        local: Vec<Vec<LocalClick>>,
        general: Vec<Vec<PersonClick>>,
        dates: ts::Parser,
    }
    let states = par::run(
        threads,
        chunks.len(),
        |_| ClickState {
            local: (0..shards).map(|_| Vec::with_capacity(local_capacity)).collect(),
            general: new_partitions(),
            dates: ts::Parser::default(),
        },
        |state, index| {
            files.with_chunk(&chunks[index], |bytes| {
                for_each_record::<4>(bytes, |&[click_id, user, campaign, ts_text]| {
                    if click_id.is_empty() || user.is_empty() {
                        return;
                    }
                    let Some(ts) = state.dates.parse(ts_text) else { return };
                    if ts < window.click_start || ts >= window.end {
                        return;
                    }
                    let Some(campaign) = lookups.campaigns.get(campaign, || interner.key_in(bytes, campaign)) else { return };
                    let click_id = interner.key_in(bytes, click_id);
                    if let Some(v) = dense_user(decode_in(bytes, user, b'u'), keys.user, keys.persons) {
                        let person = (v >> ALIAS_BITS) as usize;
                        let at = home[person].load(Relaxed);
                        if link_root[person] != LINK_COMPLEX && at != HOME_MULTI {
                            if at != HOME_NONE {
                                state.local[at as usize].push(LocalClick { click_id, ts, user: v, campaign });
                            }
                            return; // HOME_NONE: none of this person's IDs has an event
                        }
                    }
                    let person = names.person(interner.key_in(bytes, user));
                    state.general[partition(person)].push(PersonClick { person, click_id, ts, campaign });
                });
            });
        },
    );
    let mut local_clicks = Vec::with_capacity(states.len());
    let mut general = Vec::with_capacity(states.len());
    for state in states {
        local_clicks.push(state.local);
        general.push(state.general);
    }
    stats.clicks_local = local_clicks.iter().flatten().map(Vec::len).sum();
    stats.clicks_general = general.iter().flatten().map(Vec::len).sum();
    eprintln!("{:>8.3}s fast-path clicks parsed", begin.elapsed().as_secs_f64());

    Some(Prepared {
        keys,
        survivors,
        odd,
        others,
        home,
        link_root,
        link_aliases,
        linked_keys,
        local_clicks,
        general_clicks: transpose(general),
        stats,
    })
}

/// Per person: the root their linked aliases share and which aliases are
/// linked, LINK_NONE when none is, or LINK_COMPLEX when the person is not
/// simple: aliases in several sets, an alias of 64 or more, or a set that
/// also holds another person's ID or an ID that does not decode.
fn classify_links(linked: &Linked, keys: &Keys, threads: usize) -> (Vec<u32>, Vec<u64>) {
    let count = linked.keys.len();
    let persons = keys.persons as usize;
    let mut link_root = vec![LINK_NONE; persons];
    let mut link_aliases = vec![0u64; persons];
    // (user number, root) of every decodable linked ID; roots with any other
    // ID are complex.
    const BLOCK: usize = 1 << 16;
    let blocks = count.div_ceil(BLOCK);
    let parts = par::run(threads, blocks, |_| (Vec::new(), Vec::new()), |(dense, other): &mut (Vec<(u32, u32)>, Vec<u32>), block| {
        for x in block * BLOCK..((block + 1) * BLOCK).min(count) {
            let key = linked.keys[x];
            let decoded = key_bytes(key).and_then(|(bytes, n)| decode_bytes(&bytes[..n], b'u'));
            match dense_user(decoded, keys.user, keys.persons) {
                Some(v) => dense.push((v, linked.roots[x])),
                None => other.push(linked.roots[x]),
            }
        }
    });
    let mut complex_root = vec![false; count];
    let mut nodes: Vec<(u32, u32)> = Vec::new();
    for (dense, other) in parts {
        for root in other {
            complex_root[root as usize] = true;
        }
        nodes.extend(dense);
    }
    // A root holding aliases of two persons is complex too.
    let mut root_person = vec![u32::MAX; count];
    for &(v, root) in &nodes {
        let person = v >> ALIAS_BITS;
        let held = &mut root_person[root as usize];
        if *held == u32::MAX {
            *held = person;
        } else if *held != person {
            complex_root[root as usize] = true;
        }
    }
    drop(root_person);
    nodes.sort_unstable();
    let mut k = 0;
    while k < nodes.len() {
        let person = nodes[k].0 >> ALIAS_BITS;
        let mut r = k + 1;
        while r < nodes.len() && nodes[r].0 >> ALIAS_BITS == person {
            r += 1;
        }
        let root = nodes[k].1;
        let simple = !complex_root[root as usize]
            && nodes[k..r].iter().all(|&(v, other_root)| other_root == root && (v & ((1 << ALIAS_BITS) - 1)) < 64);
        if simple {
            link_root[person as usize] = root;
            link_aliases[person as usize] = nodes[k..r].iter().fold(0, |mask, &(v, _)| mask | 1 << (v & 63));
        } else {
            link_root[person as usize] = LINK_COMPLEX;
        }
        k = r;
    }
    (link_root, link_aliases)
}

// ---------------------------------------------------------------- stage 2

/// Finishes every person whose events are all in one shard and returns their
/// results, with the rest of the events and clicks for `sessionize`.
pub fn finish(
    prepared: Prepared,
    names: &Names,
    lookups: &Lookups,
    window: &Window,
    strings: &Strings,
    threads: usize,
    early_dir: Option<&Path>,
) -> (Vec<ThreadResult>, Partitioned<PersonEvent>, Partitioned<PersonClick>) {
    let Prepared {
        keys,
        survivors,
        odd,
        others,
        home,
        link_root,
        link_aliases,
        linked_keys,
        local_clicks,
        general_clicks,
        mut stats,
    } = prepared;
    let organic = lookups.campaign_names.len();
    let shards = survivors.len();
    let user_key = keys.user;
    // A user number's ID, and the name of its person (as `Names::person`).
    let user_id = |v: u32| encode(b'u', scramble(v as u64, user_key));
    let other_id = |user: u32| others[((user & !OTHER) >> OTHER_SHIFT) as usize][(user & ((1 << OTHER_SHIFT) - 1)) as usize];
    let name_of = |user: u32| -> Key { names.person(if user & OTHER != 0 { other_id(user) } else { user_id(user) }) };
    let person_event = |person: Key, survivor: &Survivor| PersonEvent {
        person,
        ts: window.start + survivor.ts as i64,
        price: survivor.info & 0xFF_FFFF,
        category: (survivor.info >> 24) as u16,
        flags: if survivor.info != 0 { PURCHASE } else { 0 },
    };

    struct State {
        result: ThreadResult,
        general: Vec<Vec<PersonEvent>>,
        events: Vec<PersonEvent>,
        clicks: Vec<PersonClick>,
        sorted: Vec<LocalClick>,
        offsets: Vec<u32>,
        keyed: Vec<(u64, u32)>,
        local_events: usize,
    }
    let states = par::run(
        threads,
        shards,
        |thread| State {
            result: ThreadResult::new(MOST_THREADS + thread, lookups, window),
            general: new_partitions(),
            events: Vec::new(),
            clicks: Vec::new(),
            sorted: Vec::new(),
            offsets: Vec::new(),
            keyed: Vec::new(),
            local_events: 0,
        },
        |state, shard| {
            let kept = std::mem::take(&mut *survivors[shard].lock().unwrap());
            let State { result, general, events, clicks, sorted, offsets, keyed, local_events } = state;
            // This shard's clicks grouped by person: a counting sort over the
            // shard's range of persons (a plain sort if that range is sparse).
            let total: usize = local_clicks.iter().map(|t| t[shard].len()).sum();
            let (mut low, mut high) = (u32::MAX, 0u32);
            for click in local_clicks.iter().flat_map(|t| &t[shard]) {
                low = low.min(click.user >> ALIAS_BITS);
                high = high.max(click.user >> ALIAS_BITS);
            }
            sorted.clear();
            offsets.clear();
            let counted = total > 0 && ((high - low) as usize) < 4 * total + 4096;
            if counted {
                let span = (high - low) as usize + 1;
                offsets.resize(span + 1, 0);
                for click in local_clicks.iter().flat_map(|t| &t[shard]) {
                    offsets[(click.user >> ALIAS_BITS) as usize - low as usize + 1] += 1;
                }
                for at in 0..span {
                    offsets[at + 1] += offsets[at];
                }
                sorted.resize(total, LocalClick { click_id: 0, ts: 0, user: 0, campaign: 0 });
                let mut cursor: Vec<u32> = offsets[..span].to_vec();
                for click in local_clicks.iter().flat_map(|t| &t[shard]) {
                    let bucket = (click.user >> ALIAS_BITS) as usize - low as usize;
                    sorted[cursor[bucket] as usize] = *click;
                    cursor[bucket] += 1;
                }
            } else {
                for list in local_clicks.iter() {
                    sorted.extend_from_slice(&list[shard]);
                }
                sorted.sort_unstable_by_key(|click| click.user >> ALIAS_BITS);
            }
            let clicks_of = |sorted: &[LocalClick], offsets: &[u32], person: u32| -> (usize, usize) {
                if counted {
                    if person < low || person > high {
                        (0, 0)
                    } else {
                        let bucket = (person - low) as usize;
                        (offsets[bucket] as usize, offsets[bucket + 1] as usize)
                    }
                } else {
                    (
                        sorted.partition_point(|c| c.user >> ALIAS_BITS < person),
                        sorted.partition_point(|c| c.user >> ALIAS_BITS <= person),
                    )
                }
            };

            let mut k = 0;
            while k < kept.len() {
                let user = kept[k].user;
                if user & OTHER != 0 {
                    let person = name_of(user);
                    general[partition(person)].push(person_event(person, &kept[k]));
                    k += 1;
                    continue;
                }
                let person = user >> ALIAS_BITS;
                let mut r = k + 1;
                while r < kept.len() && kept[r].user & OTHER == 0 && kept[r].user >> ALIAS_BITS == person {
                    r += 1;
                }
                let run = &kept[k..r];
                k = r;
                let root = link_root[person as usize];
                if root == LINK_COMPLEX || home[person as usize].load(Relaxed) != shard as u32 {
                    for survivor in run {
                        let name = name_of(survivor.user);
                        general[partition(name)].push(person_event(name, survivor));
                    }
                    continue;
                }
                // Finished here. Each event's person key: the root for a
                // linked alias, else the user number itself.
                *local_events += run.len();
                let aliases = if root == LINK_NONE { 0 } else { link_aliases[person as usize] };
                let person_key = |v: u32| -> u64 {
                    let alias = v & ((1 << ALIAS_BITS) - 1);
                    if alias < 64 && aliases >> alias & 1 == 1 { ROOT | root as u64 } else { v as u64 }
                };
                let name_of_key =
                    |key: u64| -> Key { if key & ROOT != 0 { linked_keys[(key & !ROOT) as usize] } else { user_id(key as u32) } };
                let (c0, c1) = clicks_of(sorted, offsets, person);
                let person_clicks = &sorted[c0..c1];
                let first = person_key(run[0].user);
                if run.iter().all(|s| person_key(s.user) == first) {
                    events.clear();
                    events.extend(run.iter().map(|s| person_event(0, s)));
                    events.sort_unstable_by_key(|e| e.ts);
                    clicks.clear();
                    clicks.extend(
                        person_clicks
                            .iter()
                            .filter(|c| person_key(c.user) == first)
                            .map(|c| PersonClick { person: 0, click_id: c.click_id, ts: c.ts, campaign: c.campaign }),
                    );
                    let n = sort_clicks(clicks, strings);
                    finish_person(name_of_key(first), events, &clicks[..n], window, organic, &mut result.scratch, &mut result.user_days, &mut result.campaigns);
                    continue;
                }
                // Several persons in one run (aliases that are not linked).
                keyed.clear();
                keyed.extend(run.iter().enumerate().map(|(at, s)| (person_key(s.user), at as u32)));
                keyed.sort_unstable();
                let mut a = 0;
                while a < keyed.len() {
                    let key = keyed[a].0;
                    let mut b = a + 1;
                    while b < keyed.len() && keyed[b].0 == key {
                        b += 1;
                    }
                    events.clear();
                    events.extend(keyed[a..b].iter().map(|&(_, at)| person_event(0, &run[at as usize])));
                    events.sort_unstable_by_key(|e| e.ts);
                    clicks.clear();
                    clicks.extend(
                        person_clicks
                            .iter()
                            .filter(|c| person_key(c.user) == key)
                            .map(|c| PersonClick { person: 0, click_id: c.click_id, ts: c.ts, campaign: c.campaign }),
                    );
                    let n = sort_clicks(clicks, strings);
                    finish_person(name_of_key(key), events, &clicks[..n], window, organic, &mut result.scratch, &mut result.user_days, &mut result.campaigns);
                    a = b;
                }
            }
            write_early(result, early_dir, lookups, strings);
        },
    );
    // Odd event IDs: always general.
    let mut odd_general: Vec<Vec<PersonEvent>> = new_partitions();
    for survivor in &odd {
        let name = name_of(survivor.user);
        odd_general[partition(name)].push(person_event(name, survivor));
    }
    let local_events: usize = states.iter().map(|s| s.local_events).sum();
    let mut results = Vec::with_capacity(states.len());
    let mut general = Vec::with_capacity(states.len() + 1);
    for state in states {
        results.push(state.result);
        general.push(state.general);
    }
    general.push(odd_general);
    let general_events: usize = general.iter().flatten().map(Vec::len).sum();
    stats.clicks_local = stats.clicks_local.max(0);
    eprintln!(
        "fast-path: {} rows ({} odd event IDs, {} undecoded users); events local {} general {}; clicks local {} general {}",
        stats.rows,
        stats.odd_rows,
        stats.other_users,
        local_events,
        general_events,
        stats.clicks_local,
        stats.clicks_general
    );
    (results, transpose(general), general_clicks)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 11
        }
    }

    #[test]
    fn scramble_round_trip() {
        let mut rng = Rng(0x1234_5678);
        for _ in 0..100_000 {
            let (v, key) = (rng.next() & MASK, rng.next());
            assert_eq!(unscramble(scramble(v, key), key), v);
        }
    }

    /// The generator's own code: tools/gen/src/text.rs `scramble` and `base36`.
    #[test]
    fn encode_matches_the_generator() {
        let mut rng = Rng(77);
        for y in [0u64, 1, 35, 36, 1295, 1296, MASK].into_iter().chain((0..10_000).map(|_| rng.next() & MASK)) {
            let mut text = b"u".to_vec();
            let mut digits = Vec::new();
            let mut rest = y;
            loop {
                digits.push(DIGITS[(rest % 36) as usize]);
                rest /= 36;
                if rest == 0 {
                    break;
                }
            }
            text.extend(digits.iter().rev());
            assert_eq!(encode(b'u', y), crate::key::pack_short(&text));
            assert_eq!(decode_bytes(&text, b'u'), Some(y));
        }
    }

    /// The SWAR decoder against the scalar one, on canonical and broken IDs,
    /// at every position in a chunk (including its first bytes).
    #[test]
    fn decode_in_matches_decode_bytes() {
        let mut rng = Rng(7);
        let alphabet = b"0123456789abcdefghijklmnopqrstuvwxyzueAZ_-/:` \x7f\x80\xff";
        for _ in 0..300_000 {
            let lead = (rng.next() % 12) as usize;
            let n = (rng.next() % 12) as usize;
            let mut chunk: Vec<u8> = (0..lead).map(|_| alphabet[(rng.next() % alphabet.len() as u64) as usize]).collect();
            let start = chunk.len();
            for at in 0..n {
                let c = if at == 0 && rng.next() % 4 != 0 {
                    b'u'
                } else if rng.next() % 8 != 0 {
                    alphabet[(rng.next() % 36) as usize]
                } else {
                    alphabet[(rng.next() % alphabet.len() as u64) as usize]
                };
                chunk.push(c);
            }
            let end = chunk.len();
            chunk.extend_from_slice(b",x");
            let field = &chunk[start..end];
            if field.is_empty() {
                continue;
            }
            assert_eq!(decode_in(&chunk, field, b'u'), decode_bytes(field, b'u'), "{:?}", String::from_utf8_lossy(field));
        }
        assert_eq!(decode_bytes(b"uzzzzzzzz", b'u'), None); // 36^8 - 1 > 2^40
        assert_eq!(decode_bytes(b"u0", b'u'), Some(0));
        assert_eq!(decode_bytes(b"u01", b'u'), None);
        assert_eq!(decode_bytes(b"U1", b'u'), None);
        assert_eq!(decode_bytes(b"u", b'u'), None);
    }

    #[test]
    fn recovers_keys_from_pairs() {
        let mut rng = Rng(99);
        for round in 0..20 {
            let key = rng.next() & MASK;
            let mut pairs = Vec::new();
            for _ in 0..500 {
                let person = rng.next() % (1 << 20);
                let (a, b) = (rng.next() % 6, rng.next() % 6);
                if a != b {
                    pairs.push((scramble(person * 512 + a, key), scramble(person * 512 + b, key)));
                }
            }
            let ys: Vec<u64> = pairs.iter().map(|p| p.0).collect();
            let score =
                |k: u64| ys.iter().filter(|&&y| unscramble(y, k) >> 9 < 1 << 20).count() as f64 / ys.len() as f64;
            let (found, s) = solve_key(&pairs, 8, score).unwrap();
            assert_eq!(found, key, "round {round}");
            assert_eq!(s, 1.0);
        }
    }
}
