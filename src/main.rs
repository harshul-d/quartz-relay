//! Shopping Sessions pipeline. Reads $DATA_DIR, writes $OUTPUT_DIR/user_daily/
//! and $OUTPUT_DIR/campaign_daily/ as Parquet, exactly as rules.md defines.

mod csv;
mod key;
mod out;
mod par;
mod ts;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use csv::{Files, for_each_line, split};
use key::{FastMap, Interner, Key, Strings, hash};
use ts::{DAY, HOUR, MINUTE};

const SESSION_GAP: i64 = 30 * MINUTE;
const LATE_LIMIT: i64 = 24 * HOUR;
const LOOKBACK: i64 = 7 * DAY;

/// Records are spread over this many hash partitions (a power of two).
const PARTITION_BITS: u32 = 8;
const PARTITIONS: usize = 1 << PARTITION_BITS;
const CHUNK_BYTES: usize = 4 << 20;

#[inline(always)]
fn partition(key: Key) -> usize {
    (hash(key) >> (64 - PARTITION_BITS)) as usize
}

/// One parsed, well-formed event line with a non-empty event_id.
#[derive(Clone, Copy)]
struct EventRow {
    event_id: Key,
    user: Key,
    ingest: i64,
    ts: i64,
    price: u32,
    category: u16,
    /// Bit 0: passes validation and is not late. Bit 1: a purchase.
    flags: u8,
}

const VALID: u8 = 1;
const PURCHASE: u8 = 2;

/// A surviving event, keyed by its person.
#[derive(Clone, Copy)]
struct PersonEvent {
    person: Key,
    ts: i64,
    price: u32,
    category: u16,
    purchase: bool,
}

/// A valid click with a non-empty click_id.
#[derive(Clone, Copy)]
struct ClickRow {
    click_id: Key,
    user: Key,
    ts: i64,
    campaign: u32,
}

#[derive(Clone, Copy)]
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
    products: FastMap<Vec<u8>, u16>,
    /// Sorted category names, `unknown` included.
    categories: Vec<Vec<u8>>,
    unknown: u16,
    /// campaign_id -> campaign index
    campaigns: FastMap<Vec<u8>, u32>,
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
    let begin = Instant::now();
    let data = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "/data".into()));
    let output = PathBuf::from(std::env::var("OUTPUT_DIR").unwrap_or_else(|_| "/output".into()));
    let threads = std::env::var("NCPU")
        .ok()
        .and_then(|n| n.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, usize::from))
        .min(64);
    let log = |what: &str| eprintln!("{:>8.3}s {what}", begin.elapsed().as_secs_f64());

    let window = read_window(&data.join("manifest.json"));
    let lookups = read_lookups(&data);
    let interner = Interner::default();
    log("lookups");

    let names = link_names(&data, &interner, threads);
    log("links");

    let event_parts = parse_events(&data, &interner, &lookups, &window, threads);
    log("events parsed");
    let click_parts = parse_clicks(&data, &interner, &lookups, &window, threads);
    log("clicks parsed");
    let strings = interner.finish();

    let person_events = dedup_events(event_parts, &names, threads);
    log("events deduplicated");
    let person_clicks = dedup_clicks(click_parts, &names, threads);
    log("clicks deduplicated");
    drop(names);

    let results = sessionize(person_events, person_clicks, &lookups, &window, &strings, threads);
    log("sessionized");

    out::write_all(&output, results, &lookups, &window, &strings, threads);
    log("written");
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

