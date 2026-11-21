//! Click-stream sessionization pipeline. Reads $DATA_DIR, writes $OUTPUT_DIR/user_daily/
//! and $OUTPUT_DIR/campaign_daily/ as Parquet, exactly as rules.md defines.
//!
//! Data flow:
//! 1. Lookups and links (union-find; each linked ID maps to its person's name).
//! 2. Events, in parallel chunks: each line is parsed and validated. A valid
//!    row goes straight to its person's partition; every row (valid or not)
//!    also leaves an (event_id, ingest_ts, location) copy in its event_id's
//!    partition, because dedup comes before validation.
//! 3. Dedup per event_id partition with a hash table: every copy that loses
//!    (a later ingest_ts, or an equal one seen second) and is valid becomes a
//!    tombstone for its person partition.
//! 4. Clicks, in parallel chunks: valid clicks go to their person's partition.
//!    Copies of a click are identical, so they meet there and are dropped as
//!    neighbours after sorting.
//! 5. Per person partition: drop tombstoned events, sort, sessionize, credit
//!    and assist, and aggregate.
//! 6. Write Parquet.

mod csv;
mod key;
mod links;
mod out;
mod par;
mod timing;
mod ts;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

unsafe extern "C" {
    /// From the statically linked mimalloc (mimalloc.h).
    fn mi_option_set(option: i32, value: std::ffi::c_long);
}
/// `mi_option_purge_delay` in mimalloc 2.x's `mi_option_e`.
const MI_OPTION_PURGE_DELAY: i32 = 15;

use csv::{Files, for_each_line, for_each_record, split};
use key::{FastMap, Interner, Key, Strings, hash};
use ts::{DAY, HOUR, MINUTE};

const SESSION_GAP: i64 = 30 * MINUTE;
const LATE_LIMIT: i64 = 24 * HOUR;
const LOOKBACK: i64 = 7 * DAY;

/// Records are spread over this many hash partitions (a power of two).
const PARTITION_BITS: u32 = 8;
const PARTITIONS: usize = 1 << PARTITION_BITS;
/// Input chunk size; `PIPELINE_CHUNK_KB` overrides it for A/B runs.
fn chunk_bytes() -> u64 {
    static BYTES: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BYTES.get_or_init(|| {
        std::env::var("PIPELINE_CHUNK_KB").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&kb| kb >= 16).map_or(2 << 20, |kb| kb << 10)
    })
}

#[inline(always)]
fn partition(key: Key) -> usize {
    partition_of_hash(hash(key))
}

/// `partition(key)` given `hash(key)`.
#[inline(always)]
fn partition_of_hash(hashed: u64) -> usize {
    (hashed >> (64 - PARTITION_BITS)) as usize
}

/// A surviving-so-far event in its person's partition.
#[derive(Clone, Copy, Default)]
struct PersonEvent {
    person: Key,
    ts: i64,
    price: u32,
    category: u16,
    flags: u8,
}

const PURCHASE: u8 = 1;
const DELETED: u8 = 2;

/// One copy of an event, for dedup. `location` points at the copy's
/// `PersonEvent` (or is `INVALID` when the copy failed validation).
#[derive(Clone, Copy)]
struct EventCopy {
    event_id: Key,
    ingest: i64,
    location: u64,
}

const INVALID: u64 = u64::MAX;

/// Location of a `PersonEvent`: person partition, parsing thread, index.
#[inline(always)]
fn location(partition: usize, thread: usize, index: usize) -> u64 {
    partition as u64 | (thread as u64) << 16 | (index as u64) << 24
}

#[derive(Clone, Copy, Default)]
struct PersonClick {
    person: Key,
    click_id: Key,
    ts: i64,
    campaign: u32,
}

pub struct UserDay {
    pub person: Key,
    pub day: i32,
    pub sessions: u32,
    pub events: u32,
    pub revenue: u64,
    /// Index into the sorted category names, or `NO_CATEGORY`.
    pub top: u16,
}

pub const NO_CATEGORY: u16 = u16::MAX;

/// Per (campaign, day): credited sessions, purchasing sessions, revenue, assisted sessions.
type CampaignCounts = [u64; 4];

struct Lookups {
    /// product_id -> category index
    products: FastMap<Key, u16>,
    /// Sorted category names, `unknown` included.
    categories: Vec<Vec<u8>>,
    unknown: u16,
    /// campaign_id -> campaign index
    campaigns: FastMap<Key, u32>,
    campaign_names: Vec<Vec<u8>>,
    channels: Vec<Vec<u8>>,
}

struct Window {
    start: i64,
    end: i64,
    click_start: i64,
    first_day: i64,
    days: usize,
}

