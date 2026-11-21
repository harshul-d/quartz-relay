//! The CSV dialect (rules.md "CSV dialect") and newline-aligned input chunks.

use std::fs::File;
use std::path::{Path, PathBuf};

use memchr::{memchr, memchr_iter};

/// Removes the quotes around a whole field. `None` means a quote somewhere
/// else, which makes the whole line malformed.
#[inline(always)]
fn unquote(field: &[u8]) -> Option<&[u8]> {
    if memchr(b'"', field).is_none() {
        return Some(field);
    }
    let length = field.len();
    if length >= 2 && field[0] == b'"' && field[length - 1] == b'"' && memchr(b'"', &field[1..length - 1]).is_none() {
        Some(&field[1..length - 1])
    } else {
        None
    }
}

/// Splits a line (without its `\n`) into exactly `N` unquoted values; an
/// empty value is null. `None` means malformed: the wrong number of fields,
/// a stray quote, or an empty line. `\r` and spaces are ordinary bytes.
#[inline(always)]
pub fn split<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    if line.is_empty() {
        return None;
    }
    let mut fields: [&[u8]; N] = [&[]; N];
    let mut count = 0;
    let mut start = 0;
    for comma in memchr_iter(b',', line) {
        if count + 1 >= N {
            return None;
        }
        fields[count] = &line[start..comma];
        count += 1;
        start = comma + 1;
    }
    if count != N - 1 {
        return None;
    }
    fields[N - 1] = &line[start..];
    if memchr(b'"', line).is_some() {
        for field in fields.iter_mut() {
            *field = unquote(field)?;
        }
    }
    Some(fields)
}

/// Calls `each` with every line of `bytes` (a run of whole lines). Empty
/// lines are malformed in every table, so they are skipped here.
#[inline(always)]
pub fn for_each_line(bytes: &[u8], mut each: impl FnMut(&[u8])) {
    let mut start = 0;
    for newline in memchr_iter(b'\n', bytes) {
        if newline > start {
            each(&bytes[start..newline]);
        }
        start = newline + 1;
    }
    if start < bytes.len() {
        each(&bytes[start..]);
    }
}

/// Bit masks of `,`, `\n` and `"` in a 64-byte block (bit i = byte i).
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
fn masks(block: &[u8; 64]) -> (u64, u64, u64) {
    use std::arch::x86_64::*;
    // SAFETY: AVX2 is enabled at compile time and the loads read 2 x 32 bytes of a 64-byte array.
    unsafe {
        let low = _mm256_loadu_si256(block.as_ptr() as *const __m256i);
        let high = _mm256_loadu_si256(block.as_ptr().add(32) as *const __m256i);
        let mask = |byte: u8| -> u64 {
            let needle = _mm256_set1_epi8(byte as i8);
            let l = _mm256_movemask_epi8(_mm256_cmpeq_epi8(low, needle)) as u32 as u64;
            let h = _mm256_movemask_epi8(_mm256_cmpeq_epi8(high, needle)) as u32 as u64;
            l | (h << 32)
        };
        (mask(b','), mask(b'\n'), mask(b'"'))
    }
}