fn read_lookups(data: &Path) -> Lookups {
    let mut product_rows: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let products = Files::open(&[data.join("products.csv")]);
    for chunk in products.chunks(usize::MAX / 4) {
        for_each_line(chunk.bytes(&products), |line| {
            if let Some([product, category]) = split::<2>(line) {
                if !product.is_empty() && !category.is_empty() {
                    product_rows.push((product.to_vec(), category.to_vec()));
                }
            }
        });
    }
    let mut categories: Vec<Vec<u8>> = product_rows.iter().map(|(_, c)| c.clone()).collect();
    categories.push(b"unknown".to_vec());
    categories.sort();
    categories.dedup();
    assert!(categories.len() < NO_CATEGORY as usize, "too many categories");
    let category_index: FastMap<Vec<u8>, u16> =
        categories.iter().enumerate().map(|(i, c)| (c.clone(), i as u16)).collect();
    let unknown = category_index[&b"unknown"[..]];
    let mut products_map: FastMap<Vec<u8>, u16> = FastMap::default();
    for (product, category) in product_rows {
        products_map.insert(product, category_index[&category]); // a repeated ID keeps its last row
    }

    let mut campaigns: FastMap<Vec<u8>, u32> = FastMap::default();
    let mut campaign_names = Vec::new();
    let mut channels = Vec::new();
    let files = Files::open(&[data.join("campaigns.csv")]);
    for chunk in files.chunks(usize::MAX / 4) {
        for_each_line(chunk.bytes(&files), |line| {
            if let Some([campaign, channel]) = split::<2>(line) {
                if campaign.is_empty() || channel.is_empty() {
                    return;
                }
                match campaigns.get(campaign) {
                    Some(&index) => channels[index as usize] = channel.to_vec(),
                    None => {
                        campaigns.insert(campaign.to_vec(), campaign_names.len() as u32);
                        campaign_names.push(campaign.to_vec());
                        channels.push(channel.to_vec());
                    }
                }
            }
        });
    }
    Lookups { products: products_map, categories, unknown, campaigns, campaign_names, channels }
}

/// Linked IDs: union-find over every valid link line. Returns, for each linked
/// ID that is not its person's name, the name (the byte-wise smallest ID).
fn link_names(data: &Path, interner: &Interner, threads: usize) -> FastMap<Key, Key> {
    let files = Files::open_folder(&data.join("links"));
    let chunks = files.chunks(CHUNK_BYTES);
    let pairs = par::run(threads, chunks.len(), Vec::new, |pairs: &mut Vec<(Key, Key)>, index| {
        for_each_line(chunks[index].bytes(&files), |line| {
            if let Some([a, b]) = split::<2>(line) {
                if !a.is_empty() && !b.is_empty() {
                    pairs.push((interner.key(a), interner.key(b)));
                }
            }
        });
    });

    let mut ids: FastMap<Key, u32> = FastMap::default();
    let mut keys: Vec<Key> = Vec::new();
    let mut parent: Vec<u32> = Vec::new();
    let intern = |key: Key, ids: &mut FastMap<Key, u32>, keys: &mut Vec<Key>, parent: &mut Vec<u32>| -> u32 {
        *ids.entry(key).or_insert_with(|| {
            keys.push(key);
            parent.push(parent.len() as u32);
            (keys.len() - 1) as u32
        })
    };
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
    for list in &pairs {
        for &(a, b) in list {
            let a = intern(a, &mut ids, &mut keys, &mut parent);
            let b = intern(b, &mut ids, &mut keys, &mut parent);
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra as usize] = rb;
            }
        }
    }
    drop(pairs);
    drop(ids);

    // The name of each set: its byte-wise smallest key. Long keys (interned)
    // need their bytes; they cannot be resolved yet, so they compare through
    // a temporary snapshot of the interner.
    let long_bytes = |key: Key| -> Vec<u8> { interner_bytes(interner, key) };
    let mut smallest: Vec<Key> = vec![0; keys.len()];
    let mut has: Vec<bool> = vec![false; keys.len()];
    for x in 0..keys.len() as u32 {
        let root = find(&mut parent, x) as usize;
        let key = keys[x as usize];
        if !has[root] || compare_keys(key, smallest[root], &long_bytes).is_lt() {
            smallest[root] = key;
            has[root] = true;
        }
    }
    let mut names: FastMap<Key, Key> = FastMap::default();
    for x in 0..keys.len() as u32 {
        let root = find(&mut parent, x) as usize;
        if keys[x as usize] != smallest[root] {
            names.insert(keys[x as usize], smallest[root]);
        }
    }
    names
}

