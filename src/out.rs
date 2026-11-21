//! Writes the two output tables as Parquet (rules.md "Outputs").
//!
//! A small, self-contained Parquet writer for exactly these flat schemas:
//! PLAIN encoding, no compression, one data page (v1) per column per row
//! group, definition levels only for the nullable `top_category`. The file
//! metadata is Thrift compact protocol, written by hand below.

use std::path::Path;

use crate::key::Strings;
use crate::{Lookups, NO_CATEGORY, ThreadResult, UserDay, Window, par};

/// Rows per row group (and so per page).
const ROW_GROUP_ROWS: usize = 1 << 19;

// Parquet enums.
const TYPE_INT32: i32 = 1;
const TYPE_INT64: i32 = 2;
const TYPE_BYTE_ARRAY: i32 = 6;
const REQUIRED: i32 = 0;
const OPTIONAL: i32 = 1;
const CONVERTED_UTF8: i32 = 0;
const CONVERTED_DATE: i32 = 6;
const ENCODING_PLAIN: i32 = 0;
const ENCODING_RLE: i32 = 3;
const ENCODING_RLE_DICTIONARY: i32 = 8;
const CODEC_UNCOMPRESSED: i32 = 0;
const PAGE_DATA: i32 = 0;
const PAGE_DICTIONARY: i32 = 2;

// Thrift compact protocol field types.
const T_I32: u8 = 5;
const T_I64: u8 = 6;
const T_BINARY: u8 = 8;
const T_LIST: u8 = 9;
const T_STRUCT: u8 = 12;

/// Thrift compact protocol encoder (only what the Parquet metadata needs).
struct Thrift {
    out: Vec<u8>,
    last: Vec<i16>,
}

impl Thrift {
    fn new() -> Thrift {
        Thrift { out: Vec::new(), last: vec![0] }
    }
    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.out.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.out.push(value as u8);
    }
    fn zigzag(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }
    fn field(&mut self, id: i16, kind: u8) {
        let delta = id - *self.last.last().unwrap();
        if delta > 0 && delta <= 15 {
            self.out.push((delta as u8) << 4 | kind);
        } else {
            self.out.push(kind);
            self.zigzag(id as i64);
        }
        *self.last.last_mut().unwrap() = id;
    }
    fn i32(&mut self, id: i16, value: i32) {
        self.field(id, T_I32);
        self.zigzag(value as i64);
    }
    fn i64(&mut self, id: i16, value: i64) {
        self.field(id, T_I64);
        self.zigzag(value);
    }
    fn string(&mut self, id: i16, value: &[u8]) {
        self.field(id, T_BINARY);
        self.varint(value.len() as u64);
        self.out.extend_from_slice(value);
    }
    fn list_header(&mut self, id: i16, kind: u8, size: usize) {
        self.field(id, T_LIST);
        if size < 15 {
            self.out.push((size as u8) << 4 | kind);
        } else {
            self.out.push(0xF0 | kind);
            self.varint(size as u64);
        }
    }
    /// Starts a struct as field `id` (or as a list element when `id` is None).
    fn begin(&mut self, id: Option<i16>) {
        if let Some(id) = id {
            self.field(id, T_STRUCT);
        }
        self.last.push(0);
    }
    fn end(&mut self) {
        self.out.push(0);
        self.last.pop();
    }
}

/// One column's schema.
struct Column {
    name: &'static str,
    physical: i32,
    optional: bool,
    converted: Option<i32>,
}

const fn column(name: &'static str, physical: i32, optional: bool, converted: Option<i32>) -> Column {
    Column { name, physical, optional, converted }
}

const USER_DAILY: [Column; 6] = [
    column("user_id", TYPE_BYTE_ARRAY, false, Some(CONVERTED_UTF8)),
    column("day", TYPE_INT32, false, Some(CONVERTED_DATE)),
    column("sessions", TYPE_INT64, false, None),
    column("events", TYPE_INT64, false, None),
    column("revenue_cents", TYPE_INT64, false, None),
    column("top_category", TYPE_BYTE_ARRAY, true, Some(CONVERTED_UTF8)),
];

const CAMPAIGN_DAILY: [Column; 7] = [
    column("campaign_id", TYPE_BYTE_ARRAY, false, Some(CONVERTED_UTF8)),
    column("channel", TYPE_BYTE_ARRAY, false, Some(CONVERTED_UTF8)),
    column("day", TYPE_INT32, false, Some(CONVERTED_DATE)),
    column("sessions", TYPE_INT64, false, None),
    column("purchasing_sessions", TYPE_INT64, false, None),
    column("revenue_cents", TYPE_INT64, false, None),
    column("assisted_sessions", TYPE_INT64, false, None),
];

