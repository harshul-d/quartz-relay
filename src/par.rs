//! A tiny work-sharing loop over scoped threads.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs `work(state, item)` for every item in `0..items` on up to `threads`
/// threads, handing items out in order. Thread `t` owns the state made by
/// `init(t)`; all states are returned, in thread order.
pub fn run<S, I, W>(threads: usize, items: usize, init: I, work: W) -> Vec<S>
where
    S: Send,
    I: Fn(usize) -> S + Sync,
    W: Fn(&mut S, usize) + Sync,
{
    let next = AtomicUsize::new(0);
    let threads = threads.clamp(1, items.max(1));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|thread| {
                let (next, init, work) = (&next, &init, &work);
                scope.spawn(move || {
                    let mut state = init(thread);
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
