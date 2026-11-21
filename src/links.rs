//! Linked IDs (rules.md "Linked IDs"): union-find over the link lines, and
//! the lookup from any user ID to its person's name.
//!
//! Every ID that appears in a valid link line gets a dense integer id from a
//! hash table whose slot range is split by the top bits of the key hash, so
//! each partition's slots are disjoint and the table is built in parallel
//! without locks. Union-find then runs on the dense ids.

use std::path::Path;

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

/// Parses every link file and builds the person names.
pub fn build(data: &Path, interner: &Interner, threads: usize, chunk_bytes: u64) -> Names {
    let files = Files::open_folder(&data.join("links"));
    let chunks = files.chunks(chunk_bytes);

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
                    let (a, b) = (interner.key(a), interner.key(b));
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
        let slot_ranges: Vec<std::sync::Mutex<&mut [u64]>> =
            slots.chunks_mut(slots_per_partition).map(std::sync::Mutex::new).collect();
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

    // 3. Translate the pairs to dense ids (in parallel), then union-find
    //    (Linked IDs 2: joining is transitive, repeats and self-links are no-ops).
    let id_pairs = par::run(threads, parsed.len(), |_| Vec::new(), |out: &mut Vec<Vec<(u32, u32)>>, index| {
        let list = parsed[index]
            .pairs
            .iter()
            .map(|&(a, b)| (names.id(a, hash(a)).unwrap(), names.id(b, hash(b)).unwrap()))
            .collect();
        out.push(list);
    });
    drop(parsed);
    let count = names.keys.len();
    let mut parent: Vec<u32> = (0..count as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        let mut root = x;
        while parent[root as usize] != root {
            root = parent[root as usize];
        }
        while parent[x as usize] != root {
            let next = parent[x as usize];
            parent[x as usize] = root;
            x = next;
        }
        root
    }
    for lists in &id_pairs {
        for list in lists {
            for &(a, b) in list {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra as usize] = rb;
                }
            }
        }
    }
    drop(id_pairs);

    // 4. Linked IDs 3: each person's name is its byte-wise smallest ID.
    let mut smallest: Vec<u32> = vec![u32::MAX; count];
    for x in 0..count as u32 {
        let root = find(&mut parent, x) as usize;
        let current = smallest[root];
        if current == u32::MAX || compare(interner, names.keys[x as usize], names.keys[current as usize]).is_lt() {
            smallest[root] = x;
        }
    }
    let person_names: Vec<Key> = (0..count).map(|x| names.keys[smallest[parent[x] as usize] as usize]).collect();
    drop(smallest);
    drop(parent);

    // 5. The lookup table: only IDs named by another ID go in. Dense ids are
    //    grouped by partition, and partition p fills its own slot range.
    let linked_in = |part: usize| -> std::ops::Range<usize> {
        let end = names.bases.get(part + 1).map_or(count, |&base| base as usize);
        names.bases[part] as usize..end
    };
    let most = (0..PARTITIONS)
        .map(|part| linked_in(part).filter(|&x| person_names[x] != names.keys[x]).count())
        .max()
        .unwrap_or(0);
    let range = (most + most / 2 + 1).next_power_of_two().max(2);
    let table_bits = (range * PARTITIONS).trailing_zeros();
    let range_mask = range - 1;
    let mut table: Vec<(Key, Key)> = vec![(0, 0); range * PARTITIONS];
    {
        let ranges: Vec<std::sync::Mutex<&mut [(Key, Key)]>> = table.chunks_mut(range).map(std::sync::Mutex::new).collect();
        par::run(threads, PARTITIONS, |_| (), |_, part| {
            let mut slots = ranges[part].lock().unwrap();
            for x in linked_in(part) {
                let (key, name) = (names.keys[x], person_names[x]);
                if key == name {
                    continue;
                }
                let mut slot = (hash(key) >> (64 - table_bits)) as usize & range_mask;
                while slots[slot].0 != 0 {
                    slot = (slot + 1) & range_mask;
                }
                slots[slot] = (key, name);
            }
        });
    }

    // 6. The filter over the same keys.
    let linked = (0..count).filter(|&x| person_names[x] != names.keys[x]).count();
    let bits = (linked * 16).next_power_of_two().max(1 << 16);
    let mut filter = vec![0u64; bits / 64];
    let filter_mask = bits as u64 - 1;
    for x in 0..count {
        if person_names[x] != names.keys[x] {
            let bit = filter_bit(hash(names.keys[x]), filter_mask);
            filter[(bit >> 6) as usize] |= 1 << (bit & 63);
        }
    }
    Names { table, table_bits, range_mask, filter, filter_mask }
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
