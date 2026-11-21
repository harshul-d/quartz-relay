//! Partitioned appends of 32-byte records with write-combining.
//!
//! Parsing appends every event to one of 256 partitions (twice: the person
//! record and the dedup copy). With plain pushes each new cache line of a
//! partition is first read from memory (read for ownership) only to be
//! overwritten. Here the first record of each pair waits in a small staging
//! slot; with the second, the full 64-byte pair is written with
//! non-temporal stores, which skip that read. The partitions are read again
//! only in a later phase, so bypassing the cache costs nothing.

use std::mem::MaybeUninit;

/// One staged record per partition, on its own cache line.
#[repr(C, align(64))]
struct Slot<T: Copy> {
    first: MaybeUninit<T>,
}

pub struct Scatter<T: Copy> {
    parts: Vec<Vec<T>>,
    slots: Vec<Slot<T>>,
    staged: Vec<bool>,
    streaming: bool,
}

impl<T: Copy> Scatter<T> {
    const SIZE_OK: () = assert!(std::mem::size_of::<T>() == 32 && std::mem::align_of::<T>() >= 16);

    /// `partitions` buffers of `capacity` records each. `streaming: false`
    /// (A/B only) uses plain pushes.
    pub fn new(partitions: usize, capacity: usize, streaming: bool) -> Scatter<T> {
        let () = Self::SIZE_OK;
        Scatter {
            parts: (0..partitions).map(|_| Vec::with_capacity(capacity)).collect(),
            slots: (0..partitions).map(|_| Slot { first: MaybeUninit::uninit() }).collect(),
            staged: vec![false; partitions],
            streaming: streaming && cfg!(target_arch = "x86_64"),
        }
    }

    /// Records appended to `part` so far (staged ones included): the index
    /// the next one will have.
    #[inline(always)]
    pub fn len(&self, part: usize) -> usize {
        self.parts[part].len() + self.staged[part] as usize
    }

    #[inline(always)]
    pub fn push(&mut self, part: usize, value: T) {
        if !self.streaming {
            self.parts[part].push(value);
            return;
        }
        if !self.staged[part] {
            self.slots[part].first = MaybeUninit::new(value);
            self.staged[part] = true;
            return;
        }
        self.staged[part] = false;
        let buffer = &mut self.parts[part];
        buffer.reserve(2);
        // SAFETY: `reserve` made room for two more records; the slot holds a
        // record written by the previous push; `T` is 32 bytes and at least
        // 16-byte aligned, so every 16-byte store below is aligned.
        unsafe {
            let at = buffer.as_mut_ptr().add(buffer.len());
            stream_pair(at, self.slots[part].first.as_ptr(), &value);
            buffer.set_len(buffer.len() + 2);
        }
    }

    /// Appends the staged records and returns the partitions. The pushing
    /// thread must have called `fence` after its last push (any thread may
    /// call this afterwards).
    pub fn finish(mut self) -> Vec<Vec<T>> {
        self.fence();
        for (part, staged) in self.staged.iter().enumerate() {
            if *staged {
                // SAFETY: a staged slot holds a record.
                self.parts[part].push(unsafe { self.slots[part].first.assume_init() });
            }
        }
        self.parts
    }

    /// Makes the streaming stores so far visible to other threads.
    #[inline]
    pub fn fence(&self) {
        #[cfg(target_arch = "x86_64")]
        if self.streaming {
            // SAFETY: sfence has no preconditions.
            unsafe { std::arch::x86_64::_mm_sfence() };
        }
    }
}

/// Writes `*first` then `*second` to `at` (two 32-byte records) with
/// non-temporal 16-byte stores.
///
/// # Safety
/// `at` must be valid for 64 bytes of writes and 16-byte aligned; `first`
/// and `second` must point to 32 readable bytes each.
#[inline(always)]
unsafe fn stream_pair<T>(at: *mut T, first: *const T, second: *const T) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::*;
        let to = at as *mut __m128i;
        let a = first as *const __m128i;
        let b = second as *const __m128i;
        _mm_stream_si128(to, _mm_loadu_si128(a));
        _mm_stream_si128(to.add(1), _mm_loadu_si128(a.add(1)));
        _mm_stream_si128(to.add(2), _mm_loadu_si128(b));
        _mm_stream_si128(to.add(3), _mm_loadu_si128(b.add(1)));
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        std::ptr::copy_nonoverlapping(first, at, 1);
        std::ptr::copy_nonoverlapping(second, at.add(1), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    #[repr(C, align(16))]
    struct Record([u64; 4]);

    /// Same partitions and order as plain pushes, streaming or not.
    #[test]
    fn matches_plain_pushes() {
        for streaming in [true, false] {
            let mut scatter = Scatter::<Record>::new(7, 3, streaming);
            let mut plain: Vec<Vec<Record>> = vec![Vec::new(); 7];
            let mut state = 0x1234_5678u64;
            for i in 0..10_000u64 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let part = (state % 7) as usize;
                assert_eq!(scatter.len(part), plain[part].len());
                let record = Record([i, state, !i, i * 3]);
                scatter.push(part, record);
                plain[part].push(record);
            }
            assert_eq!(scatter.finish(), plain);
        }
    }
}
