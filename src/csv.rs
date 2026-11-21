//! The CSV dialect (rules.md "CSV dialect") and newline-aligned input chunks.

use std::fs::File;
use std::path::{Path, PathBuf};

use memchr::{memchr, memchr_iter};
use memmap2::Mmap;

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

/// Every input file of one table, memory-mapped.
pub struct Files {
    maps: Vec<Mmap>,
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
        let mut maps = Vec::new();
        for path in paths {
            let file = File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
            let length = file.metadata().map(|m| m.len()).unwrap_or(0);
            if length == 0 {
                continue;
            }
            // SAFETY: the inputs are read-only and nothing modifies them while we run.
            let map = unsafe { Mmap::map(&file) }.unwrap_or_else(|error| panic!("mmap {}: {error}", path.display()));
            maps.push(map);
        }
        Files { maps }
    }

    pub fn bytes(&self, file: usize) -> &[u8] {
        &self.maps[file]
    }

    /// Splits the data lines (everything after each file's header line) into
    /// chunks of about `target` bytes that start and end on line boundaries.
    pub fn chunks(&self, target: usize) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        for (file, map) in self.maps.iter().enumerate() {
            let bytes: &[u8] = map;
            let Some(header_end) = memchr(b'\n', bytes).map(|at| at + 1) else {
                continue; // only a header line
            };
            let mut start = header_end;
            while start < bytes.len() {
                let mut end = (start + target).min(bytes.len());
                if end < bytes.len() {
                    end = match memchr(b'\n', &bytes[end..]) {
                        Some(at) => end + at + 1,
                        None => bytes.len(),
                    };
                }
                chunks.push(Chunk { file, start, end });
                start = end;
            }
        }
        chunks
    }
}

#[derive(Clone, Copy)]
pub struct Chunk {
    pub file: usize,
    pub start: usize,
    pub end: usize,
}

impl Chunk {
    pub fn bytes<'a>(&self, files: &'a Files) -> &'a [u8] {
        &files.bytes(self.file)[self.start..self.end]
    }
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
}