fn main() {
    // A short-lived batch job: never hand freed memory back to the OS, so the
    // next big allocation reuses already-faulted pages.
    // SAFETY: plain FFI call with valid arguments, before any threads start.
    unsafe { mi_option_set(MI_OPTION_PURGE_DELAY, -1) };
    let begin = Instant::now();
    timing::epoch("main-start");
    let data = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let output = PathBuf::from(std::env::var("OUTPUT_DIR").unwrap_or_else(|_| "/output".into()));
    let threads = std::env::var("NCPU")
        .ok()
        .and_then(|n| n.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, usize::from))
        .min(64);
    let log = |what: &str| eprintln!("{:>8.3}s {what}", begin.elapsed().as_secs_f64());

    csv::prefetch_inputs(&data);
    let window = read_window(&data.join("manifest.json"));
    let interner = Interner::default();
    let lookups = read_lookups(&data, &interner);
    log("lookups");

    let names = links::build(&data, &interner, threads, chunk_bytes());
    log("links");

    let events = parse_events(&data, &interner, &names, &lookups, &window, threads);
    log("events parsed");
    let deletions = dedup_events(events.copies, threads);
    log("events deduplicated");
    let clicks = parse_clicks(&data, &interner, &names, &lookups, &window, threads);
    log("clicks parsed");
    drop(names);
    let strings = interner.finish();

    // `PIPELINE_EARLY_WRITE=0` (A/B only): write everything at the end, no writeback hints.
    let early = std::env::var_os("PIPELINE_EARLY_WRITE").is_none_or(|v| v != "0");
    out::set_writeback(early);
    let user_dir = out::create_dirs(&output);
    let results =
        sessionize(events.people, deletions, clicks, &lookups, &window, &strings, threads, early.then_some(&*user_dir));
    log("sessionized");

    out::write_all(&output, results, &lookups, &window, &strings, threads);
    log("written");
    timing::report();
    eprintln!("threads {threads}");
    drop((lookups, strings));
    log("end of main");
    timing::epoch("main-end");
}

/// manifest.json: `{"window_start": "...", "window_end": "..."}`.
fn read_window(path: &Path) -> Window {
    let text = std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let value = |name: &str| -> i64 {
        let needle = format!("\"{name}\"");
        let at = memchr::memmem::find(&text, needle.as_bytes()).expect("manifest key") + needle.len();
        let rest = &text[at..];
        let colon = memchr::memchr(b':', rest).expect("manifest colon");
        let rest = &rest[colon + 1..];
        let open = memchr::memchr(b'"', rest).expect("manifest value") + 1;
        let close = open + memchr::memchr(b'"', &rest[open..]).expect("manifest value end");
        ts::parse(&rest[open..close]).expect("manifest timestamp")
    };
    let start = value("window_start");
    let end = value("window_end");
    let first_day = start.div_euclid(DAY);
    let days = if end > start { ((end - 1).div_euclid(DAY) - first_day + 1) as usize } else { 0 };
    Window { start, end, click_start: start - LOOKBACK, first_day, days }
}

fn read_lookups(data: &Path, interner: &Interner) -> Lookups {
    let mut product_rows: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for_each_line(&csv::read_data_lines(&data.join("products.csv")), |line| {
        if let Some([product, category]) = split::<2>(line) {
            if !product.is_empty() && !category.is_empty() {
                product_rows.push((product.to_vec(), category.to_vec()));
            }
        }
    });
    let mut categories: Vec<Vec<u8>> = product_rows.iter().map(|(_, c)| c.clone()).collect();
    categories.push(b"unknown".to_vec());
    categories.sort();
    categories.dedup();
    assert!(categories.len() < NO_CATEGORY as usize, "too many categories");
    let category_index: FastMap<Vec<u8>, u16> =
        categories.iter().enumerate().map(|(i, c)| (c.clone(), i as u16)).collect();
    let unknown = category_index[&b"unknown"[..]];
    let mut products_map: FastMap<Key, u16> = FastMap::default();
    for (product, category) in product_rows {
        products_map.insert(interner.key(&product), category_index[&category]); // a repeated ID keeps its last row
    }

    let mut campaigns: FastMap<Key, u32> = FastMap::default();
    let mut campaign_names = Vec::new();
    let mut channels = Vec::new();
    for_each_line(&csv::read_data_lines(&data.join("campaigns.csv")), |line| {
        if let Some([campaign, channel]) = split::<2>(line) {
            if campaign.is_empty() || channel.is_empty() {
                return;
            }
            match campaigns.get(&interner.key(campaign)) {
                Some(&index) => channels[index as usize] = channel.to_vec(),
                None => {
                    campaigns.insert(interner.key(campaign), campaign_names.len() as u32);
                    campaign_names.push(campaign.to_vec());
                    channels.push(channel.to_vec());
                }
            }
        }
    });
    Lookups { products: products_map, categories, unknown, campaigns, campaign_names, channels }
}

fn new_partitions<T>() -> Vec<Vec<T>> {
    (0..PARTITIONS).map(|_| Vec::new()).collect()
}

/// Per-thread, per-partition capacity for about `rows` rows in total.
fn partition_capacity(rows: u64, threads: usize) -> usize {
    (rows as usize / threads.max(1) / PARTITIONS) + 16
}

fn sized_partitions<T>(capacity: usize) -> Vec<Vec<T>> {
    (0..PARTITIONS).map(|_| Vec::with_capacity(capacity)).collect()
}