#[cfg(all(target_arch = "x86_64", not(target_feature = "avx2")))]
#[inline(always)]
fn masks(block: &[u8; 64]) -> (u64, u64, u64) {
    use std::arch::x86_64::*;
    // SAFETY: SSE2 is part of x86_64; the loads read 4 x 16 bytes of a 64-byte array.
    unsafe {
        let parts: [__m128i; 4] = std::array::from_fn(|k| _mm_loadu_si128(block.as_ptr().add(16 * k) as *const __m128i));
        let mask = |byte: u8| -> u64 {
            let needle = _mm_set1_epi8(byte as i8);
            let mut out = 0u64;
            for (k, part) in parts.iter().enumerate() {
                out |= (_mm_movemask_epi8(_mm_cmpeq_epi8(*part, needle)) as u16 as u64) << (16 * k);
            }
            out
        };
        (mask(b','), mask(b'\n'), mask(b'"'))
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
fn masks(block: &[u8; 64]) -> (u64, u64, u64) {
    let (mut comma, mut newline, mut quote) = (0u64, 0u64, 0u64);
    for (i, &byte) in block.iter().enumerate() {
        comma |= ((byte == b',') as u64) << i;
        newline |= ((byte == b'\n') as u64) << i;
        quote |= ((byte == b'"') as u64) << i;
    }
    (comma, newline, quote)
}

/// Hands one finished line's fields to `each`, unquoted, unless malformed.
/// `quotes` is the number of `"` in the line: each must be the first or
/// last byte of a field that it wraps, so the count must equal twice the
/// number of wrapped fields.
#[inline(always)]
fn emit<const N: usize>(bytes: &[u8], spans: &[(usize, usize); N], quotes: u32, each: &mut impl FnMut(&[&[u8]; N])) {
    let mut values: [&[u8]; N] = [&[]; N];
    // Branchless: quoting is random per field, so a branch would mispredict.
    let mut wrapped = 0u32;
    let length = bytes.len();
    for k in 0..N {
        let (start, end) = spans[k];
        // The tokenizer guarantees start <= end <= length, and length > 0.
        debug_assert!(start <= end && end <= length && length > 0);
        let long_enough = end - start >= 2;
        // For a field shorter than 2 bytes these reads are clamped and ignored.
        let first_at = start.min(length - 1);
        let last_at = end.max(1) - 1;
        // SAFETY: both indices are below `length` (see above); when `quoted`
        // is 1 the field has at least 2 bytes, so start + 1 <= end - 1.
        unsafe {
            let first = *bytes.get_unchecked(first_at);
            let last = *bytes.get_unchecked(last_at);
            let quoted = (long_enough & (first == b'"') & (last == b'"')) as usize;
            values[k] = bytes.get_unchecked(start + quoted..end - quoted);
            wrapped += 2 * quoted as u32;
        }
    }
    if wrapped != quotes {
        return;
    }
    each(&values);
}

/// Whether `for_each_record` splits line by line (default) or walks every
/// separator of every block (`PIPELINE_SPLIT=blocks`, for A/B runs).
fn split_lines() -> bool {
    static LINES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LINES.get_or_init(|| std::env::var("PIPELINE_SPLIT").map_or(true, |v| v != "blocks"))
}

/// Calls `each` with the `N` unquoted values of every well-formed line of
/// `bytes` (a run of whole lines; the last may lack its `\n`). Malformed and
/// empty lines are skipped. Same result as `for_each_line` + `split::<N>`.
#[inline(always)]
pub fn for_each_record<const N: usize>(bytes: &[u8], each: impl FnMut(&[&[u8]; N])) {
    if split_lines() { for_each_record_lines(bytes, each) } else { for_each_record_blocks(bytes, each) }
}

/// Hands a line's fields to `each`: directly when the line has no quote,
/// else through `emit`, which unwraps quoted fields and checks the count.
#[inline(always)]
fn finish<const N: usize>(bytes: &[u8], spans: &[(usize, usize); N], quotes: u32, each: &mut impl FnMut(&[&[u8]; N])) {
    if quotes != 0 {
        return emit(bytes, spans, quotes, each);
    }
    let values = spans.map(|(start, end)| {
        debug_assert!(start <= end && end <= bytes.len());
        // SAFETY: every caller builds ordered spans inside `bytes`.
        unsafe { bytes.get_unchecked(start..end) }
    });
    each(&values);
}

/// A bit mask over a line window (`u64`: 64 bytes, `u128`: 128 bytes).
trait Bits: Copy {
    fn ones(self) -> u32;
    fn lowest(self) -> usize;
    fn rest(self) -> Self;
}

macro_rules! bits {
    ($($t:ty),*) => {$(
        impl Bits for $t {
            #[inline(always)]
            fn ones(self) -> u32 { self.count_ones() }
            #[inline(always)]
            fn lowest(self) -> usize { self.trailing_zeros() as usize }
            #[inline(always)]
            fn rest(self) -> Self { self & self.wrapping_sub(1) }
        }
    )*};
}
bits!(u64, u128);

/// Cuts the line `bytes[start..end]` at its commas: `comma` and `quote` mark
/// bytes of this line only (bit i = byte start + i, all below end - start).
#[inline(always)]
fn cut<const N: usize, B: Bits>(bytes: &[u8], start: usize, end: usize, mut comma: B, quote: B, each: &mut impl FnMut(&[&[u8]; N])) {
    if comma.ones() as usize != N - 1 {
        return;
    }
    let mut spans = [(0usize, 0usize); N];
    let mut from = start;
    for span in spans[..N - 1].iter_mut() {
        let at = start + comma.lowest();
        debug_assert!(from <= at && at < end);
        *span = (from, at);
        from = at + 1;
        comma = comma.rest();
    }
    debug_assert!(from <= end && end <= bytes.len());
    spans[N - 1] = (from, end);
    finish(bytes, &spans, quote.ones(), each);
}

/// `cut` for a line too long for a 128-byte window, with `memchr`.
#[cold]
#[inline(never)]
fn cut_long<const N: usize>(bytes: &[u8], start: usize, end: usize, each: &mut impl FnMut(&[&[u8]; N])) {
    let line = &bytes[start..end];
    let mut spans = [(0usize, 0usize); N];
    let mut count = 0;
    let mut from = start;
    for comma in memchr_iter(b',', line) {
        if count + 1 >= N {
            return;
        }
        spans[count] = (from, start + comma);
        count += 1;
        from = start + comma + 1;
    }
    if count != N - 1 {
        return;
    }
    spans[N - 1] = (from, end);
    finish(bytes, &spans, memchr_iter(b'"', line).count() as u32, each);
}

/// `masks` of the 64 bytes at `at`; past the end of `bytes` the window is
/// zero padding, which has no bits.
#[inline(always)]
fn window(bytes: &[u8], at: usize) -> (u64, u64, u64) {
    match bytes.get(at..at + 64) {
        Some(block) => masks(block.try_into().unwrap()),
        None => tail_window(&bytes[at..]),
    }
}

#[cold]
#[inline(never)]
fn tail_window(rest: &[u8]) -> (u64, u64, u64) {
    let mut padded = [0u8; 64];
    padded[..rest.len()].copy_from_slice(rest);
    masks(&padded)
}

/// `for_each_record` that splits each line in one step: the masks of a 64-
/// (or 128-) byte window from the line start give its end (first newline),
/// its commas (exactly N - 1 before the end, else malformed) and whether it
/// has quotes at all (only then are fields unwrapped). Longer lines take a
/// `memchr` path.
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[inline(always)]
pub fn for_each_record_lines<const N: usize>(bytes: &[u8], mut each: impl FnMut(&[&[u8]; N])) {
    let length = bytes.len();
    let mut start = 0;
    while start < length {
        let (comma, newline, quote) = window(bytes, start);
        if newline != 0 {
            let inside = !newline & newline.wrapping_sub(1);
            let end = start + newline.trailing_zeros() as usize;
            cut(bytes, start, end, comma & inside, quote & inside, &mut each);
            start = end + 1;
            continue;
        }
        if start + 64 >= length {
            // The last line, without `\n`; the window has no bits past it.
            cut(bytes, start, length, comma, quote, &mut each);
            return;
        }
        let (comma_high, newline_high, quote_high) = window(bytes, start + 64);
        let comma = comma as u128 | (comma_high as u128) << 64;
        let quote = quote as u128 | (quote_high as u128) << 64;
        if newline_high != 0 {
            let newline = (newline_high as u128) << 64;
            let inside = !newline & newline.wrapping_sub(1);
            let end = start + newline.trailing_zeros() as usize;
            cut(bytes, start, end, comma & inside, quote & inside, &mut each);
            start = end + 1;
            continue;
        }
        if start + 128 >= length {
            cut(bytes, start, length, comma, quote, &mut each);
            return;
        }
        let end = memchr(b'\n', &bytes[start + 128..]).map_or(length, |at| start + 128 + at);
        cut_long(bytes, start, end, &mut each);
        start = end + 1;
    }
}

/// Without AVX2 the line splitter is no faster: walk the blocks.
#[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
#[inline(always)]
pub fn for_each_record_lines<const N: usize>(bytes: &[u8], each: impl FnMut(&[&[u8]; N])) {
    for_each_record_blocks(bytes, each)
}

/// `for_each_record` from one SIMD pass that walks every separator of every
/// 64-byte block, branching on newline per separator.
#[inline(always)]
pub fn for_each_record_blocks<const N: usize>(bytes: &[u8], mut each: impl FnMut(&[&[u8]; N])) {
    let length = bytes.len();
    let mut spans = [(0usize, 0usize); N];
    let mut count = 0usize; // fields ended so far in this line
    let mut field_start = 0usize;
    let mut line_quotes = 0u32;
    let mut base = 0usize;
    let mut tail = [0u8; 64];
    while base < length {
        let block: &[u8; 64] = if base + 64 <= length {
            bytes[base..base + 64].try_into().unwrap()
        } else {
            tail[..length - base].copy_from_slice(&bytes[base..]);
            &tail
        };
        let (comma, newline, quote) = masks(block);
        let mut separators = comma | newline;
        let mut counted = 0u32; // quotes of this block already added to a line
        while separators != 0 {
            let bit = separators.trailing_zeros();
            separators &= separators - 1;
            let at = base + bit as usize;
            if count < N {
                spans[count] = (field_start, at);
            }
            count += 1;
            field_start = at + 1;
            if (newline >> bit) & 1 != 0 {
                let before = (quote & ((1u64 << bit) - 1)).count_ones();
                line_quotes += before - counted;
                counted = before;
                if count == N {
                    emit(bytes, &spans, line_quotes, &mut each);
                }
                count = 0;
                line_quotes = 0;
            }
        }
        line_quotes += quote.count_ones() - counted;
        base += 64;
    }
    if length > 0 && bytes[length - 1] != b'\n' {
        // The last line has no `\n`; it may still end in an empty field.
        if count < N {
            spans[count] = (field_start, length);
        }
        count += 1;
        if count == N {
            emit(bytes, &spans, line_quotes, &mut each);
        }
    }
}

/// Reads as many bytes as are there at `offset`, up to `buffer.len()`.
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        #[cfg(unix)]
        let result = std::os::unix::fs::FileExt::read_at(file, &mut buffer[filled..], offset + filled as u64);
        #[cfg(windows)]
        let result = std::os::windows::fs::FileExt::seek_read(file, &mut buffer[filled..], offset + filled as u64);
        match result {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => panic!("read: {error}"),
        }
    }
    filled
}

