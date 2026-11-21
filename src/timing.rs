//! Cheap run-time accounting for the per-phase breakdown printed on stderr.
//! Counters are summed over all threads (CPU-seconds of work, not wall time)
//! and are only touched once per input chunk or per partition, so they cost
//! nothing measurable and stay on in every build.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct Counter {
    name: &'static str,
    nanos: AtomicU64,
}

impl Counter {
    const fn new(name: &'static str) -> Counter {
        Counter { name, nanos: AtomicU64::new(0) }
    }

    #[inline(always)]
    pub fn add_since(&self, since: Instant) {
        self.nanos.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn add_nanos(&self, nanos: u64) {
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
    }
}

/// Positional reads of the input chunks (all tables).
pub static READ: Counter = Counter::new("read");
/// Sessionize: tombstones, grouping and sorting of each partition's events.
pub static GROUP_EVENTS: Counter = Counter::new("group-events");
/// Sessionize: grouping, sorting and dedup of each partition's clicks.
pub static GROUP_CLICKS: Counter = Counter::new("group-clicks");
/// Sessionize: sessions and user_daily rows (attribution excluded).
pub static SESSIONS: Counter = Counter::new("sessions");
/// Sessionize: credit and assists (Clicks 4-7).
pub static ATTRIBUTION: Counter = Counter::new("attribution");

/// A cycle counter for spans too short for `Instant` (one person's
/// attribution); converted to time by the share of a measured span.
#[inline(always)]
pub fn ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: rdtsc has no preconditions on x86_64.
    unsafe {
        std::arch::x86_64::_rdtsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

/// Prints every counter as `cpu NAME SECONDS` on stderr.
pub fn report() {
    for counter in [&READ, &GROUP_EVENTS, &GROUP_CLICKS, &SESSIONS, &ATTRIBUTION] {
        eprintln!("cpu {} {:.3}", counter.name, counter.nanos.load(Ordering::Relaxed) as f64 / 1e9);
    }
}