/// `[partition][thread]` buffers, each partition taken by one worker.
type Partitioned<T> = Vec<Mutex<Vec<Vec<T>>>>;

/// Partition indices, largest first: the skewed partitions (whale persons)
/// start early instead of leaving one thread working alone at the end.
fn largest_first<T>(parts: &Partitioned<T>) -> Vec<usize> {
    let sizes: Vec<usize> = parts.iter().map(|part| part.lock().unwrap().iter().map(Vec::len).sum()).collect();
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(sizes[index]));
    order
}

/// Regroups `[thread][partition]` buffers into `[partition][thread]`.
fn transpose<T>(states: Vec<Vec<Vec<T>>>) -> Partitioned<T> {
    let mut parts: Vec<Vec<Vec<T>>> = (0..PARTITIONS).map(|_| Vec::with_capacity(states.len())).collect();
    for state in states {
        for (index, buffer) in state.into_iter().enumerate() {
            parts[index].push(buffer);
        }
    }
    parts.into_iter().map(Mutex::new).collect()
}

struct ParsedEvents {
    /// Valid rows by person partition, `[partition][thread]`.
    people: Partitioned<PersonEvent>,
    /// Every row's dedup copy by event_id partition.
    copies: Partitioned<EventCopy>,
}

/// Events 1 and 3-4 per line, plus the person of each valid row.
fn parse_events(
    data: &Path,
    interner: &Interner,
    names: &links::Names,
    lookups: &Lookups,
    window: &Window,
    threads: usize,
) -> ParsedEvents {
    let skip: u8 = std::env::var("PIPELINE_SKIP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let files = Files::open_folder(&data.join("events"));
    let chunks = files.chunks(chunk_bytes());
    // Event lines average ~75 bytes; reserve for a little more rows than that,
    // so the partition buffers almost never grow (and copy).
    let capacity = partition_capacity(files.total_bytes() / 64, threads);
    /// A valid row waiting for its person: lookups are done in batches so
    /// their cache misses overlap (see `links::Names`).
    #[derive(Clone, Copy)]
    struct Pending {
        user: Key,
        event_id: Key,
        hashed: u64,
        ingest: i64,
        ts: i64,
        price: u32,
        category: u16,
        flags: u8,
        linked: bool,
    }
    const BATCH: usize = 32;
    struct State {
        thread: usize,
        people: Vec<Vec<PersonEvent>>,
        copies: Vec<Vec<EventCopy>>,
        pending: Vec<Pending>,
        dates: ts::Parser,
        /// Diagnostics only (`PIPELINE_SKIP`, wrong output): bit 0 skips the
        /// dedup copies, bit 1 the person records, bit 2 the whole lookup and
        /// scatter, bit 3 everything after tokenizing.
        skip: u8,
    }
    fn flush(state: &mut State, names: &links::Names) {
        if state.skip & 4 != 0 {
            state.pending.clear();
            return;
        }
        for row in state.pending.iter_mut() {
            row.linked = names.may_be_linked(row.hashed);
            if row.linked {
                names.prefetch_slot(row.hashed);
            }
        }
        for row in &state.pending {
            // An unlinked user is its own person: its hash is already known.
            let (person, part) = if row.linked {
                let person = names.resolve(row.user, row.hashed);
                (person, partition(person))
            } else {
                (row.user, partition_of_hash(row.hashed))
            };
            let buffer = &mut state.people[part];
            let location = location_of(part, state.thread, buffer.len());
            if state.skip & 2 == 0 {
                buffer.push(PersonEvent { person, ts: row.ts, price: row.price, category: row.category, flags: row.flags });
            }
            if state.skip & 1 == 0 {
                state.copies[partition(row.event_id)].push(EventCopy { event_id: row.event_id, ingest: row.ingest, location });
            }
        }
        state.pending.clear();
    }
    /// One event line, decoded field by field as the tokenizer cuts it
    /// (Events 1 and 3-4 run at the line's end).
    struct EventSink<'a, 's> {
        state: &'s mut State,
        bytes: &'a [u8],
        interner: &'s Interner,
        names: &'s links::Names,
        lookups: &'s Lookups,
        window: &'s Window,
        /// Key of the event_id; 0 = empty.
        event_id: Key,
        user: &'a [u8],
        ts: Option<i64>,
        ingest: i64,
        kind: u8,
        product: &'a [u8],
        price: &'a [u8],
    }
    const VIEW_OR_CART: u8 = 0;
    const PURCHASE_TYPE: u8 = 1;
    const BAD_TYPE: u8 = 2;
    impl<'a> csv::FieldSink<'a> for EventSink<'a, '_> {
        #[inline(always)]
        fn field(&mut self, index: usize, value: &'a [u8]) {
            match index {
                0 => self.event_id = self.interner.key_in(self.bytes, value),
                1 => self.user = value,
                2 => self.ts = self.state.dates.parse(value),
                // ingest_ts is always valid F1; were it not, the copy sorts last.
                3 => self.ingest = self.state.dates.parse(value).unwrap_or(i64::MAX),
                4 => {
                    self.kind = match value {
                        b"view" | b"cart" => VIEW_OR_CART,
                        b"purchase" => PURCHASE_TYPE,
                        _ => BAD_TYPE,
                    }
                }
                5 => self.product = value,
                _ => self.price = value,
            }
        }

        #[inline(always)]
        fn end(&mut self, complete: bool) {
            // An incomplete line set only some fields: all are ignored.
            if !complete || self.event_id == 0 || self.state.skip & 8 != 0 {
                return;
            }
            let (event_id, ingest) = (self.event_id, self.ingest);
            'validate: {
                if self.user.is_empty() || self.product.is_empty() {
                    break 'validate;
                }
                let Some(ts) = self.ts else { break 'validate };
                if ts < self.window.start || ts >= self.window.end {
                    break 'validate;
                }
                if self.kind == BAD_TYPE {
                    break 'validate;
                }
                let purchase = self.kind == PURCHASE_TYPE;
                let mut price = 0;
                let mut category = 0;
                if purchase {
                    let Some(value) = ts::price(self.price) else { break 'validate };
                    price = value;
                    let product = self.interner.key_in(self.bytes, self.product);
                    category = self.lookups.products.get(&product).copied().unwrap_or(self.lookups.unknown);
                }
                if ingest.saturating_sub(ts) > LATE_LIMIT {
                    break 'validate;
                }
                let user = self.interner.key_in(self.bytes, self.user);
                let hashed = hash(user);
                self.names.prefetch_filter(hashed);
                self.state.pending.push(Pending {
                    user,
                    event_id,
                    hashed,
                    ingest,
                    ts,
                    price,
                    category,
                    flags: if purchase { PURCHASE } else { 0 },
                    linked: false,
                });
                if self.state.pending.len() == BATCH {
                    flush(self.state, self.names);
                }
                return;
            }
            // Failed validation: only the dedup copy is kept.
            if self.state.skip & 1 == 0 {
                self.state.copies[partition(event_id)].push(EventCopy { event_id, ingest, location: INVALID });
            }
        }
    }
    let states = par::run(
        threads,
        chunks.len(),
        |thread| State {
            thread,
            people: sized_partitions(capacity),
            copies: sized_partitions(capacity),
            pending: Vec::with_capacity(BATCH),
            dates: ts::Parser::default(),
            skip,
        },
        |state, index| {
            files.with_chunk(&chunks[index], |bytes| {
                let mut sink = EventSink {
                    state: &mut *state,
                    bytes,
                    interner,
                    names,
                    lookups,
                    window,
                    event_id: 0,
                    user: &[],
                    ts: None,
                    ingest: i64::MAX,
                    kind: BAD_TYPE,
                    product: &[],
                    price: &[],
                };
                csv::for_each_field::<7>(bytes, &mut sink);
                flush(state, names);
            });
        },
    );    let mut people = Vec::with_capacity(states.len());
    let mut copies = Vec::with_capacity(states.len());
    for state in states {
        people.push(state.people);
        copies.push(state.copies);
    }
    ParsedEvents { people: transpose(people), copies: transpose(copies) }
}

#[inline(always)]
fn location_of(partition: usize, thread: usize, index: usize) -> u64 {
    location(partition, thread, index)
}

/// Events 2: per event_id, the copy with the smallest ingest_ts is kept (on
/// a tie, copies are identical and the first seen is kept). Every other copy
/// that passed validation becomes a tombstone. A kept copy that failed
/// validation drops the event: its valid later copies are tombstoned too.
/// Returns tombstone locations by person partition.
fn dedup_events(copies: Partitioned<EventCopy>, threads: usize) -> Partitioned<u64> {
    let order = largest_first(&copies);
    let states = par::run(threads, PARTITIONS, |_| new_partitions::<u64>(), |deletions, item| {
        let index = order[item];
        let buffers = std::mem::take(&mut *copies[index].lock().unwrap());
        let total: usize = buffers.iter().map(Vec::len).sum();
        if total == 0 {
            return;
        }
        let capacity = (total * 2).next_power_of_two();
        let mask = capacity - 1;
        // Slot = 1 + index into `all` of the kept copy so far; 0 = empty.
        let mut slots: Vec<u32> = vec![0; capacity];
        let all: Vec<EventCopy> = buffers.concat();
        drop(buffers);
        let mut tombstone = |location: u64| {
            if location != INVALID {
                deletions[(location & 0xFFFF) as usize].push(location);
            }
        };
        for (at, copy) in all.iter().enumerate() {
            let mut slot = hash(copy.event_id) as usize & mask;
            loop {
                let held = slots[slot];
                if held == 0 {
                    slots[slot] = at as u32 + 1;
                    break;
                }
                let kept = &all[held as usize - 1];
                if kept.event_id == copy.event_id {
                    if copy.ingest < kept.ingest {
                        tombstone(kept.location);
                        slots[slot] = at as u32 + 1;
                    } else {
                        tombstone(copy.location);
                    }
                    break;
                }
                slot = (slot + 1) & mask;
            }
        }
    });
    transpose(states)
}

/// Clicks 1 and 3 per line; valid clicks go to their person's partition.
/// Copies of a click are identical (Clicks 2), so validating before dedup is
/// the same, and the copies meet in one partition.
fn parse_clicks(
    data: &Path,
    interner: &Interner,
    names: &links::Names,
    lookups: &Lookups,
    window: &Window,
    threads: usize,
) -> Partitioned<PersonClick> {
    let files = Files::open_folder(&data.join("clicks"));
    let chunks = files.chunks(chunk_bytes());
    // Click lines average ~45 bytes.
    let capacity = partition_capacity(files.total_bytes() / 38, threads);
    struct State {
        parts: Vec<Vec<PersonClick>>,
        pending: Vec<(PersonClick, u64, bool)>, // click with its user in `person`, user hash, linked
        dates: ts::Parser,
    }
    const BATCH: usize = 32;
    fn flush(state: &mut State, names: &links::Names) {
        for (_, hashed, linked) in state.pending.iter_mut() {
            *linked = names.may_be_linked(*hashed);
            if *linked {
                names.prefetch_slot(*hashed);
            }
        }
        for &(mut click, hashed, linked) in &state.pending {
            let part = if linked {
                click.person = names.resolve(click.person, hashed);
                partition(click.person)
            } else {
                partition_of_hash(hashed)
            };
            state.parts[part].push(click);
        }
        state.pending.clear();
    }
    let states = par::run(
        threads,
        chunks.len(),
        |_| State {
            parts: sized_partitions::<PersonClick>(capacity),
            pending: Vec::with_capacity(BATCH),
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
                    let Some(&campaign) = lookups.campaigns.get(&interner.key_in(bytes, campaign)) else { return };
                    let user = interner.key_in(bytes, user);
                    let hashed = hash(user);
                    names.prefetch_filter(hashed);
                    let click = PersonClick { person: user, click_id: interner.key_in(bytes, click_id), ts, campaign };
                    state.pending.push((click, hashed, false));
                    if state.pending.len() == BATCH {
                        flush(state, names);
                    }
                });
                flush(state, names);
            });
        },
    );
    let states: Vec<Vec<Vec<PersonClick>>> = states.into_iter().map(|state| state.parts).collect();    transpose(states)
}

