//! A tiny work-sharing loop over scoped threads.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs `work(state, item)` for every item in `0..items` on up to `threads`
/// threads, handing items out in order. Each thread owns a state made by
/// `init`; all states are returned.
pub fn run<S, I, W>(threads: usize, items: usize, init: I, work: W) -> Vec<S>
where
    S: Send,
    I: Fn() -> S + Sync,
    W: Fn(&mut S, usize) + Sync,
{
    let next = AtomicUsize::new(0);
    let threads = threads.clamp(1, items.max(1));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = init();
                    loop {
                        let item = next.fetch_add(1, Ordering::Relaxed);
                        if item >= items {
                            break;
                        }
                        work(&mut state, item);
                    }
                    state
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap()).collect()
    })
}