/// Byte-wise comparison of two keys while the interner is still open.
fn compare_keys(a: Key, b: Key, long_bytes: &impl Fn(Key) -> Vec<u8>) -> std::cmp::Ordering {
    if !key::is_long(a) && !key::is_long(b) {
        a.cmp(&b)
    } else {
        long_bytes(a).cmp(&long_bytes(b))
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

#[inline(always)]
fn person_of(names: &FastMap<Key, Key>, user: Key) -> Key {
    match names.get(&user) {
        Some(&name) => name,
        None => user,
    }
}

type Partitioned<T> = Vec<Mutex<Vec<Vec<T>>>>;

/// Regroups per-thread partition buffers into per-partition lists.
fn transpose<T>(states: Vec<Vec<Vec<T>>>) -> Partitioned<T> {
    let mut parts: Vec<Vec<Vec<T>>> = (0..PARTITIONS).map(|_| Vec::new()).collect();
    for state in states {
        for (index, buffer) in state.into_iter().enumerate() {
            if !buffer.is_empty() {
                parts[index].push(buffer);
            }
        }
    }
    parts.into_iter().map(Mutex::new).collect()
}

fn new_partitions<T>() -> Vec<Vec<T>> {
    (0..PARTITIONS).map(|_| Vec::new()).collect()
}

/// Events 1 and 3-4 per line. Each row is kept with its validity, because
/// dedup (step 2) happens before validation: an invalid kept copy still wins.
fn parse_events(data: &Path, interner: &Interner, lookups: &Lookups, window: &Window, threads: usize) -> Partitioned<EventRow> {
    let files = Files::open_folder(&data.join("events"));
    let chunks = files.chunks(CHUNK_BYTES);
    let states = par::run(threads, chunks.len(), new_partitions::<EventRow>, |parts, index| {
        for_each_line(chunks[index].bytes(&files), |line| {
            let Some([event_id, user, ts_text, ingest_text, event_type, product, price_text]) = split::<7>(line) else {
                return;
            };
            if event_id.is_empty() {
                return;
            }
            let event_id = interner.key(event_id);
            // ingest_ts is always valid F1; were it not, the copy sorts last.
            let ingest = ts::parse(ingest_text).unwrap_or(i64::MAX);
            let mut row = EventRow { event_id, user: 0, ingest, ts: 0, price: 0, category: 0, flags: 0 };
            'validate: {
                if user.is_empty() || product.is_empty() {
                    break 'validate;
                }
                let Some(ts) = ts::parse(ts_text) else { break 'validate };
                if ts < window.start || ts >= window.end {
                    break 'validate;
                }
                let purchase = match event_type {
                    b"view" | b"cart" => false,
                    b"purchase" => true,
                    _ => break 'validate,
                };
                if purchase {
                    let Some(price) = ts::price(price_text) else { break 'validate };
                    row.price = price;
                    row.category = lookups.products.get(product).copied().unwrap_or(lookups.unknown);
                }
                if ingest.saturating_sub(ts) > LATE_LIMIT {
                    break 'validate;
                }
                row.ts = ts;
                row.user = interner.key(user);
                row.flags = VALID | if purchase { PURCHASE } else { 0 };
            }
            parts[partition(event_id)].push(row);
        });
    });
    transpose(states)
}

/// Clicks 1 and 3 per line. Copies of a click are identical, so validating
/// before dedup (step 2) gives the same result.
fn parse_clicks(data: &Path, interner: &Interner, lookups: &Lookups, window: &Window, threads: usize) -> Partitioned<ClickRow> {
    let files = Files::open_folder(&data.join("clicks"));
    let chunks = files.chunks(CHUNK_BYTES);
    let states = par::run(threads, chunks.len(), new_partitions::<ClickRow>, |parts, index| {
        for_each_line(chunks[index].bytes(&files), |line| {
            let Some([click_id, user, campaign, ts_text]) = split::<4>(line) else { return };
            if click_id.is_empty() || user.is_empty() {
                return;
            }
            let Some(ts) = ts::parse(ts_text) else { return };
            if ts < window.click_start || ts >= window.end {
                return;
            }
            let Some(&campaign) = lookups.campaigns.get(campaign) else { return };
            let click_id = interner.key(click_id);
            parts[partition(click_id)].push(ClickRow { click_id, user: interner.key(user), ts, campaign });
        });
    });
    transpose(states)
}

/// Events 2: per event_id keep the copy with the smallest ingest_ts; keep it
/// only if valid. Survivors are regrouped by person.
fn dedup_events(parts: Partitioned<EventRow>, names: &FastMap<Key, Key>, threads: usize) -> Partitioned<PersonEvent> {
    let states = par::run(threads, PARTITIONS, new_partitions::<PersonEvent>, |out, index| {
        let buffers = std::mem::take(&mut *parts[index].lock().unwrap());
        let mut rows: Vec<EventRow> = buffers.concat();
        drop(buffers);
        rows.sort_unstable_by_key(|row| (row.event_id, row.ingest));
        let mut previous: Option<Key> = None;
        for row in &rows {
            if previous == Some(row.event_id) {
                continue;
            }
            previous = Some(row.event_id);
            if row.flags & VALID != 0 {
                let person = person_of(names, row.user);
                out[partition(person)].push(PersonEvent {
                    person,
                    ts: row.ts,
                    price: row.price,
                    category: row.category,
                    purchase: row.flags & PURCHASE != 0,
                });
            }
        }
    });
    transpose(states)
}

/// Clicks 2: keep one copy per click_id, regrouped by person.
fn dedup_clicks(parts: Partitioned<ClickRow>, names: &FastMap<Key, Key>, threads: usize) -> Partitioned<PersonClick> {
    let states = par::run(threads, PARTITIONS, new_partitions::<PersonClick>, |out, index| {
        let buffers = std::mem::take(&mut *parts[index].lock().unwrap());
        let mut rows: Vec<ClickRow> = buffers.concat();
        drop(buffers);
        rows.sort_unstable_by_key(|row| row.click_id);
        let mut previous: Option<Key> = None;
        for row in &rows {
            if previous == Some(row.click_id) {
                continue;
            }
            previous = Some(row.click_id);
            let person = person_of(names, row.user);
            out[partition(person)].push(PersonClick { person, click_id: row.click_id, ts: row.ts, campaign: row.campaign });
        }
    });
    transpose(states)
}

/// Results of one thread's share of the sessionizing.
pub struct ThreadResult {
    pub user_days: Vec<UserDay>,
    /// Indexed by campaign * days + day offset; the last campaign is `organic`.
    pub campaigns: Vec<CampaignCounts>,
}

struct Session {
    start: i64,
    day: usize,
    events: u32,
    revenue: u64,
    purchase: bool,
}

/// Events 5-7 and Clicks 4-7, one person at a time.
fn sessionize(
    events: Partitioned<PersonEvent>,
    clicks: Partitioned<PersonClick>,
    lookups: &Lookups,
    window: &Window,
    strings: &Strings,
    threads: usize,
) -> Vec<ThreadResult> {
    let organic = lookups.campaign_names.len();
    let days = window.days;
    let categories = lookups.categories.len();
    par::run(
        threads,
        PARTITIONS,
        || ThreadResult { user_days: Vec::new(), campaigns: vec![[0; 4]; (organic + 1) * days] },
        |result, index| {
            let mut events: Vec<PersonEvent> = std::mem::take(&mut *events[index].lock().unwrap()).concat();
            let mut clicks: Vec<PersonClick> = std::mem::take(&mut *clicks[index].lock().unwrap()).concat();
            if events.is_empty() {
                return;
            }
            events.sort_unstable_by_key(|event| (event.person, event.ts));
            clicks.sort_unstable_by(|a, b| {
                a.person.cmp(&b.person).then(a.ts.cmp(&b.ts)).then_with(|| strings.cmp(a.click_id, b.click_id))
            });

            let mut sessions: Vec<Session> = Vec::new();
            let mut category_sums: Vec<u64> = vec![0; categories];
            let mut touched: Vec<u16> = Vec::new();
            let mut scratch = AttributionScratch::default();

            let mut next_click = 0;
            let mut at = 0;
            while at < events.len() {
                let person = events[at].person;
                let mut end = at;
                while end < events.len() && events[end].person == person {
                    end += 1;
                }
                while next_click < clicks.len() && clicks[next_click].person < person {
                    next_click += 1;
                }
                let click_start = next_click;
                while next_click < clicks.len() && clicks[next_click].person == person {
                    next_click += 1;
                }
                let person_clicks = &clicks[click_start..next_click];

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
                    if event.purchase {
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
                        for event in &events[event_at..event_at + session.events as usize] {
                            if event.purchase {
                                let category = event.category as usize;
                                if category_sums[category] == 0 {
                                    touched.push(event.category);
                                }
                                category_sums[category] += event.price as u64;
                            }
                        }
                        event_at += session.events as usize;
                        session_at += 1;
                    }
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

                attribute(&sessions, person_clicks, organic, days, &mut result.campaigns, &mut scratch);
                at = end;
            }
        },
    )
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