/// Results of one thread's share of the sessionizing.
pub struct ThreadResult {
    /// user_daily rows not yet written.
    pub user_days: Vec<UserDay>,
    /// Indexed by campaign * days + day offset; the last campaign is `organic`.
    pub campaigns: Vec<CampaignCounts>,
    thread: usize,
    files: usize,
}

/// user_daily rows a sessionizing thread collects before it writes them out
/// (see `sessionize`).
const EARLY_FILE_ROWS: usize = 1 << 16;

struct Session {
    start: i64,
    day: usize,
    events: u32,
    revenue: u64,
    purchase: bool,
}

/// Events 5-7 and Clicks 4-7, one person at a time.
fn sessionize(
    people: Partitioned<PersonEvent>,
    deletions: Partitioned<u64>,
    clicks: Partitioned<PersonClick>,
    lookups: &Lookups,
    window: &Window,
    strings: &Strings,
    threads: usize,
    early_dir: Option<&Path>,
) -> Vec<ThreadResult> {
    let organic = lookups.campaign_names.len();
    let days = window.days;
    let categories = lookups.categories.len();
    let order = largest_first(&people);
    par::run(
        threads,
        PARTITIONS,
        |thread| ThreadResult {
            user_days: Vec::new(),
            campaigns: vec![[0; 4]; (organic + 1) * days],
            thread,
            files: 0,
        },
        |result, item| {
            let index = order[item];
            let begin = Instant::now();
            let mut buffers = std::mem::take(&mut *people[index].lock().unwrap());
            for list in std::mem::take(&mut *deletions[index].lock().unwrap()) {
                for location in list {
                    let thread = ((location >> 16) & 0xFF) as usize;
                    buffers[thread][(location >> 24) as usize].flags |= DELETED;
                }
            }
            // Group by person without a big sort: a dense id per person in this
            // partition, a counting sort by id, then a small sort by ts per person.
            // The id replaces the person key inside the record (`persons` keeps it).
            let total: usize = buffers.iter().map(Vec::len).sum();
            let mut ids = DenseIds::with_capacity(total / 16 + 16);
            let mut event_starts: Vec<usize> = Vec::new(); // counts, then offsets
            for buffer in buffers.iter_mut() {
                for event in buffer.iter_mut() {
                    if event.flags & DELETED != 0 {
                        continue;
                    }
                    let id = ids.insert(event.person) as usize;
                    if id == event_starts.len() {
                        event_starts.push(0);
                    }
                    event_starts[id] += 1;
                    event.person = id as Key;
                }
            }
            let persons = ids.keys();
            if persons.is_empty() {
                timing::GROUP_EVENTS.add_since(begin);
                return;
            }
            let sum = prefix_sums(&mut event_starts);
            event_starts.push(sum);
            let mut events: Vec<PersonEvent> = Vec::with_capacity(sum);
            let mut cursor: Vec<usize> = event_starts[..persons.len()].to_vec();
            {
                let spare = events.spare_capacity_mut();
                for event in buffers.iter().flatten() {
                    if event.flags & DELETED == 0 {
                        let id = event.person as usize;
                        spare[cursor[id]].write(*event);
                        cursor[id] += 1;
                    }
                }
            }
            // SAFETY: the counts were taken over exactly these events, so every
            // slot in 0..sum was written once above.
            unsafe { events.set_len(sum) };
            drop(buffers);
            for person in 0..persons.len() {
                events[event_starts[person]..event_starts[person + 1]].sort_unstable_by_key(|event| event.ts);
            }
            timing::GROUP_EVENTS.add_since(begin);
            let begin = Instant::now();

            // The same grouping for clicks; clicks of people without events can
            // credit nothing and are dropped.
            let mut click_buffers = std::mem::take(&mut *clicks[index].lock().unwrap());
            let mut click_starts: Vec<usize> = vec![0; persons.len() + 1];
            for click in click_buffers.iter_mut().flatten() {
                let id = ids.get(click.person).unwrap_or(u32::MAX);
                if id != u32::MAX {
                    click_starts[id as usize] += 1;
                }
                click.person = id as Key;
            }
            let sum = prefix_sums(&mut click_starts[..persons.len()]);
            click_starts[persons.len()] = sum;
            let mut clicks: Vec<PersonClick> = Vec::with_capacity(sum);
            let mut cursor: Vec<usize> = click_starts[..persons.len()].to_vec();
            {
                let spare = clicks.spare_capacity_mut();
                for click in click_buffers.iter().flatten() {
                    if click.person != u32::MAX as Key {
                        let id = click.person as usize;
                        spare[cursor[id]].write(*click);
                        cursor[id] += 1;
                    }
                }
            }
            // SAFETY: as for the events, every slot in 0..sum was written once.
            unsafe { clicks.set_len(sum) };
            drop(click_buffers);            // Sort each person's clicks by ts, then click_id byte-wise, and drop
            // identical copies (Clicks 2), which are now neighbours.
            let mut click_ends: Vec<usize> = vec![0; persons.len()];
            for person in 0..persons.len() {
                let (start, end) = (click_starts[person], click_starts[person + 1]);
                let slice = &mut clicks[start..end];
                slice.sort_unstable_by(|a, b| a.ts.cmp(&b.ts).then_with(|| strings.cmp(a.click_id, b.click_id)));
                let mut kept = 0;
                for at in 0..slice.len() {
                    if kept == 0 || slice[kept - 1].click_id != slice[at].click_id {
                        slice[kept] = slice[at];
                        kept += 1;
                    }
                }
                click_ends[person] = start + kept;
            }
            timing::GROUP_CLICKS.add_since(begin);
            let begin = Instant::now();
            let loop_start = timing::ticks();
            let mut attribution_ticks = 0u64;

            let mut sessions: Vec<Session> = Vec::new();
            let mut category_sums: Vec<u64> = vec![0; categories];
            let mut touched: Vec<u16> = Vec::new();
            let mut scratch = AttributionScratch::default();

            for (index, &person) in persons.iter().enumerate() {
                let (at, end) = (event_starts[index], event_starts[index + 1]);
                let person_clicks = &clicks[click_starts[index]..click_ends[index]];
                // Events 5-6: sessions by gaps over 30 minutes between distinct timestamps.
                sessions.clear();
                let mut previous = i64::MIN;
                for event in &events[at..end] {
                    if sessions.is_empty() || event.ts - previous > SESSION_GAP {
                        sessions.push(Session {
                            start: event.ts,
                            day: (event.ts.div_euclid(DAY) - window.first_day) as usize,
                            events: 0,
                            revenue: 0,
                            purchase: false,
                        });
                    }
                    previous = event.ts;
                    let session = sessions.last_mut().unwrap();
                    session.events += 1;
                    if event.flags & PURCHASE != 0 {
                        session.revenue += event.price as u64;
                        session.purchase = true;
                    }
                }

                // user_daily: sessions are in start order, so days come in runs.
                let mut session_at = 0;
                let mut event_at = at;
                while session_at < sessions.len() {
                    let day = sessions[session_at].day;
                    let mut row = UserDay {
                        person,
                        day: (window.first_day + day as i64) as i32,
                        sessions: 0,
                        events: 0,
                        revenue: 0,
                        top: NO_CATEGORY,
                    };
                    while session_at < sessions.len() && sessions[session_at].day == day {
                        let session = &sessions[session_at];
                        row.sessions += 1;
                        row.events += session.events;
                        row.revenue += session.revenue;
                        if session.purchase {
                            for event in &events[event_at..event_at + session.events as usize] {
                                if event.flags & PURCHASE != 0 {
                                    let category = event.category as usize;
                                    if category_sums[category] == 0 {
                                        touched.push(event.category);
                                    }
                                    category_sums[category] += event.price as u64;
                                }
                            }
                        }
                        event_at += session.events as usize;
                        session_at += 1;
                    }
                    // Events 7 / top_category: highest sum, ties to the smallest name.
                    let mut best: u64 = 0;
                    for &category in &touched {
                        let sum = category_sums[category as usize];
                        if sum > best || (sum == best && category < row.top) {
                            best = sum;
                            row.top = category;
                        }
                        category_sums[category as usize] = 0;
                    }
                    touched.clear();
                    result.user_days.push(row);
                }

                let attribution_start = timing::ticks();
                attribute(&sessions, person_clicks, organic, days, &mut result.campaigns, &mut scratch);
                attribution_ticks += timing::ticks().wrapping_sub(attribution_start);
            }
            // Split the measured span by the cycle-count share of attribution.
            let nanos = begin.elapsed().as_nanos() as u64;
            let loop_ticks = timing::ticks().wrapping_sub(loop_start);
            let attribution = if loop_ticks == 0 {
                0
            } else {
                (nanos as f64 * (attribution_ticks as f64 / loop_ticks as f64).min(1.0)) as u64
            };
            timing::ATTRIBUTION.add_nanos(attribution);
            timing::SESSIONS.add_nanos(nanos - attribution);

            // Write finished rows while sessionizing goes on, so the disk works
            // in parallel: the container's exit waits for the output to be on
            // disk (PROGRESS.md, H5b).
            if let Some(dir) = early_dir {
                if result.user_days.len() >= EARLY_FILE_ROWS {
                    let name = format!("part-t{:02}-{:04}.parquet", result.thread, result.files);
                    out::write_user_daily(&dir.join(name), &result.user_days, lookups, strings);
                    result.files += 1;
                    result.user_days.clear();
                }
            }
        },
    )
}