/// The data lines of a whole small file (everything after its header line).
pub fn read_data_lines(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    match memchr(b'\n', &bytes) {
        Some(at) => bytes[at + 1..].to_vec(),
        None => Vec::new(),
    }
}

/// Every input file of one table. Chunks of them are read with positional
/// reads into a reused per-thread buffer: no page faults on the input, and
/// the bytes are hot in cache when parsed.
pub struct Files {
    files: Vec<File>,
    lengths: Vec<u64>,
    /// Memory maps of the files (Linux, `PIPELINE_READ=mmap*`), else empty.
    maps: Vec<Map>,
    populate_chunks: bool,
}

/// How input is read: `PIPELINE_READ` = `pread` (default), `mmap` (lazy),
/// `mmap-populate` (MAP_POPULATE), `mmap-seq` (MADV_SEQUENTIAL),
/// `mmap-willneed` (MADV_WILLNEED), `mmap-chunk` (MADV_POPULATE_READ per
/// chunk, in the worker that parses it).
#[derive(Clone, Copy, PartialEq)]
enum ReadMode {
    Pread,
    Mmap { flags: i32, advice: i32, chunk: bool },
}

fn read_mode() -> ReadMode {
    static MODE: std::sync::OnceLock<ReadMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| {
        const MAP_POPULATE: i32 = 0x8000;
        const MADV_SEQUENTIAL: i32 = 2;
        const MADV_WILLNEED: i32 = 3;
        let mode = std::env::var("PIPELINE_READ").unwrap_or_default();
        let mmap = |flags, advice, chunk| ReadMode::Mmap { flags, advice, chunk };
        match mode.as_str() {
            _ if !cfg!(target_os = "linux") => ReadMode::Pread,
            "mmap" => mmap(0, 0, false),
            "mmap-populate" => mmap(MAP_POPULATE, 0, false),
            "mmap-seq" => mmap(0, MADV_SEQUENTIAL, false),
            "mmap-willneed" => mmap(0, MADV_WILLNEED, false),
            "mmap-chunk" => mmap(0, 0, true),
            _ => ReadMode::Pread,
        }
    })
}

