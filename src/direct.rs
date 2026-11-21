//! Lookup tables keyed by IDs of the form `<prefix><n>`, answered by array
//! index instead of hashing.
//!
//! An ID is canonical when it is the prefix followed by 1 to 7 decimal digits
//! with no leading zero, so each number `n` has exactly one canonical spelling.
//! A `DirectIndex` is made from the finished `FastMap` the lookup used to be:
//! every canonical entry with `n < LIMIT` moves into a dense array indexed by
//! `n`, and every other entry (upper case, leading zeros, long, huge, other
//! prefixes, interned) stays in a fallback map. `get` therefore answers exactly
//! what the map would: a canonical field with `n < LIMIT` is in the map if and
//! only if its array slot is filled, and anything else asks the fallback map.

use crate::key::{FastMap, Key, is_long};

/// Canonical numbers below this go in the array (which is sized to the
/// largest one present, so a catalog of `p1..p50000` takes 50001 slots).
const LIMIT: u32 = 1 << 20;
/// Most digits a canonical ID has. 7 digits keep `n` below 10^7, and keep
/// a canonical ID (prefix of at most 8 bytes) a short, packed key.
const MAX_DIGITS: usize = 7;
/// Longest supported prefix.
const MAX_PREFIX: usize = 8;

pub struct DirectIndex<V: Copy> {
    prefix: [u8; MAX_PREFIX],
    prefix_len: usize,
    /// Slot `n` holds the value of the canonical ID with number `n`.
    slots: Vec<Option<V>>,
    /// Every entry that is not canonical with `n < LIMIT`.
    fallback: FastMap<Key, V>,
}

impl<V: Copy> DirectIndex<V> {
    /// Splits a finished map (built with `Interner::key`, so it already has
    /// the CSV's first-row or last-row semantics) into array and fallback.
    pub fn from_map(prefix: &[u8], map: FastMap<Key, V>) -> Self {
        assert!(prefix.len() <= MAX_PREFIX, "prefix too long");
        let mut index = DirectIndex {
            prefix: [0; MAX_PREFIX],
            prefix_len: prefix.len(),
            slots: Vec::new(),
            fallback: FastMap::default(),
        };
        index.prefix[..prefix.len()].copy_from_slice(prefix);
        let mut direct: Vec<(u32, V)> = Vec::new();
        for (key, value) in map {
            let number = if is_long(key) {
                None // longer than 15 bytes: never canonical
            } else {
                let bytes = key.to_be_bytes();
                index.canonical(&bytes[..bytes[15] as usize])
            };
            match number {
                Some(n) if n < LIMIT => direct.push((n, value)),
                _ => {
                    index.fallback.insert(key, value);
                }
            }
        }
        let size = direct.iter().map(|&(n, _)| n as usize + 1).max().unwrap_or(0);
        index.slots = vec![None; size];
        for (n, value) in direct {
            index.slots[n as usize] = Some(value); // distinct keys, so distinct n
        }
        index.fallback.shrink_to_fit();
        index
    }

    /// The number of `field` if it is `<prefix><1..=7 digits, no leading 0>`.
    #[inline(always)]
    fn canonical(&self, field: &[u8]) -> Option<u32> {
        let prefix_len = self.prefix_len;
        let length = field.len();
        if length <= prefix_len || length > prefix_len + MAX_DIGITS {
            return None;
        }
        if field[..prefix_len] != self.prefix[..prefix_len] {
            return None;
        }
        let digits = &field[prefix_len..];
        if digits[0] == b'0' {
            return None;
        }
        let mut n = 0u32;
        for &byte in digits {
            let digit = byte.wrapping_sub(b'0');
            if digit > 9 {
                return None;
            }
            n = n * 10 + digit as u32;
        }
        Some(n)
    }

    /// What `map.get(&key())` answered for the field's bytes. `key` is only
    /// called for fields that are not canonical with a number below `LIMIT`.
    #[inline(always)]
    pub fn get(&self, field: &[u8], key: impl FnOnce() -> Key) -> Option<V> {
        match self.canonical(field) {
            Some(n) if n < LIMIT => self.slots.get(n as usize).copied().flatten(),
            _ => self.fallback.get(&key()).copied(),
        }
    }

    /// Lookup by key alone (no fast path), for the rare caller without bytes.
    #[allow(dead_code)]
    pub fn get_key(&self, key: Key) -> Option<V> {
        if !is_long(key) {
            let bytes = key.to_be_bytes();
            let field = &bytes[..bytes[15] as usize];
            if let Some(n) = self.canonical(field) {
                if n < LIMIT {
                    return self.slots.get(n as usize).copied().flatten();
                }
            }
        }
        self.fallback.get(&key).copied()
    }

