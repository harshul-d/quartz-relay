//! Linked IDs (rules.md "Linked IDs"): union-find over the link lines, and
//! the lookup from any user ID to its person's name.
//!
//! Every ID that appears in a valid link line gets a dense integer id from a
//! hash table whose slot range is split by the top bits of the key hash, so
//! each partition's slots are disjoint and the table is built in parallel
//! without locks. Union-find then runs on the dense ids.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::csv::{Files, for_each_record};
use crate::key::{self, Interner, Key, hash};
use crate::par;

const PARTITION_BITS: u32 = 8;
const PARTITIONS: usize = 1 << PARTITION_BITS;

/// The lookup used on every event and click: each linked ID whose name is
/// another ID, mapped to that name, in one flat open-addressing table of
/// (key, name) slots, so a hit costs a single cache miss. Slot positions
/// come from the top hash bits, so partition `p` owns one contiguous slot
/// range (probing wraps inside it) and the table is built in parallel. A
/// bitset over the same keys answers most misses without touching it.
pub struct Names {
    /// (key, name); key 0 (the empty ID, never linked) marks an empty slot.
    table: Vec<(Key, Key)>,
    /// log2 of the table size.
    table_bits: u32,
    /// Slots per partition - 1.
    range_mask: usize,
    filter: Vec<u64>,
    filter_mask: u64,
}