/// A read-only private mapping of a whole file.
struct Map {
    at: *const u8,
    length: usize,
}

// SAFETY: the mapping is read-only and lives as long as the `Map`.
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

#[cfg(target_os = "linux")]
mod sys {
    unsafe extern "C" {
        pub fn mmap(addr: *mut u8, length: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
        pub fn munmap(addr: *mut u8, length: usize) -> i32;
        pub fn madvise(addr: *mut u8, length: usize, advice: i32) -> i32;
    }
    pub const PROT_READ: i32 = 1;
    pub const MAP_PRIVATE: i32 = 2;
    pub const MADV_POPULATE_READ: i32 = 22;
}

impl Map {
    #[cfg(target_os = "linux")]
    fn new(file: &File, length: u64, flags: i32, advice: i32) -> Option<Map> {
        use std::os::fd::AsRawFd;
        let length = length as usize;
        // SAFETY: a fresh read-only private mapping of an open file.
        let at = unsafe {
            sys::mmap(std::ptr::null_mut(), length, sys::PROT_READ, sys::MAP_PRIVATE | flags, file.as_raw_fd(), 0)
        };
        if at as isize == -1 || at.is_null() {
            return None;
        }
        if advice != 0 {
            // SAFETY: the range is the mapping just made; advice is a hint.
            unsafe { sys::madvise(at, length, advice) };
        }
        Some(Map { at, length })
    }

