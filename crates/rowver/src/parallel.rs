//! Run one function over many items on a few threads, keeping the items' order.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// How many threads the machine offers.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// `work` of every item, in order, on up to `workers` threads; after a failure none starts, and the first in order is returned.
pub fn map<T: Sync, R: Send, E: Send + From<&'static str>>(
    items: &[T],
    workers: usize,
    work: impl Fn(&T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E> {
    if workers < 2 || items.len() < 2 {
        return items.iter().map(work).collect();
    }
    let (next, failed) = (AtomicUsize::new(0), AtomicBool::new(false));
    let done: Mutex<Vec<Option<Result<R, E>>>> = Mutex::new(items.iter().map(|_| None).collect());
    // Joined by hand: a scope left with a panicked thread would panic in turn.
    let crashed = std::thread::scope(|scope| {
        let run = || {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(at).filter(|_| !failed.load(Ordering::Acquire)) else {
                    break;
                };
                let result = work(item);
                if result.is_err() {
                    failed.store(true, Ordering::Release);
                }
                done.lock().unwrap_or_else(|error| error.into_inner())[at] = Some(result);
            })
        };
        let threads: Vec<_> = (0..workers.min(items.len())).map(|_| run()).collect();
        threads
            .into_iter()
            .fold(false, |crashed, thread| thread.join().is_err() || crashed)
    });
    if crashed {
        return Err("A worker stopped unexpectedly".into());
    }
    let done = done.into_inner().unwrap_or_else(|error| error.into_inner());
    let mut results = Vec::with_capacity(done.len());
    let mut undone = false;
    for result in done {
        match result {
            Some(Ok(result)) if !undone => results.push(result),
            Some(Err(error)) => return Err(error),
            _ => undone = true,
        }
    }
    if undone {
        return Err("An item was left undone".into());
    }
    Ok(results)
}