#[inline(always)]
fn prefetch<T>(item: &T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: prefetching is a hint and never faults; the pointer is valid anyway.
    unsafe {
        std::arch::x86_64::_mm_prefetch(item as *const T as *const i8, std::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = item;
}

impl Names {
    /// The person (name) of a user ID: the ID itself unless a link joins it
    /// to a byte-wise smaller one. (The pipeline uses the batched steps below.)
    #[allow(dead_code)]
    #[inline(always)]
    pub fn person(&self, user: Key) -> Key {
        let hashed = hash(user);
        if !self.may_be_linked(hashed) {
            return user;
        }
        self.resolve(user, hashed)
    }

    // The three steps of `person`, for batched lookups that overlap their
    // cache misses: prefetch the filter word, test it and prefetch the table
    // slot, then resolve.

    #[inline(always)]
    pub fn prefetch_filter(&self, hashed: u64) {
        let bit = filter_bit(hashed, self.filter_mask);
        prefetch(&self.filter[(bit >> 6) as usize]);
    }

    #[inline(always)]
    pub fn may_be_linked(&self, hashed: u64) -> bool {
        let bit = filter_bit(hashed, self.filter_mask);
        self.filter[(bit >> 6) as usize] & (1 << (bit & 63)) != 0
    }

    #[inline(always)]
    pub fn prefetch_slot(&self, hashed: u64) {
        prefetch(&self.table[(hashed >> (64 - self.table_bits)) as usize]);
    }

    /// The table lookup itself (`user` hashes to `hashed`).
    #[inline(always)]
    pub fn resolve(&self, user: Key, hashed: u64) -> Key {
        let mut slot = (hashed >> (64 - self.table_bits)) as usize;
        loop {
            let (key, name) = self.table[slot];
            if key == user {
                return name;
            }
            if key == 0 {
                return user;
            }
            slot = (slot & !self.range_mask) | ((slot + 1) & self.range_mask);
        }
    }
}

/// An open-addressing table from keys to dense ids, used while building. A
/// slot holds a 32-bit fingerprint of the hash and 1 + the id's index within
/// its partition (0 = empty); a fingerprint match is confirmed by comparing
/// the full key.
struct DenseIds {
    /// Slots per partition, a power of two.
    slots_per_partition: usize,
    slots: Vec<u64>,
    /// First dense id of each partition.
    bases: Vec<u32>,
    /// Key of each dense id.
    keys: Vec<Key>,
}

#[inline(always)]
fn partition_of(hashed: u64) -> usize {
    (hashed >> (64 - PARTITION_BITS)) as usize
}

#[inline(always)]
fn fingerprint(hashed: u64) -> u64 {
    hashed & 0xFFFF_FFFF
}

#[inline(always)]
fn filter_bit(hashed: u64, mask: u64) -> u64 {
    hashed.rotate_left(29) & mask
}

impl DenseIds {
    /// The dense id of `key`, if it appears in a link.
    #[inline(always)]
    fn id(&self, key: Key, hashed: u64) -> Option<u32> {
        let part = partition_of(hashed);
        let base = part * self.slots_per_partition;
        let mask = self.slots_per_partition - 1;
        let tag = fingerprint(hashed);
        // Slot position from hash bits below the partition bits.
        let mut slot = (hashed >> 32) as usize & mask;
        loop {
            let held = self.slots[base + slot];
            if held == 0 {
                return None;
            }
            if held >> 32 == tag {
                let id = self.bases[part] + (held as u32 - 1);
                if self.keys[id as usize] == key {
                    return Some(id);
                }
            }
            slot = (slot + 1) & mask;
        }
    }
}

/// Every linked ID with its set, for the fast path (`fast.rs`): `keys[x]` is
/// the ID with dense id `x`, `roots[x]` the dense id of its set's root, whose
/// key is the set's name (the smallest ID). `sample` holds the first few
/// thousand link pairs, for key recovery.
pub struct Linked {
    pub keys: Vec<Key>,
    pub roots: Vec<u32>,
    pub sample: Vec<(Key, Key)>,
}

/// Link pairs kept in `Linked::sample`.
const SAMPLE_PAIRS: usize = 8192;

/// Parses every link file and builds the person names.
#[allow(dead_code)] // tests and examples/parsebench.rs
pub fn build(data: &Path, interner: &Interner, threads: usize, chunk_bytes: u64) -> Names {
    build_linked(data, interner, threads, chunk_bytes).0
}

/// `build`, plus the sets themselves.
pub fn build_linked(data: &Path, interner: &Interner, threads: usize, chunk_bytes: u64) -> (Names, Linked) {
    let files = Files::open_folder(&data.join("links"));
    // The link files are small next to the events: about 16 chunks per thread,
    // so every thread has work (2 MB chunks left ~25 for the whole 20M set).
    let target = (files.total_bytes() / (threads.max(1) as u64 * 16)).max(64 << 10).min(chunk_bytes);
    let chunks = files.chunks(target);

    // 1. Parse (Linked IDs 1): valid pairs, and each endpoint by partition.
    struct Parsed {
        pairs: Vec<(Key, Key)>,
        endpoints: Vec<Vec<Key>>,
    }
    let parsed = par::run(
        threads,
        chunks.len(),
        |_| Parsed { pairs: Vec::new(), endpoints: (0..PARTITIONS).map(|_| Vec::new()).collect() },
        |state, index| {
            files.with_chunk(&chunks[index], |bytes| {
                for_each_record::<2>(bytes, |&[a, b]| {
                    if a.is_empty() || b.is_empty() {
                        return;
                    }
                    let (a, b) = (interner.key_in(bytes, a), interner.key_in(bytes, b));
                    state.pairs.push((a, b));
                    state.endpoints[partition_of(hash(a))].push(a);
                    state.endpoints[partition_of(hash(b))].push(b);
                })
            });
        },
    );

    // 2. Dense ids: each partition inserts its keys into its own slot range.
    let most = (0..PARTITIONS)
        .map(|part| parsed.iter().map(|state| state.endpoints[part].len()).sum::<usize>())
        .max()
        .unwrap_or(0);
    let slots_per_partition = (most + most / 2 + 1).next_power_of_two().max(16);
    let mut slots = vec![0u64; slots_per_partition * PARTITIONS];
    let partition_keys: Vec<Vec<Key>> = {
        let slot_ranges: Vec<Mutex<&mut [u64]>> =
            slots.chunks_mut(slots_per_partition).map(Mutex::new).collect();
        let results = par::run(threads, PARTITIONS, |_| Vec::new(), |out: &mut Vec<(usize, Vec<Key>)>, part| {
            let mut range = slot_ranges[part].lock().unwrap();
            let mask = slots_per_partition - 1;
            let mut keys: Vec<Key> = Vec::new();
            for state in &parsed {
                for &key in &state.endpoints[part] {
                    let hashed = hash(key);
                    let tag = fingerprint(hashed);
                    let mut slot = (hashed >> 32) as usize & mask;
                    loop {
                        let held = range[slot];
                        if held == 0 {
                            keys.push(key);
                            range[slot] = tag << 32 | keys.len() as u64;
                            break;
                        }
                        if held >> 32 == tag && keys[(held as u32 - 1) as usize] == key {
                            break;
                        }
                        slot = (slot + 1) & mask;
                    }
                }
            }
            out.push((part, keys));
        });
        let mut by_part: Vec<Vec<Key>> = (0..PARTITIONS).map(|_| Vec::new()).collect();
        for list in results {
            for (part, keys) in list {
                by_part[part] = keys;
            }
        }
        by_part
    };
    let mut bases = Vec::with_capacity(PARTITIONS);
    let mut keys: Vec<Key> = Vec::with_capacity(partition_keys.iter().map(Vec::len).sum());
    for part_keys in &partition_keys {
        bases.push(keys.len() as u32);
        keys.extend_from_slice(part_keys);
    }
    drop(partition_keys);
    let names = DenseIds { slots_per_partition, slots, bases, keys };

    // 3. Union-find over dense ids (Linked IDs 2: joining is transitive;
    //    repeats and self-links are no-ops), in parallel. A root is only ever
    //    linked, by compare-and-swap, under a root with a byte-wise smaller
    //    key, and path halving only moves a pointer up to an ancestor, so a
    //    parent always has a smaller key than its child: no cycles, and each
    //    root is its set's smallest ID, the person's name (Linked IDs 3).
    let count = names.keys.len();
    let parent: Vec<AtomicU32> = (0..count as u32).map(AtomicU32::new).collect();
    let find = |mut x: u32| -> u32 {
        loop {
            let up = parent[x as usize].load(Ordering::Relaxed);
            if up == x {
                return x;
            }
            let grand = parent[up as usize].load(Ordering::Relaxed);
            if grand != up {
                let _ = parent[x as usize].compare_exchange_weak(up, grand, Ordering::Relaxed, Ordering::Relaxed);
            }
            x = up;
        }
    };
    par::run(threads, parsed.len(), |_| (), |_, index| {
        for &(a, b) in &parsed[index].pairs {
            let (mut a, mut b) = (names.id(a, hash(a)).unwrap(), names.id(b, hash(b)).unwrap());
            loop {
                a = find(a);
                b = find(b);
                if a == b {
                    break;
                }
                let (low, high) =
                    if compare(interner, names.keys[a as usize], names.keys[b as usize]).is_lt() { (a, b) } else { (b, a) };
                if parent[high as usize].compare_exchange(high, low, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    break;
                }
            }
        }
    });
    let sample: Vec<(Key, Key)> = parsed.iter().flat_map(|state| state.pairs.iter().copied()).take(SAMPLE_PAIRS).collect();
    drop(parsed);
    const BLOCK: usize = 1 << 16;
    let mut person_names: Vec<Key> = vec![0; count];
    let mut roots: Vec<u32> = vec![0; count];
    {
        let blocks: Vec<Mutex<(&mut [Key], &mut [u32])>> =
            person_names.chunks_mut(BLOCK).zip(roots.chunks_mut(BLOCK)).map(Mutex::new).collect();
        par::run(threads, blocks.len(), |_| (), |_, block| {
            let mut guard = blocks[block].lock().unwrap();
            let (out, out_roots) = &mut *guard;
            for (offset, name) in out.iter_mut().enumerate() {
                let root = find((block * BLOCK + offset) as u32);
                out_roots[offset] = root;
                *name = names.keys[root as usize];
            }
        });
    }
    drop(parent);    // 5. The lookup table: only IDs named by another ID go in. Dense ids are
    //    grouped by partition, and partition p fills its own slot range.
    let linked_in = |part: usize| -> std::ops::Range<usize> {
        let end = names.bases.get(part + 1).map_or(count, |&base| base as usize);
        names.bases[part] as usize..end
    };
    let counts: Vec<usize> = {
        let counted = par::run(threads, PARTITIONS, |_| Vec::new(), |out: &mut Vec<(usize, usize)>, part| {
            out.push((part, linked_in(part).filter(|&x| person_names[x] != names.keys[x]).count()));
        });
        let mut counts = vec![0; PARTITIONS];
        for (part, n) in counted.into_iter().flatten() {
            counts[part] = n;
        }
        counts
    };
    let most = counts.iter().copied().max().unwrap_or(0);
    let range = (most + most / 2 + 1).next_power_of_two().max(2);
    let table_bits = (range * PARTITIONS).trailing_zeros();
    let range_mask = range - 1;
    let mut table: Vec<(Key, Key)> = vec![(0, 0); range * PARTITIONS];

    // 6. The filter over the same keys, filled in the same parallel pass.
    let linked: usize = counts.iter().sum();
    let bits = (linked * 16).next_power_of_two().max(1 << 16);
    let filter_mask = bits as u64 - 1;
    let filter: Vec<std::sync::atomic::AtomicU64> = (0..bits / 64).map(|_| Default::default()).collect();
    {
        let ranges: Vec<Mutex<&mut [(Key, Key)]>> = table.chunks_mut(range).map(Mutex::new).collect();
        par::run(threads, PARTITIONS, |_| (), |_, part| {
            let mut slots = ranges[part].lock().unwrap();
            for x in linked_in(part) {
                let (key, name) = (names.keys[x], person_names[x]);
                if key == name {
                    continue;
                }
                let hashed = hash(key);
                let mut slot = (hashed >> (64 - table_bits)) as usize & range_mask;
                while slots[slot].0 != 0 {
                    slot = (slot + 1) & range_mask;
                }
                slots[slot] = (key, name);
                let bit = filter_bit(hashed, filter_mask);
                filter[(bit >> 6) as usize].fetch_or(1 << (bit & 63), Ordering::Relaxed);
            }
        });
    }
    let filter: Vec<u64> = filter.into_iter().map(|word| word.into_inner()).collect();
    let linked = Linked { keys: names.keys, roots, sample };
    (Names { table, table_bits, range_mask, filter, filter_mask }, linked)
}
/// Byte-wise comparison of two keys while the interner is still open.
fn compare(interner: &Interner, a: Key, b: Key) -> std::cmp::Ordering {
    if !key::is_long(a) && !key::is_long(b) {
        a.cmp(&b)
    } else {
        let bytes = |key: Key| -> Vec<u8> {
            if key::is_long(key) {
                interner.lookup_long(key)
            } else {
                let bytes = key.to_be_bytes();
                bytes[..bytes[15] as usize].to_vec()
            }
        };
        bytes(a).cmp(&bytes(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// The parallel union-find and lookup table against a plain sequential
    /// union-find, on random link files with long chains, stars, repeats,
    /// self-links and IDs that sort before everything else.
    #[test]
    fn names_match_sequential_union_find() {
        for round in 0..6u64 {
            let mut rng = Rng(0x1234_5678 + round * 7919);
            let ids: Vec<String> = (0..4000u64)
                .map(|i| match rng.below(20) {
                    0 => format!("U{i:x}"),
                    1 => format!("long-identifier-over-fifteen-bytes-{i}"),
                    _ => format!("u{:x}", (i as u64 * 2_654_435_761) % 100_003),
                })
                .collect();
            let mut pairs: Vec<(usize, usize)> = Vec::new();
            for _ in 0..30 {
                // a chain hundreds of links long, in random order
                let length = 50 + rng.below(400) as usize;
                let members: Vec<usize> = (0..length).map(|_| rng.below(ids.len() as u64) as usize).collect();
                for w in members.windows(2) {
                    pairs.push((w[0], w[1]));
                }
            }
            for _ in 0..3000 {
                let a = rng.below(ids.len() as u64) as usize;
                let b = if rng.below(10) == 0 { a } else { rng.below(ids.len() as u64) as usize };
                pairs.push((a, b));
            }
            let hub = rng.below(ids.len() as u64) as usize;
            for _ in 0..500 {
                pairs.push((hub, rng.below(ids.len() as u64) as usize));
            }
            for i in (0..pairs.len()).rev() {
                let j = rng.below(i as u64 + 1) as usize;
                pairs.swap(i, j);
            }
            let repeats: Vec<(usize, usize)> = pairs.iter().step_by(13).map(|&(a, b)| (b, a)).collect();
            pairs.extend(repeats);

            let dir = std::env::temp_dir().join(format!("ss-links-test-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("links")).unwrap();
            for (part, chunk) in pairs.chunks(pairs.len() / 5 + 1).enumerate() {
                let mut text = String::from("user_id_a,user_id_b\n");
                for &(a, b) in chunk {
                    text.push_str(&format!("{},{}\n", ids[a], ids[b]));
                }
                std::fs::write(dir.join("links").join(format!("part-{part}.csv")), text).unwrap();
            }
            let interner = Interner::default();
            let names = build(&dir, &interner, 8, 512);

            // Sequential reference on the strings themselves.
            let mut parent: HashMap<&str, &str> = HashMap::new();
            fn root<'a>(parent: &mut HashMap<&'a str, &'a str>, x: &'a str) -> &'a str {
                let mut r = x;
                while let Some(&p) = parent.get(r) {
                    if p == r {
                        break;
                    }
                    r = p;
                }
                r
            }
            for &(a, b) in &pairs {
                let (a, b) = (ids[a].as_str(), ids[b].as_str());
                parent.entry(a).or_insert(a);
                parent.entry(b).or_insert(b);
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    if ra < rb {
                        parent.insert(rb, ra);
                    } else {
                        parent.insert(ra, rb);
                    }
                }
            }
            for id in &ids {
                let expected = if parent.contains_key(id.as_str()) { root(&mut parent, id) } else { id.as_str() };
                let got = names.person(interner.key(id.as_bytes()));
                let got_bytes = interner_bytes(&interner, got);
                assert_eq!(got_bytes, expected.as_bytes(), "round {round}: person of {id}");
            }
            assert_eq!(names.person(interner.key(b"never-linked")), interner.key(b"never-linked"));
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    fn interner_bytes(interner: &Interner, key: Key) -> Vec<u8> {
        if key::is_long(key) {
            interner.lookup_long(key)
        } else {
            let bytes = key.to_be_bytes();
            bytes[..bytes[15] as usize].to_vec()
        }
    }
}