    #[cfg(not(target_os = "linux"))]
    fn new(_file: &File, _length: u64, _flags: i32, _advice: i32) -> Option<Map> {
        None
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping covers `length` readable bytes while `self` lives.
        unsafe { std::slice::from_raw_parts(self.at, self.length) }
    }

    /// Faults `range` in with one call (MADV_POPULATE_READ, Linux 5.14+).
    fn populate(&self, start: usize, end: usize) {
        #[cfg(target_os = "linux")]
        {
            const PAGE: usize = 4096;
            let from = start & !(PAGE - 1);
            // SAFETY: a page-aligned range inside the mapping; only a hint.
            unsafe { sys::madvise(self.at.add(from) as *mut u8, end - from, sys::MADV_POPULATE_READ) };
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (start, end);
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        // SAFETY: unmapping the mapping this `Map` owns.
        unsafe {
            sys::munmap(self.at as *mut u8, self.length);
        }
    }
}

impl Files {
    /// Every regular `*.csv` file in `folder` (names starting with `.` are
    /// skipped). A missing folder has no files; empty files are left out.
    pub fn open_folder(folder: &Path) -> Files {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if !name.ends_with(".csv") || name.starts_with('.') {
                    continue;
                }
                let path = entry.path();
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        Files::open(&paths)
    }

    pub fn open(paths: &[PathBuf]) -> Files {
        let mut files = Vec::new();
        let mut lengths = Vec::new();
        for path in paths {
            let file = File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
            let length = file.metadata().map(|m| m.len()).unwrap_or(0);
            if length > 0 {
                files.push(file);
                lengths.push(length);
            }
        }
        let mut maps = Vec::new();
        let mut populate_chunks = false;
        if let ReadMode::Mmap { flags, advice, chunk } = read_mode() {
            let mapped: Option<Vec<Map>> =
                files.iter().zip(&lengths).map(|(file, &length)| Map::new(file, length, flags, advice)).collect();
            if let Some(mapped) = mapped {
                maps = mapped;
                populate_chunks = chunk;
            }
        }
        Files { files, lengths, maps, populate_chunks }
    }

    /// Total size of all the files, in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.lengths.iter().sum()
    }

    /// The offset just past the first `\n` at or after `from`, or the file
    /// length if there is none.
    fn next_line_start(&self, file: usize, from: u64) -> u64 {
        let length = self.lengths[file];
        let mut buffer = [0u8; 4096];
        let mut at = from;
        while at < length {
            let read = read_at(&self.files[file], &mut buffer, at);
            if read == 0 {
                break;
            }
            if let Some(newline) = memchr(b'\n', &buffer[..read]) {
                return at + newline as u64 + 1;
            }
            at += read as u64;
        }
        length
    }

    /// Splits the data lines (everything after each file's header line) into
    /// chunks of about `target` bytes that start and end on line boundaries.
    pub fn chunks(&self, target: u64) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        for file in 0..self.files.len() {
            let length = self.lengths[file];
            let mut start = self.next_line_start(file, 0); // skip the header line
            while start < length {
                let mut end = start.saturating_add(target).min(length);
                if end < length {
                    // The chunk ends after the line holding byte end - 1.
                    end = self.next_line_start(file, end - 1);
                }
                chunks.push(Chunk { file, start, end });
                start = end;
            }
        }
        chunks
    }