/// Turns counts into start offsets in place; returns the total.
fn prefix_sums(counts: &mut [usize]) -> usize {
    let mut sum = 0;
    for count in counts.iter_mut() {
        let value = *count;
        *count = sum;
        sum += value;
    }
    sum
}

/// Dense ids 0, 1, 2, ... for the persons of one partition: open addressing
/// over the key hash (its low bits; the partition used the top bits), with
/// the full key compared on every probe.
struct DenseIds {
    slots: Vec<u32>, // 1 + id, or 0 for empty
    keys: Vec<Key>,
    mask: usize,
}

impl DenseIds {
    fn with_capacity(expected: usize) -> DenseIds {
        let size = (expected * 2).next_power_of_two().max(64);
        DenseIds { slots: vec![0; size], keys: Vec::with_capacity(expected), mask: size - 1 }
    }

    #[inline(always)]
    fn insert(&mut self, key: Key) -> u32 {
        if self.keys.len() * 2 >= self.slots.len() {
            self.grow();
        }
        let mut slot = hash(key) as usize & self.mask;
        loop {
            let held = self.slots[slot];
            if held == 0 {
                self.keys.push(key);
                self.slots[slot] = self.keys.len() as u32;
                return self.keys.len() as u32 - 1;
            }
            if self.keys[held as usize - 1] == key {
                return held - 1;
            }
            slot = (slot + 1) & self.mask;
        }
    }

