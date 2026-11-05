//! Run one function over many items on a few threads, keeping the items' order.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

/// How many threads the machine offers.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

std::thread_local! {
    /// Whether this thread is one of `map`'s workers, whose own `map`s run in it: threads never multiply.
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `work` of every item, in order, on up to `workers` threads (in this one, inside another `map`);
/// after a failure none starts, and the first in order is returned.
pub fn map<T: Sync, R: Send, E: Send + From<&'static str>>(
    items: &[T],
    workers: usize,
    work: impl Fn(&T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E> {
    if workers < 2 || items.len() < 2 || WORKER.get() {
        return items.iter().map(work).collect();
    }
    let (next, failed) = (AtomicUsize::new(0), AtomicBool::new(false));
    let done: Mutex<Vec<Option<Result<R, E>>>> = Mutex::new(items.iter().map(|_| None).collect());
    // Joined by hand: a scope left with a panicked thread would panic in turn.
    let crashed = std::thread::scope(|scope| {
        let run = || {
            scope.spawn(|| loop {
                WORKER.set(true);
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

/// What `ordered`'s threads share: the next item to start, how many are handed on, the results
/// not yet handed on, whether to start no more, and whether a worker stopped unexpectedly.
struct Queue<R, E> {
    next: usize,
    taken: usize,
    done: BTreeMap<usize, Result<R, E>>,
    stop: bool,
    lost: bool,
}

/// `ordered`'s queue, and its signals: a result is in, and there is room for more.
struct Shared<R, E> {
    queue: Mutex<Queue<R, E>>,
    ready: Condvar,
    room: Condvar,
}

impl<R, E> Shared<R, E> {
    fn lock(&self) -> MutexGuard<'_, Queue<R, E>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Dropped as a thread of `ordered` ends: a worker that panics says so, and the one handing on
/// results lets the workers go, whatever ended it.
struct Ending<'a, R, E>(&'a Shared<R, E>, bool);

impl<R, E> Drop for Ending<'_, R, E> {
    fn drop(&mut self) {
        let mut queue = self.0.lock();
        match self.1 {
            true => queue.lost |= std::thread::panicking(),
            false => queue.stop = true,
        }
        drop(queue);
        self.0.ready.notify_all();
        self.0.room.notify_all();
    }
}

/// `work` of items `0..items` on up to `workers` threads (in this one, inside a `map`), each
/// result handed to `each` here, in order, with no more than twice `workers` held at once; after a
/// failure none starts, and the first in order is returned.
pub fn ordered<R: Send, E: Send + From<&'static str>>(
    items: usize,
    workers: usize,
    work: impl Fn(usize) -> Result<R, E> + Sync,
    mut each: impl FnMut(R) -> Result<(), E>,
) -> Result<(), E> {
    if workers < 2 || items < 2 || WORKER.get() {
        return (0..items).try_for_each(|item| each(work(item)?));
    }
    let ahead = 2 * workers;
    let shared = Shared { queue: Mutex::new(Queue { next: 0, taken: 0, done: BTreeMap::new(), stop: false, lost: false }), ready: Condvar::new(), room: Condvar::new() };
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..workers.min(items))
            .map(|_| {
                scope.spawn(|| {
                    WORKER.set(true);
                    let _ending = Ending(&shared, true);
                    loop {
                        let mut queue = shared.lock();
                        while !queue.stop && queue.next < items && queue.next >= queue.taken + ahead {
                            queue = shared.room.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        if queue.stop || queue.next >= items {
                            break;
                        }
                        let at = queue.next;
                        queue.next += 1;
                        drop(queue);
                        let result = work(at);
                        let mut queue = shared.lock();
                        queue.stop |= result.is_err();
                        queue.done.insert(at, result);
                        drop(queue);
                        shared.ready.notify_all();
                    }
                })
            })
            .collect();
        let handed = (|| {
            let _ending = Ending(&shared, false);
            for at in 0..items {
                let mut queue = shared.lock();
                let result = loop {
                    if let Some(result) = queue.done.remove(&at) {
                        break result;
                    }
                    if queue.lost {
                        return Err("A worker stopped unexpectedly".into());
                    }
                    queue = shared.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                };
                queue.taken = at + 1;
                drop(queue);
                shared.room.notify_all();
                each(result?)?;
            }
            Ok(())
        })();
        // Joined by hand: a scope left with a panicked thread would panic in turn.
        let crashed = threads.into_iter().fold(false, |crashed, thread| thread.join().is_err() || crashed);
        match crashed {
            true => Err("A worker stopped unexpectedly".into()),
            false => handed,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // FORMAT 7; SPEC 7.2.2
    #[test]
    fn ordered_hands_every_result_on_in_order_and_stops_at_the_first_failure() {
        let items: Vec<usize> = (0..500).collect();
        let mut seen = Vec::new();
        let slow = |n: usize| -> Result<usize, String> {
            std::thread::sleep(std::time::Duration::from_micros((n * 7919 % 13) as u64 * 50));
            Ok(n * 2)
        };
        ordered(500, 4, slow, |n| {
            seen.push(n);
            Ok::<_, String>(())
        })
        .unwrap();
        assert_eq!(seen, items.iter().map(|n| n * 2).collect::<Vec<_>>());
        let (mut seen, failing) = (Vec::new(), |n: usize| if n == 300 { Err(format!("item {n}")) } else { Ok(n) });
        let handed = ordered(500, 4, failing, |n| {
            seen.push(n);
            Ok(())
        });
        assert_eq!(handed, Err("item 300".into()));
        assert_eq!(seen, (0..300).collect::<Vec<_>>());
        let mut handed = 0;
        assert_eq!(ordered(500, 4, Ok::<_, String>, |_| { handed += 1; if handed == 10 { Err("enough".into()) } else { Ok(()) } }), Err("enough".into()));
        let panicked = std::panic::catch_unwind(|| ordered(500, 4, |n| if n == 50 { panic!("a bug") } else { Ok::<_, String>(n) }, |_| Ok(())));
        assert_eq!(panicked.ok(), Some(Err("A worker stopped unexpectedly".into())));
    }
}