    /// The whole lines in the first `limit` bytes of a chunk (for sampling).
    pub fn head(&self, chunk: &Chunk, limit: usize) -> Vec<u8> {
        let length = ((chunk.end - chunk.start) as usize).min(limit);
        let mut buffer = vec![0u8; length];
        let read = read_at(&self.files[chunk.file], &mut buffer, chunk.start);
        buffer.truncate(read);
        if (chunk.start as usize + read) < chunk.end as usize {
            let keep = buffer.iter().rposition(|&b| b == b'\n').map_or(0, |at| at + 1);
            buffer.truncate(keep);
        }
        buffer
    }

    /// Reads one chunk into this thread's reused buffer and hands it to `f`.
    pub fn with_chunk<R>(&self, chunk: &Chunk, f: impl FnOnce(&[u8]) -> R) -> R {
        if let Some(map) = self.maps.get(chunk.file) {
            let (start, end) = (chunk.start as usize, chunk.end as usize);
            if self.populate_chunks {
                let begin = std::time::Instant::now();
                map.populate(start, end);
                crate::timing::READ.add_since(begin);
            }
            return f(&map.bytes()[start..end]);
        }
        thread_local! {
            static BUFFER: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            let length = (chunk.end - chunk.start) as usize;
            if buffer.len() < length {
                buffer.resize(length, 0);
            }
            let begin = std::time::Instant::now();
            let read = read_at(&self.files[chunk.file], &mut buffer[..length], chunk.start);
            crate::timing::READ.add_since(begin);
            f(&buffer[..read])
        })
    }
}