    #[inline(always)]
    fn get(&self, key: Key) -> Option<u32> {
        let mut slot = hash(key) as usize & self.mask;
        loop {
            let held = self.slots[slot];
            if held == 0 {
                return None;
            }
            if self.keys[held as usize - 1] == key {
                return Some(held - 1);
            }
            slot = (slot + 1) & self.mask;
        }
    }

    #[cold]
    fn grow(&mut self) {
        let size = self.slots.len() * 2;
        self.slots = vec![0; size];
        self.mask = size - 1;
        for (index, &key) in self.keys.iter().enumerate() {
            let mut slot = hash(key) as usize & self.mask;
            while self.slots[slot] != 0 {
                slot = (slot + 1) & self.mask;
            }
            self.slots[slot] = index as u32 + 1;
        }
    }

    fn keys(&self) -> Vec<Key> {
        self.keys.clone()
    }
}

#[derive(Default)]
struct AttributionScratch {
    run_first: Vec<usize>,
    by_campaign: Vec<(u32, i64)>,
    starts: Vec<i64>,
}

/// Clicks 4-7 for one person. `sessions` are in start order; `clicks` are
/// the person's valid clicks sorted by ts, then click_id byte-wise. Adds to
/// `counts[campaign * days + day]` (campaign `organic` last).
fn attribute(
    sessions: &[Session],
    clicks: &[PersonClick],
    organic: usize,
    days: usize,
    counts: &mut [CampaignCounts],
    scratch: &mut AttributionScratch,
) {
    // Clicks 4-5: credit each session to the latest click in
    // [start - 7 days, start]; on a tie in ts, the smallest click_id, which
    // is the first of its run of equal timestamps.
    let run_first = &mut scratch.run_first;
    run_first.clear();
    for (index, click) in clicks.iter().enumerate() {
        if index > 0 && clicks[index - 1].ts == click.ts {
            run_first.push(run_first[index - 1]);
        } else {
            run_first.push(index);
        }
    }
    let mut upto = 0; // clicks with ts <= session start
    for session in sessions {
        while upto < clicks.len() && clicks[upto].ts <= session.start {
            upto += 1;
        }
        let campaign = if upto > 0 && session.start - clicks[upto - 1].ts <= LOOKBACK {
            clicks[run_first[upto - 1]].campaign as usize
        } else {
            organic
        };
        let counts = &mut counts[campaign * days + session.day];
        counts[0] += 1;
        counts[1] += session.purchase as u64;
        counts[2] += session.revenue;
    }

    // Clicks 7: a campaign assists every session whose start lies in
    // [click.ts, click.ts + 7 days] for one of its clicks, once. Merge each
    // campaign's intervals, then count the sessions inside per day.
    if clicks.is_empty() {
        return;
    }
    let starts = &mut scratch.starts;
    starts.clear();
    starts.extend(sessions.iter().map(|s| s.start));
    let by_campaign = &mut scratch.by_campaign;
    by_campaign.clear();
    by_campaign.extend(clicks.iter().map(|c| (c.campaign, c.ts)));
    by_campaign.sort_unstable();
    let mut index = 0;
    while index < by_campaign.len() {
        let (campaign, first_ts) = by_campaign[index];
        let low = first_ts;
        let mut high = first_ts + LOOKBACK;
        index += 1;
        while index < by_campaign.len() && by_campaign[index].0 == campaign && by_campaign[index].1 <= high {
            high = by_campaign[index].1 + LOOKBACK;
            index += 1;
        }
        count_assists(starts, sessions, low, high, &mut counts[campaign as usize * days..]);
    }
}