/// A column's values for one row group, PLAIN encoded or (with `dict`)
/// dictionary encoded, with definition levels (1 = present) for an optional
/// column.
#[derive(Default)]
struct ColumnData {
    values: Vec<u8>,
    levels: Vec<bool>,
    dict: Option<Dictionary>,
}

/// A dictionary-encoded column chunk: the distinct values (PLAIN encoded,
/// in index order) and one index per present value. Smaller output matters:
/// the run ends only once the output is on disk (see PROGRESS.md, H5b).
#[derive(Default)]
struct Dictionary {
    values: Vec<u8>,
    count: u32,
    indices: Vec<u32>,
    /// Open addressing: (key, 1 + index), 0 = empty.
    slots: Vec<(u64, u32)>,
}

impl Dictionary {
    /// The index of the value identified by `key`; a new value's PLAIN bytes
    /// are appended by `fill`.
    #[inline]
    fn index_of(&mut self, key: u64, fill: impl FnOnce(&mut Vec<u8>)) -> u32 {
        if self.count as usize * 2 >= self.slots.len() {
            self.grow();
        }
        let mask = self.slots.len() - 1;
        let mut slot = crate::key::hash(key as u128) as usize & mask;
        loop {
            let (held, index) = self.slots[slot];
            if index == 0 {
                self.slots[slot] = (key, self.count + 1);
                return self.add(fill);
            }
            if held == key {
                return index - 1;
            }
            slot = (slot + 1) & mask;
        }
    }

    /// Appends a new value without lookup (the caller knows it is new).
    #[inline]
    fn add(&mut self, fill: impl FnOnce(&mut Vec<u8>)) -> u32 {
        fill(&mut self.values);
        self.count += 1;
        self.count - 1
    }