    /// Entries in the array and in the fallback map (for logging).
    #[allow(dead_code)]
    pub fn sizes(&self) -> (usize, usize) {
        (self.slots.iter().filter(|slot| slot.is_some()).count(), self.fallback.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::Interner;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // splitmix64
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len() as u64) as usize]
        }
    }

    /// A random ID: mostly canonical, plus every kind of near miss.
    fn random_id(rng: &mut Rng, prefix: &[u8], catalog_max: u64) -> Vec<u8> {
        let mut id = Vec::new();
        let canonical = |n: u64| {
            let mut id = prefix.to_vec();
            id.extend_from_slice(n.to_string().as_bytes());
            id
        };
        match rng.below(20) {
            0..=8 => return canonical(1 + rng.below(catalog_max)),
            9 => return canonical(1 + rng.below(9_999_999)), // 7 digits, some above LIMIT
            10 => return canonical(LIMIT as u64 - 2 + rng.below(4)), // around LIMIT
            11 => return canonical(10_000_000 + rng.below(1 << 40)), // 8+ digits
            12 => {
                // leading zeros, including n = 0 itself
                id.extend_from_slice(prefix);
                for _ in 0..1 + rng.below(3) {
                    id.push(b'0');
                }
                if rng.below(4) != 0 {
                    id.extend_from_slice(rng.below(catalog_max).to_string().as_bytes());
                }
            }
            13 => {
                // case variant of the prefix
                id = canonical(1 + rng.below(catalog_max));
                for byte in &mut id[..prefix.len()] {
                    *byte = byte.to_ascii_uppercase();
                }
            }
            14 => id.extend_from_slice(prefix), // prefix only
            15 => {}                            // empty
            16 => {
                // canonical plus a trailing non-digit, or a non-digit inside
                id = canonical(1 + rng.below(catalog_max));
                let at = prefix.len() + rng.below((id.len() - prefix.len()) as u64 + 1) as usize;
                id.insert(at, *rng.pick(b"x/:;@ _-0a9"));
            }
            17 => {
                // long, often interned (> 15 bytes)
                id = canonical(1 + rng.below(catalog_max));
                for _ in 0..rng.below(20) {
                    id.push(b'0' + rng.below(10) as u8);
                }
            }
            18 => {
                // a prefix of the prefix, or the other generator's prefix
                id.extend_from_slice(rng.pick(&[&b"p"[..], b"c", b"c_", b"P", b"C_", b"_", b"pp"]));
                id.extend_from_slice((1 + rng.below(catalog_max)).to_string().as_bytes());
            }
            _ => {
                // random bytes around the digit range
                for _ in 0..rng.below(12) {
                    id.push(*rng.pick(b"pc_0123456789/:;Pz"));
                }
            }
        }
        id
    }

    fn check(prefix: &[u8], first_wins: bool, seed: u64) {
        let mut rng = Rng(seed);
        let interner = Interner::default();
        let catalog_max = *rng.pick(&[1u64, 9, 100, 2000, 50_000, 2_000_000]);
        let rows = rng.below(3 * catalog_max.min(30_000) + 10);
        let mut map: FastMap<Key, u32> = FastMap::default();
        for row in 0..rows {
            let id = if rng.below(10) == 0 && row > 0 {
                // a duplicate of a recent canonical ID
                let mut id = prefix.to_vec();
                id.extend_from_slice((1 + rng.below(catalog_max.min(50))).to_string().as_bytes());
                id
            } else {
                random_id(&mut rng, prefix, catalog_max)
            };
            let key = interner.key(&id);
            let value = row as u32;
            if first_wins {
                map.entry(key).or_insert(value);
            } else {
                map.insert(key, value);
            }
        }
        let expected = map.clone();
        let index = DirectIndex::from_map(prefix, map);
        let (direct, fallback) = index.sizes();
        assert_eq!(direct + fallback, expected.len());

        // Probes as slices of one chunk, so `key_in` is exercised too.
        let mut chunk = Vec::new();
        let mut spans = Vec::new();
        for _ in 0..20_000 {
            let id = random_id(&mut rng, prefix, catalog_max + catalog_max / 4 + 2);
            spans.push((chunk.len(), id.len()));
            chunk.extend_from_slice(&id);
            chunk.push(b',');
        }
        for &(start, length) in &spans {
            let field = &chunk[start..start + length];
            let want = expected.get(&interner.key(field)).copied();
            let got = index.get(field, || interner.key_in(&chunk, field));
            assert_eq!(got, want, "seed {seed} field {:?}", String::from_utf8_lossy(field));
            assert_eq!(index.get_key(interner.key(field)), want);
        }
        // Every catalog entry is found again.
        for (&key, &value) in &expected {
            assert_eq!(index.get_key(key), Some(value));
        }
    }

    #[test]
    fn products_last_row_wins() {
        for seed in 0..40 {
            check(b"p", false, seed);
        }
    }

    #[test]
    fn campaigns_first_row_wins() {
        for seed in 100..140 {
            check(b"c_", true, seed);
        }
    }

    #[test]
    fn edge_cases() {
        let interner = Interner::default();
        let mut map: FastMap<Key, u16> = FastMap::default();
        for (id, value) in [
            (&b"p1"[..], 1),
            (b"p01", 2),
            (b"P1", 3),
            (b"p", 4),
            (b"", 5),
            (b"p1048575", 6),
            (b"p1048576", 7),
            (b"p9999999", 8),
            (b"p10000000", 9),
            (b"p0", 10),
            (b"p12345678901234567890", 11),
        ] {
            map.insert(interner.key(id), value);
        }
        let index = DirectIndex::from_map(b"p", map.clone());
        assert_eq!(index.slots.len(), 1_048_576);
        assert_eq!(index.sizes(), (2, 9));
        for probe in [
            &b"p1"[..], b"p01", b"P1", b"p", b"", b"p2", b"p1048575", b"p1048576", b"p1048577", b"p9999999",
            b"p10000000", b"p0", b"p00", b"p12345678901234567890", b"p1x", b"q1", b"p999999",
        ] {
            assert_eq!(index.get(probe, || interner.key(probe)), map.get(&interner.key(probe)).copied(), "{probe:?}");
        }
        let empty = DirectIndex::<u16>::from_map(b"c_", FastMap::default());
        assert_eq!(empty.get(b"c_1", || interner.key(b"c_1")), None);
        assert_eq!(empty.get(b"x", || interner.key(b"x")), None);
    }
}