/// Asks the kernel, from a background thread, to start reading every input
/// file now (`POSIX_FADV_WILLNEED`), in the order the tables are processed,
/// so cold reads run ahead of parsing and overlap other work (measured: -5%
/// cold, no change warm). `PIPELINE_WILLNEED=0` turns it off for A/B runs.
pub fn prefetch_inputs(data: &Path) {
    if std::env::var_os("PIPELINE_WILLNEED").is_some_and(|v| v == "0") {
        return;
    }
    let data = data.to_path_buf();
    std::thread::spawn(move || {
        for table in ["links", "events", "clicks"] {
            let Ok(entries) = std::fs::read_dir(data.join(table)) else { continue };
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for path in paths {
                if let Ok(file) = File::open(&path) {
                    will_need(&file);
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn will_need(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
    }
    const POSIX_FADV_WILLNEED: i32 = 3;
    // SAFETY: a valid open descriptor; the call only gives the kernel a hint.
    unsafe { posix_fadvise(file.as_raw_fd(), 0, 0, POSIX_FADV_WILLNEED) };
}

#[cfg(not(target_os = "linux"))]
fn will_need(_file: &File) {}

#[derive(Clone, Copy)]
pub struct Chunk {
    pub file: usize,
    pub start: u64,
    pub end: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect() {
        assert_eq!(split::<2>(b"a,b"), Some([&b"a"[..], &b"b"[..]]));
        assert_eq!(split::<2>(b"\"a\",\"\""), Some([&b"a"[..], &b""[..]]));
        assert_eq!(split::<2>(b"a,b,c"), None);
        assert_eq!(split::<2>(b"a"), None);
        assert_eq!(split::<2>(b""), None);
        assert_eq!(split::<2>(b"a,b\""), None);
        assert_eq!(split::<2>(b"a,\""), None);
        assert_eq!(split::<2>(b"a,\"b\"c\""), None);
        assert_eq!(split::<2>(b" a,\" b\""), Some([&b" a"[..], &b" b"[..]]));
        assert_eq!(split::<2>(b"a,b\r"), Some([&b"a"[..], &b"b\r"[..]]));
    }

    fn simple<const N: usize>(bytes: &[u8]) -> Vec<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for_each_line(bytes, |line| {
            if let Some(fields) = split::<N>(line) {
                out.push(fields.iter().map(|f| f.to_vec()).collect());
            }
        });
        out
    }

    fn fast<const N: usize>(bytes: &[u8]) -> Vec<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for_each_record::<N>(bytes, |fields| out.push(fields.iter().map(|f| f.to_vec()).collect()));
        let mut blocks: Vec<Vec<Vec<u8>>> = Vec::new();
        for_each_record_blocks::<N>(bytes, |fields| blocks.push(fields.iter().map(|f| f.to_vec()).collect()));
        assert_eq!(out, blocks);
        out
    }

    /// Every record as (offset in `bytes`, value) per field.
    type Records = Vec<Vec<(usize, Vec<u8>)>>;

    fn records<const N: usize>(bytes: &[u8], lines: bool) -> Records {
        let mut out = Vec::new();
        let base = bytes.as_ptr() as usize;
        let mut push = |fields: &[&[u8]; N]| out.push(fields.iter().map(|f| (f.as_ptr() as usize - base, f.to_vec())).collect());
        if lines { for_each_record_lines::<N>(bytes, &mut push) } else { for_each_record_blocks::<N>(bytes, &mut push) }
        out
    }

    fn same<const N: usize>(bytes: &[u8], what: &str) -> usize {
        let lines = records::<N>(bytes, true);
        let blocks = records::<N>(bytes, false);
        if lines != blocks {
            panic!("{what}, N = {N}: {:?}\nlines  {lines:?}\nblocks {blocks:?}", String::from_utf8_lossy(bytes));
        }
        lines.len()
    }

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A dirty line of about `length` bytes with `fields` fields (before its own dirt).
    fn dirty_line(random: &mut Random, fields: usize, length: usize, out: &mut Vec<u8>) {
        let alphabet = b"abcxyz0189 ,\"\r\n";
        let width = length / fields.max(1);
        for field in 0..fields {
            if field > 0 {
                out.push(b',');
            }
            let quoted = random.below(3) == 0;
            if quoted {
                out.push(b'"');
            }
            for _ in 0..random.below(2 * width + 1) {
                // Mostly clean bytes; separators and quotes now and then.
                let byte = if random.below(40) == 0 { alphabet[10 + random.below(5)] } else { alphabet[random.below(10)] };
                out.push(byte);
            }
            if quoted {
                out.push(b'"');
            }
        }
    }

    /// The line splitter agrees with the block walker, slices included, on
    /// lines of 0..300 bytes (64- and 128-byte windows, the long path, the
    /// padded tail), with and without a final `\n`, at lengths near multiples of 64.
    #[test]
    fn lines_match_blocks() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        let (mut inputs, mut found) = (0usize, 0usize);
        for round in 0..30_000 {
            let n = [2, 4, 7][round % 3];
            let mut bytes = Vec::new();
            for _ in 0..random.below(8) {
                let fields = if random.below(4) == 0 { random.below(n + 3) } else { n };
                let length = random.below(300);
                dirty_line(&mut random, fields, length, &mut bytes);
                bytes.push(b'\n');
            }
            let mut variants = vec![bytes.clone()];
            if bytes.last() == Some(&b'\n') {
                variants.push(bytes[..bytes.len() - 1].to_vec());
            }
            // Cut within 2 bytes of two multiples of 64 the input reaches.
            for _ in 0..2 {
                let multiple = 64 * (1 + random.below(bytes.len() / 64 + 1));
                for cut in multiple - 2..=(multiple + 2).min(bytes.len()) {
                    variants.push(bytes[..cut].to_vec());
                }
            }
            for variant in &variants {
                let what = format!("round {round}");
                found += same::<2>(variant, &what) + same::<4>(variant, &what) + same::<7>(variant, &what);
                inputs += 1;
            }
        }
        // Raw random bytes from the dialect's alphabet.
        let alphabet = b"ab,,\"\"\n\n\r x";
        for round in 0..20_000 {
            let bytes: Vec<u8> = (0..random.below(400)).map(|_| alphabet[random.below(alphabet.len())]).collect();
            let what = format!("raw round {round}");
            found += same::<2>(&bytes, &what) + same::<4>(&bytes, &what) + same::<7>(&bytes, &what);
            inputs += 1;
        }
        println!("{inputs} inputs, {found} records");
        assert!(found > 100_000);
    }

    /// Realistic rows (events: 7 fields, clicks: 4, links: 2) with 20% of
    /// fields quoted and a little dirt, in chunks of 1000 lines.
    #[test]
    fn lines_match_blocks_realistic() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        let types = [&b"view"[..], b"add_to_cart", b"purchase", b"remove_from_cart"];
        let mut total = 0usize;
        for chunk in 0..300 {
            let n = [7, 7, 4, 2][chunk % 4];
            let mut bytes = Vec::new();
            let mut lines = 0;
            for _ in 0..1000 {
                let mut fields: Vec<Vec<u8>> = match n {
                    7 => vec![
                        format!("e{}", random.next() % 100_000_000).into_bytes(),
                        format!("u{}", random.next() % 1_000_000).into_bytes(),
                        format!("2024-03-{:02}T{:02}:{:02}:{:02}Z", 1 + random.below(28), random.below(24), random.below(60), random.below(60)).into_bytes(),
                        format!("2024-03-{:02} {:02}:{:02}:{:02}.{:03}", 1 + random.below(28), random.below(24), random.below(60), random.below(60), random.below(1000)).into_bytes(),
                        types[random.below(types.len())].to_vec(),
                        if random.below(10) == 0 { Vec::new() } else { format!("p{}", random.next() % 50_000).into_bytes() },
                        format!("{}.{:02}", random.below(500), random.below(100)).into_bytes(),
                    ],
                    4 => vec![
                        format!("c{}", random.next() % 100_000_000).into_bytes(),
                        format!("u{}", random.next() % 1_000_000).into_bytes(),
                        format!("camp_{}", random.below(500)).into_bytes(),
                        format!("2024-03-{:02}T{:02}:{:02}:{:02}Z", 1 + random.below(28), random.below(24), random.below(60), random.below(60)).into_bytes(),
                    ],
                    _ => vec![format!("u{}", random.next() % 1_000_000).into_bytes(), format!("x{}", random.next() % 1_000_000).into_bytes()],
                };
                match random.below(100) {
                    0 => fields[random.below(n)].push(b'"'),
                    1 => fields[random.below(n)].push(b','),
                    2 => fields[n - 1].push(b'\r'),
                    3 => fields[random.below(n)].insert(0, b' '),
                    _ => {}
                }
                for (k, field) in fields.iter().enumerate() {
                    if k > 0 {
                        bytes.push(b',');
                    }
                    let quoted = random.below(5) == 0;
                    if quoted {
                        bytes.push(b'"');
                    }
                    bytes.extend_from_slice(field);
                    if quoted {
                        bytes.push(b'"');
                    }
                }
                bytes.push(b'\n');
                lines += 1;
                if random.below(200) == 0 {
                    bytes.push(b'\n');
                }
            }
            if chunk % 2 == 1 {
                bytes.pop();
            }
            let what = format!("chunk {chunk}");
            let found = match n {
                7 => same::<7>(&bytes, &what),
                4 => same::<4>(&bytes, &what),
                _ => same::<2>(&bytes, &what),
            };
            assert!(found > lines * 9 / 10, "{found} of {lines}");
            total += lines;
        }
        println!("{total} realistic lines");
    }

    /// The SIMD tokenizer agrees with the plain one on random dirty input.
    #[test]
    fn tokenizer_matches_simple_split() {
        let alphabet = b"ab,,\"\"\n\n\r x";
        let mut state = 0x1234_5678_9ABC_DEF1u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..20_000 {
            let length = (next() % 300) as usize;
            let bytes: Vec<u8> = (0..length).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
            assert_eq!(fast::<2>(&bytes), simple::<2>(&bytes), "round {round}: {:?}", String::from_utf8_lossy(&bytes));
            assert_eq!(fast::<4>(&bytes), simple::<4>(&bytes), "round {round}");
            assert_eq!(fast::<7>(&bytes), simple::<7>(&bytes), "round {round}");
        }
        // Mostly well-formed lines with sparse dirt, across many block offsets.
        for round in 0..5_000 {
            let mut bytes = Vec::new();
            for _ in 0..(next() % 20) {
                for field in 0..4 {
                    if field > 0 {
                        bytes.push(b',');
                    }
                    let quoted = next() % 4 == 0;
                    if quoted {
                        bytes.push(b'"');
                    }
                    for _ in 0..(next() % 12) {
                        bytes.push(b"abcxyz019 \r"[(next() % 11) as usize]);
                    }
                    if next() % 50 == 0 {
                        bytes.push(b'"');
                    }
                    if quoted {
                        bytes.push(b'"');
                    }
                }
                if next() % 30 == 0 {
                    bytes.push(b',');
                }
                bytes.push(b'\n');
            }
            if next() % 2 == 0 {
                bytes.pop();
            }
            assert_eq!(fast::<4>(&bytes), simple::<4>(&bytes), "round {round}: {:?}", String::from_utf8_lossy(&bytes));
        }
    }
}