    #[cold]
    fn grow(&mut self) {
        let size = (self.slots.len() * 2).max(64);
        let old = std::mem::replace(&mut self.slots, vec![(0, 0); size]);
        for (key, index) in old {
            if index != 0 {
                let mut slot = crate::key::hash(key as u128) as usize & (size - 1);
                while self.slots[slot].1 != 0 {
                    slot = (slot + 1) & (size - 1);
                }
                self.slots[slot] = (key, index);
            }
        }
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// The RLE / bit-packing hybrid encoding of `values` at `width` bits, as
/// whichever is smaller: one RLE run per run of equal values, or a single
/// bit-packed run (padded to a multiple of 8 values).
fn hybrid(values: &[u32], width: u32, out: &mut Vec<u8>) {
    let value_bytes = width.div_ceil(8) as usize;
    let mut rle_size = 0usize;
    let mut at = 0;
    while at < values.len() {
        let start = at;
        while at < values.len() && values[at] == values[start] {
            at += 1;
        }
        rle_size += varint_len(((at - start) as u64) << 1) + value_bytes;
    }
    let groups = values.len().div_ceil(8);
    let packed_size = varint_len((groups as u64) << 1 | 1) + groups * width as usize;
    if rle_size <= packed_size {
        let mut at = 0;
        while at < values.len() {
            let start = at;
            while at < values.len() && values[at] == values[start] {
                at += 1;
            }
            varint(out, ((at - start) as u64) << 1);
            out.extend_from_slice(&values[start].to_le_bytes()[..value_bytes]);
        }
    } else {
        varint(out, (groups as u64) << 1 | 1);
        let mut buffer = 0u64;
        let mut bits = 0u32;
        for index in 0..groups * 8 {
            buffer |= (values.get(index).copied().unwrap_or(0) as u64) << bits;
            bits += width;
            while bits >= 8 {
                out.push(buffer as u8);
                buffer >>= 8;
                bits -= 8;
            }
        }
    }
}

fn varint_len(value: u64) -> usize {
    (64 - value.max(1).leading_zeros() as usize).div_ceil(7)
}

impl ColumnData {
    fn dictionary() -> ColumnData {
        ColumnData { dict: Some(Dictionary::default()), ..ColumnData::default() }
    }
    fn dict(&mut self) -> &mut Dictionary {
        self.dict.as_mut().expect("a dictionary column")
    }
    fn dict_int32(&mut self, value: i32) {
        let dict = self.dict();
        let index = dict.index_of(value as u32 as u64, |out| out.extend_from_slice(&value.to_le_bytes()));
        dict.indices.push(index);
    }
    fn dict_int64(&mut self, value: i64) {
        let dict = self.dict();
        let index = dict.index_of(value as u64, |out| out.extend_from_slice(&value.to_le_bytes()));
        dict.indices.push(index);
    }
    /// A string identified by `key` (equal keys, equal strings).
    fn dict_keyed_bytes(&mut self, key: u64, value: &[u8]) {
        let dict = self.dict();
        let index = dict.index_of(key, |out| {
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(value);
        });
        dict.indices.push(index);
    }
    /// A string equal to the previous row's when `repeat`, else a new
    /// dictionary entry written by `fill` (rows come grouped by person).
    fn dict_next_bytes(&mut self, repeat: bool, fill: impl FnOnce(&mut Vec<u8>)) {
        let dict = self.dict();
        let index = if repeat && dict.count > 0 {
            dict.count - 1
        } else {
            dict.add(|out| {
                let at = out.len();
                out.extend_from_slice(&[0; 4]);
                fill(out);
                let length = (out.len() - at - 4) as u32;
                out[at..at + 4].copy_from_slice(&length.to_le_bytes());
            })
        };
        dict.indices.push(index);
    }
    fn int32(&mut self, value: i32) {
        self.values.extend_from_slice(&value.to_le_bytes());
    }
    fn int64(&mut self, value: i64) {
        self.values.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.values.extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.values.extend_from_slice(value);
    }
    fn null(&mut self) {
        self.levels.push(false);
    }
    fn present(&mut self) {
        self.levels.push(true);
    }
}

struct ChunkMeta {
    /// Start of the column chunk (its dictionary page, if any).
    offset: u64,
    size: u64,
    values: usize,
    data_offset: u64,
    dictionary: bool,
}

struct Writer {
    file: Vec<u8>,
    row_groups: Vec<(usize, Vec<ChunkMeta>)>,
}

impl Writer {
    fn new() -> Writer {
        Writer { file: b"PAR1".to_vec(), row_groups: Vec::new() }
    }

    fn row_group(&mut self, schema: &[Column], columns: &[ColumnData], rows: usize) {
        let mut metas = Vec::new();
        for (spec, data) in schema.iter().zip(columns) {
            let mut page = Vec::new();
            if spec.optional {
                // Definition levels, bit width 1: one bit-packed run, length-prefixed.
                let groups = rows.div_ceil(8);
                let mut levels = Vec::with_capacity(groups + 10);
                let mut header = ((groups as u64) << 1) | 1;
                while header >= 0x80 {
                    levels.push(header as u8 | 0x80);
                    header >>= 7;
                }
                levels.push(header as u8);
                let mut packed = vec![0u8; groups];
                for (index, &present) in data.levels.iter().enumerate() {
                    packed[index / 8] |= (present as u8) << (index % 8);
                }
                levels.extend_from_slice(&packed);
                page.extend_from_slice(&(levels.len() as u32).to_le_bytes());
                page.extend_from_slice(&levels);
            }
            let offset = self.file.len() as u64;
            let dict = data.dict.as_ref().filter(|dict| dict.count > 0);
            if let Some(dict) = dict {
                // Dictionary page: the distinct values, PLAIN.
                let mut header = Thrift::new();
                header.i32(1, PAGE_DICTIONARY);
                header.i32(2, dict.values.len() as i32);
                header.i32(3, dict.values.len() as i32);
                header.begin(Some(7));
                header.i32(1, dict.count as i32);
                header.i32(2, ENCODING_PLAIN);
                header.end();
                header.out.push(0);
                self.file.extend_from_slice(&header.out);
                self.file.extend_from_slice(&dict.values);
                // Data page values: the bit width, then the hybrid-encoded indices.
                let width = (32 - (dict.count - 1).leading_zeros()).max(1);
                page.push(width as u8);
                hybrid(&dict.indices, width, &mut page);
            } else {
                page.extend_from_slice(&data.values);
            }

            let mut header = Thrift::new();
            header.i32(1, PAGE_DATA);
            header.i32(2, page.len() as i32);
            header.i32(3, page.len() as i32);
            header.begin(Some(5));
            header.i32(1, rows as i32);
            header.i32(2, if dict.is_some() { ENCODING_RLE_DICTIONARY } else { ENCODING_PLAIN });
            header.i32(3, ENCODING_RLE);
            header.i32(4, ENCODING_RLE);
            header.end();
            header.out.push(0);

            let data_offset = self.file.len() as u64;
            self.file.extend_from_slice(&header.out);
            self.file.extend_from_slice(&page);
            metas.push(ChunkMeta {
                offset,
                size: self.file.len() as u64 - offset,
                values: rows,
                data_offset,
                dictionary: dict.is_some(),
            });
        }
        self.row_groups.push((rows, metas));
    }

    fn finish(mut self, schema: &[Column], path: &Path) {
        let total_rows: usize = self.row_groups.iter().map(|(rows, _)| rows).sum();
        let mut meta = Thrift::new();
        meta.i32(1, 1);
        meta.list_header(2, T_STRUCT, schema.len() + 1);
        meta.begin(None);
        meta.string(4, b"schema");
        meta.i32(5, schema.len() as i32);
        meta.end();
        for spec in schema {
            meta.begin(None);
            meta.i32(1, spec.physical);
            meta.i32(3, if spec.optional { OPTIONAL } else { REQUIRED });
            meta.string(4, spec.name.as_bytes());
            if let Some(converted) = spec.converted {
                meta.i32(6, converted);
                // logicalType (a union): STRING is field 1, DATE is field 6; both empty structs.
                meta.begin(Some(10));
                meta.begin(Some(if converted == CONVERTED_DATE { 6 } else { 1 }));
                meta.end();
                meta.end();
            }
            meta.end();
        }
        meta.i64(3, total_rows as i64);
        meta.list_header(4, T_STRUCT, self.row_groups.len());
        for (rows, chunks) in &self.row_groups {
            meta.begin(None);
            meta.list_header(1, T_STRUCT, chunks.len());
            for (spec, chunk) in schema.iter().zip(chunks) {
                meta.begin(None);
                meta.i64(2, chunk.offset as i64);
                meta.begin(Some(3));
                meta.i32(1, spec.physical);
                if chunk.dictionary {
                    meta.list_header(2, T_I32, 3);
                    meta.zigzag(ENCODING_PLAIN as i64);
                    meta.zigzag(ENCODING_RLE as i64);
                    meta.zigzag(ENCODING_RLE_DICTIONARY as i64);
                } else {
                    meta.list_header(2, T_I32, 2);
                    meta.zigzag(ENCODING_PLAIN as i64);
                    meta.zigzag(ENCODING_RLE as i64);
                }
                meta.list_header(3, T_BINARY, 1);
                meta.varint(spec.name.len() as u64);
                meta.out.extend_from_slice(spec.name.as_bytes());
                meta.i32(4, CODEC_UNCOMPRESSED);
                meta.i64(5, chunk.values as i64);
                meta.i64(6, chunk.size as i64);
                meta.i64(7, chunk.size as i64);
                meta.i64(9, chunk.data_offset as i64);
                if chunk.dictionary {
                    meta.i64(11, chunk.offset as i64);
                }
                meta.end();
                meta.end();
            }
            let bytes: u64 = chunks.iter().map(|chunk| chunk.size).sum();
            meta.i64(2, bytes as i64);
            meta.i64(3, *rows as i64);
            meta.end();
        }
        meta.string(6, b"pipeline");
        meta.out.push(0);

        self.file.extend_from_slice(&meta.out);
        self.file.extend_from_slice(&(meta.out.len() as u32).to_le_bytes());
        self.file.extend_from_slice(b"PAR1");
        write_file(path, &self.file);
    }
}

/// Writes a whole output file. `PIPELINE_FSYNC=1` (diagnostics) also flushes
/// it to disk before returning, which moves the flush the container's exit
/// otherwise waits for into the phase log.
fn write_file(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let fail = |error: std::io::Error| -> ! { panic!("write {}: {error}", path.display()) };
    let mut file = std::fs::File::create(path).unwrap_or_else(|e| fail(e));
    file.write_all(bytes).unwrap_or_else(|e| fail(e));
    if std::env::var_os("PIPELINE_FSYNC").is_some_and(|v| v == "1") {
        file.sync_data().unwrap_or_else(|e| fail(e));
    }
}

fn write_user_daily(path: &Path, rows: &[UserDay], lookups: &Lookups, strings: &Strings) {
    let mut writer = Writer::new();
    for group in rows.chunks(ROW_GROUP_ROWS) {
        if std::env::var_os("PIPELINE_PLAIN_OUTPUT").is_some_and(|v| v == "1") {
            // A/B switch: the previous PLAIN-only layout.
            let mut columns: Vec<ColumnData> = (0..USER_DAILY.len()).map(|_| ColumnData::default()).collect();
            for row in group {
                columns[0].bytes(&strings.bytes(row.person));
                columns[1].int32(row.day);
                columns[2].int64(row.sessions as i64);
                columns[3].int64(row.events as i64);
                columns[4].int64(row.revenue as i64);
                if row.top == NO_CATEGORY {
                    columns[5].null();
                } else {
                    columns[5].present();
                    columns[5].bytes(&lookups.categories[row.top as usize]);
                }
            }
            writer.row_group(&USER_DAILY, &columns, group.len());
            continue;
        }
        let mut columns: Vec<ColumnData> = (0..USER_DAILY.len()).map(|_| ColumnData::dictionary()).collect();
        let mut previous = None;
        for row in group {
            // A person's rows are adjacent: one dictionary entry per run.
            columns[0].dict_next_bytes(previous == Some(row.person), |out| strings.append(row.person, out));
            previous = Some(row.person);
            columns[1].dict_int32(row.day);
            columns[2].dict_int64(row.sessions as i64);
            columns[3].dict_int64(row.events as i64);
            columns[4].dict_int64(row.revenue as i64);
            if row.top == NO_CATEGORY {
                columns[5].null();
            } else {
                columns[5].present();
                columns[5].dict_keyed_bytes(row.top as u64, &lookups.categories[row.top as usize]);
            }
        }
        writer.row_group(&USER_DAILY, &columns, group.len());
    }
    writer.finish(&USER_DAILY, path);
}

pub fn write_all(output: &Path, results: Vec<ThreadResult>, lookups: &Lookups, window: &Window, strings: &Strings, threads: usize) {
    let user_dir = output.join("user_daily");
    let campaign_dir = output.join("campaign_daily");
    std::fs::create_dir_all(&user_dir).expect("create user_daily");
    std::fs::create_dir_all(&campaign_dir).expect("create campaign_daily");

    let days = window.days;
    let user_rows: Vec<Vec<UserDay>> = results.iter().map(|_| Vec::new()).collect();
    let mut totals = vec![[0u64; 4]; (lookups.campaign_names.len() + 1) * days];
    let mut user_rows = user_rows;
    for (slot, result) in user_rows.iter_mut().zip(results) {
        for (total, counts) in totals.iter_mut().zip(&result.campaigns) {
            for k in 0..4 {
                total[k] += counts[k];
            }
        }
        *slot = result.user_days;
    }

    let jobs = user_rows.len().max(1);
    par::run(threads, jobs + 1, |_| (), |_, job| {
        if job < jobs {
            let rows: &[UserDay] = user_rows.get(job).map_or(&[], |rows| rows.as_slice());
            if rows.is_empty() && job != 0 {
                return;
            }
            write_user_daily(&user_dir.join(format!("part-{job:03}.parquet")), rows, lookups, strings);
            return;
        }
        // campaign_daily: one row per (campaign, day) with a credited or assisted session.
        let mut columns: Vec<ColumnData> = (0..CAMPAIGN_DAILY.len()).map(|_| ColumnData::default()).collect();
        let mut rows = 0;
        let mut writer = Writer::new();
        for (index, counts) in totals.iter().enumerate() {
            if counts[0] == 0 && counts[3] == 0 {
                continue;
            }
            let campaign = index / days;
            let (name, channel): (&[u8], &[u8]) = if campaign == lookups.campaign_names.len() {
                (b"organic", b"organic")
            } else {
                (&lookups.campaign_names[campaign], &lookups.channels[campaign])
            };
            columns[0].bytes(name);
            columns[1].bytes(channel);
            columns[2].int32((window.first_day + (index % days) as i64) as i32);
            for k in 0..4 {
                columns[3 + k].int64(counts[k] as i64);
            }
            rows += 1;
            if rows == ROW_GROUP_ROWS {
                writer.row_group(&CAMPAIGN_DAILY, &columns, rows);
                columns = (0..CAMPAIGN_DAILY.len()).map(|_| ColumnData::default()).collect();
                rows = 0;
            }
        }
        if rows > 0 {
            writer.row_group(&CAMPAIGN_DAILY, &columns, rows);
        }
        writer.finish(&CAMPAIGN_DAILY, &campaign_dir.join("part-000.parquet"));
    });
}