/// Adds one assist per day for every session starting in `[low, high]`.
fn count_assists(starts: &[i64], sessions: &[Session], low: i64, high: i64, counts: &mut [CampaignCounts]) {
    let mut first = starts.partition_point(|&s| s < low);
    let last = starts.partition_point(|&s| s <= high);
    while first < last {
        // Sessions are in start order, so one day's sessions are contiguous.
        let next_day = (starts[first].div_euclid(DAY) + 1) * DAY;
        let end = first + starts[first..last].partition_point(|&s| s < next_day);
        counts[sessions[first].day][3] += (end - first) as u64;
        first = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Credit and assists against a direct reading of rules.md Clicks 4-7.
    #[test]
    fn attribution_matches_brute_force() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let first_day = 20_000i64;
        let days = 6usize;
        let campaigns = 4usize;
        for _ in 0..3000 {
            let mut sessions = Vec::new();
            let mut start = first_day * DAY + rng.below(DAY as u64) as i64;
            for _ in 0..rng.below(12) {
                let day = (start.div_euclid(DAY) - first_day) as usize;
                if day >= days {
                    break;
                }
                sessions.push(Session { start, day, events: 1, revenue: rng.below(3), purchase: rng.below(2) == 1 });
                start += SESSION_GAP + 1 + rng.below(2 * DAY as u64) as i64;
            }
            let mut clicks = Vec::new();
            for _ in 0..rng.below(10) {
                let ts = match (sessions.is_empty(), rng.below(4)) {
                    (false, 0) => sessions[rng.below(sessions.len() as u64) as usize].start - LOOKBACK - 1 + rng.below(3) as i64,
                    (false, 1) => sessions[rng.below(sessions.len() as u64) as usize].start - 1 + rng.below(3) as i64,
                    _ => first_day * DAY - LOOKBACK + rng.below((days as u64 + 7) * DAY as u64) as i64,
                };
                let click_id = key::pack_short(format!("k{}", rng.below(30)).as_bytes());
                clicks.push(PersonClick { person: 1, click_id, ts, campaign: rng.below(campaigns as u64) as u32 });
            }
            clicks.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.click_id.cmp(&b.click_id)));
            clicks.dedup_by_key(|c| c.click_id);
            clicks.sort_by(|a, b| a.ts.cmp(&b.ts).then(a.click_id.cmp(&b.click_id)));

            let mut fast = vec![[0u64; 4]; (campaigns + 1) * days];
            attribute(&sessions, &clicks, campaigns, days, &mut fast, &mut AttributionScratch::default());

            let mut slow = vec![[0u64; 4]; (campaigns + 1) * days];
            for s in &sessions {
                let qualifying: Vec<&PersonClick> =
                    clicks.iter().filter(|c| s.start - LOOKBACK <= c.ts && c.ts <= s.start).collect();
                let credited = qualifying
                    .iter()
                    .max_by(|a, b| a.ts.cmp(&b.ts).then(b.click_id.cmp(&a.click_id)))
                    .map_or(campaigns, |c| c.campaign as usize);
                let row = &mut slow[credited * days + s.day];
                row[0] += 1;
                row[1] += s.purchase as u64;
                row[2] += s.revenue;
                let mut assisted: Vec<u32> = qualifying.iter().map(|c| c.campaign).collect();
                assisted.sort();
                assisted.dedup();
                for c in assisted {
                    slow[c as usize * days + s.day][3] += 1;
                }
            }
            assert_eq!(fast, slow);
        }
    }
}
