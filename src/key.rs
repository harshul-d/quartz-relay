//! String IDs as exact 128-bit keys.
//!
//! An ID of at most 15 bytes is packed big-endian into the top 15 bytes of a
//! `u128`, zero padded, with its length in the lowest byte. Two such keys are
//! equal exactly when the strings are, and their numeric order is the byte-wise
//! order of the strings (a prefix sorts first, as its padding and length are
//! smaller). A longer ID is interned: its key holds the intern index and the
//! tag 0xFF in the lowest byte, which no packed key has. Key 0 is the empty
//! string, which the CSV dialect treats as null.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Mutex;

pub type Key = u128;
const LONG_TAG: u8 = 0xFF;

#[inline(always)]
pub fn pack_short(bytes: &[u8]) -> Key {
    debug_assert!(bytes.len() <= 15);
    let mut buffer = [0u8; 16];
    buffer[..bytes.len()].copy_from_slice(bytes);
    buffer[15] = bytes.len() as u8;
    u128::from_be_bytes(buffer)
}

#[inline(always)]
pub fn is_long(key: Key) -> bool {
    key as u8 == LONG_TAG
}

/// Thread-safe interner for IDs longer than 15 bytes. The generator never
/// writes one, so the lock is uncontended in practice.
#[derive(Default)]
pub struct Interner {
    map: Mutex<(HashMap<Vec<u8>, u64>, Vec<Vec<u8>>)>,
}

impl Interner {
    #[inline(always)]
    pub fn key(&self, bytes: &[u8]) -> Key {
        if bytes.len() <= 15 {
            pack_short(bytes)
        } else {
            self.long(bytes)
        }
    }

    /// `key(field)` for a `field` that lies inside `chunk`: a short ID is one
    /// 16-byte load from the chunk, masked to the field's length. (Copying
    /// the bytes into a buffer and reloading it, as `pack_short` does, stalls
    /// on store forwarding; that was the parse loop's largest single cost.)
    #[inline(always)]
    pub fn key_in(&self, chunk: &[u8], field: &[u8]) -> Key {
        let length = field.len();
        if length > 15 {
            return self.long(field);
        }
        let offset = (field.as_ptr() as usize).wrapping_sub(chunk.as_ptr() as usize);
        debug_assert!(offset <= chunk.len() && offset + length <= chunk.len());
        match chunk.get(offset..offset.wrapping_add(16)) {
            Some(window) => {
                let word = u128::from_be_bytes(window.try_into().unwrap());
                // Keep the top `length` bytes; the length goes in the lowest byte.
                (word & !(u128::MAX >> (8 * length))) | length as u128
            }
            None => pack_short(field), // the last few bytes of a chunk
        }
    }

    #[cold]
    fn long(&self, bytes: &[u8]) -> Key {
        let mut guard = self.map.lock().unwrap();
        let (map, list) = &mut *guard;
        let index = match map.get(bytes) {
            Some(&index) => index,
            None => {
                let index = list.len() as u64;
                list.push(bytes.to_vec());
                map.insert(bytes.to_vec(), index);
                index
            }
        };
        ((index as u128) << 8) | LONG_TAG as u128
    }

    /// The bytes of a long key, while the interner is still open.
    #[cold]
    pub fn lookup_long(&self, key: Key) -> Vec<u8> {
        self.map.lock().unwrap().1[(key >> 8) as usize].clone()
    }

    pub fn finish(self) -> Strings {
        let (_, long) = self.map.into_inner().unwrap();
        Strings { long }
    }
}

/// Resolves keys back to bytes once parsing is done.
pub struct Strings {
    long: Vec<Vec<u8>>,
}

impl Strings {
    /// Appends the key's bytes to `out`.
    #[inline]
    pub fn append(&self, key: Key, out: &mut Vec<u8>) {
        if is_long(key) {
            out.extend_from_slice(&self.long[(key >> 8) as usize]);
        } else {
            let bytes = key.to_be_bytes();
            out.extend_from_slice(&bytes[..bytes[15] as usize]);
        }
    }

    pub fn bytes(&self, key: Key) -> Vec<u8> {
        let mut out = Vec::new();
        self.append(key, &mut out);
        out
    }

    /// Byte-wise order of the strings behind two keys.
    #[inline]
    pub fn cmp(&self, a: Key, b: Key) -> Ordering {
        if !is_long(a) && !is_long(b) {
            a.cmp(&b)
        } else {
            self.bytes(a).cmp(&self.bytes(b))
        }
    }
}

#[inline(always)]
fn fold_mul(a: u64, b: u64) -> u64 {
    let product = (a as u128) * (b as u128);
    (product as u64) ^ ((product >> 64) as u64)
}

/// A fast, well-mixed 64-bit hash of a key. Used only to place keys in
/// partitions and hash tables; identity is always the full key.
#[inline(always)]
pub fn hash(key: Key) -> u64 {
    let low = key as u64;
    let high = (key >> 64) as u64;
    let first = fold_mul(low ^ 0x243F_6A88_85A3_08D3, 0x9E37_79B9_7F4A_7C15);
    fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunk-load packing equals `pack_short`/`long` for every field
    /// position and length, including fields at the very end of a chunk.
    #[test]
    fn key_in_matches_key() {
        let interner = Interner::default();
        let chunk: Vec<u8> = (0..200u32).map(|i| b"az09_,\"Z~"[(i * 7 % 9) as usize] ^ (i as u8 & 0x20)).collect();
        for start in 0..chunk.len() {
            for end in start..chunk.len().min(start + 20) + 1 {
                if end > chunk.len() {
                    break;
                }
                let field = &chunk[start..end];
                for window in [&chunk[..], &chunk[..end], &chunk[start..end]] {
                    assert_eq!(interner.key_in(window, field), interner.key(field), "{start}..{end}");
                }
            }
        }
        assert_eq!(interner.key_in(&chunk, &chunk[5..5]), 0);
    }
}

/// Hasher for `HashMap`s keyed by `Key` (or by byte slices, less tuned).
#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut state = self.0;
        for &byte in bytes {
            state = (state.rotate_left(5) ^ byte as u64).wrapping_mul(0x51_7C_C1_B7_27_22_0A_95);
        }
        self.0 = fold_mul(state, 0x9E37_79B9_7F4A_7C15);
    }
    #[inline]
    fn write_u128(&mut self, key: u128) {
        self.0 ^= hash(key);
    }
    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.0 = (self.0 ^ value as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

pub type FastBuild = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = HashMap<K, V, FastBuild>;
