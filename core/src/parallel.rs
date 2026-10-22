//! Independent items worked on the program's stated number of threads ([`Threads`]), with the serial result.
//!
//! A program states how many threads a run may use by putting [`Threads`] in the world (`core.threads`); passes read
//! it with [`Threads::of`] (absent: one, the serial run) and writers take it from their options. [`map`] and
//! [`rows_mut`] hand items to scoped threads by an atomic work index (or, for rows, a shared queue), each thread one
//! item at a time, so at most that many items' working buffers exist at once. Results come back in item order and each
//! item's work sees only that item, so the output is the bytes the serial run makes; a caller that tallies across
//! items collects per item and combines in index order. A panicking item becomes an [`Panicked`] error naming it: the
//! other threads take no new item and the call returns once they finish the one they hold. No global state: nothing
//! here outlives the call.

use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{Resource, World};

/// How many threads a run's parallel work may use: the program's statement (`core.threads`), one when it states none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Threads(pub NonZeroUsize);

impl Threads {
    /// One thread: the serial run.
    pub const ONE: Threads = Threads(NonZeroUsize::MIN);

    /// `n` threads; `None` for zero.
    pub fn new(n: usize) -> Option<Threads> {
        NonZeroUsize::new(n).map(Threads)
    }

    pub fn get(self) -> usize {
        self.0.get()
    }

    /// What the world `w` states, one when it states nothing.
    pub fn of(w: &World) -> Threads {
        w.resource::<Threads>().copied().unwrap_or(Threads::ONE)
    }
}

impl Resource for Threads {
    const NAME: &'static str = "core.threads";
}

/// An item whose work panicked: its index and the panic's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panicked {
    pub item: usize,
    pub message: String,
}

impl std::fmt::Display for Panicked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "item {} panicked: {}", self.item, self.message)
    }
}

impl std::error::Error for Panicked {}

/// `f(index, item)` for every item of `items`, on up to `threads` threads; the results in item order. One thread (or
/// one item) runs on the calling thread.
pub fn map<T: Sync, R: Send>(
    threads: Threads,
    items: &[T],
    f: impl Fn(usize, &T) -> R + Sync,
) -> Result<Vec<R>, Panicked> {
    let run = |i: usize| catch_unwind(AssertUnwindSafe(|| f(i, &items[i]))).map_err(|p| panicked(i, &*p));
    let workers = threads.get().min(items.len());
    if workers <= 1 {
        return (0..items.len()).map(run).collect();
    }
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let mut done: Vec<Vec<(usize, Result<R, Panicked>)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    while !failed.load(Ordering::Relaxed) {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= items.len() {
                            break;
                        }
                        let r = run(i);
                        if r.is_err() {
                            failed.store(true, Ordering::Relaxed);
                        }
                        mine.push((i, r));
                    }
                    mine
                })
            })
            .collect();
        // a worker's own panics are caught per item, so its join cannot fail
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut out: Vec<Option<R>> = std::iter::repeat_with(|| None).take(items.len()).collect();
    let mut first_error: Option<Panicked> = None;
    for (i, r) in done.drain(..).flatten() {
        match r {
            Ok(v) => out[i] = Some(v),
            Err(e) => {
                if first_error.as_ref().is_none_or(|f| e.item < f.item) {
                    first_error = Some(e);
                }
            }
        }
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(out
        .into_iter()
        .map(|v| v.expect("every item ran when none failed"))
        .collect())
}

/// `f(row, slice)` for every `row_len`-long row of `data` (the last may be shorter), on up to `threads` threads, each
/// row's slice written in place. One thread (or one row) runs on the calling thread.
pub fn rows_mut<T: Send>(
    threads: Threads,
    data: &mut [T],
    row_len: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) -> Result<(), Panicked> {
    assert!(row_len > 0, "rows of no length");
    let rows = data.len().div_ceil(row_len);
    let run =
        |i: usize, row: &mut [T]| catch_unwind(AssertUnwindSafe(|| f(i, row))).map_err(|p| panicked(i, &*p));
    let workers = threads.get().min(rows);
    if workers <= 1 {
        for (i, row) in data.chunks_mut(row_len).enumerate() {
            run(i, row)?;
        }
        return Ok(());
    }
    let queue = Mutex::new(data.chunks_mut(row_len).enumerate());
    let failed = AtomicBool::new(false);
    let errors: Vec<Panicked> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut errors = Vec::new();
                    while !failed.load(Ordering::Relaxed) {
                        // a poisoned queue only means another row panicked, which `failed` already says
                        let taken = queue.lock().map_or(None, |mut q| q.next());
                        let Some((i, row)) = taken else { break };
                        if let Err(e) = run(i, row) {
                            failed.store(true, Ordering::Relaxed);
                            errors.push(e);
                        }
                    }
                    errors
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    match errors.into_iter().min_by_key(|e| e.item) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn panicked(item: usize, payload: &(dyn std::any::Any + Send)) -> Panicked {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned());
    Panicked { item, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threads(n: usize) -> Threads {
        Threads::new(n).unwrap()
    }

    /// The results are in item order and equal the serial run's, whatever the thread count.
    #[test]
    fn map_keeps_the_item_order_and_the_serial_result() {
        let items: Vec<u64> = (0..1000).collect();
        let f = |i: usize, x: &u64| (i as u64) * 31 + x * x;
        let serial = map(Threads::ONE, &items, f).unwrap();
        for n in [2, 3, 8, 64] {
            assert_eq!(map(threads(n), &items, f).unwrap(), serial, "{n} threads");
        }
        assert_eq!(map(threads(8), &[] as &[u64], f).unwrap(), Vec::<u64>::new());
    }

    /// At most `threads` items are worked at once.
    #[test]
    fn map_works_at_most_threads_items_at_once() {
        let busy = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        map(threads(3), &[(); 200], |_, _| {
            let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_micros(200));
            busy.fetch_sub(1, Ordering::SeqCst);
        })
        .unwrap();
        assert!(most.load(Ordering::SeqCst) <= 3, "{most:?}");
    }

    /// A panicking item is an error naming it (the lowest that panicked), on one thread or many.
    #[test]
    fn a_panic_is_an_error_naming_the_item() {
        for n in [1, 4] {
            let e = map(threads(n), &(0..100).collect::<Vec<u32>>(), |_, &x| {
                assert!(x != 37, "item thirty-seven");
                x
            })
            .unwrap_err();
            assert_eq!(e.item, 37, "{n} threads");
            assert!(e.message.contains("thirty-seven"), "{e}");
            let mut data = vec![0u8; 100];
            let e = rows_mut(threads(n), &mut data, 10, |row, _| assert!(row != 4, "row four")).unwrap_err();
            assert_eq!((e.item, e.message.contains("row four")), (4, true), "{n} threads");
        }
    }

    /// Rows are written in place, the last one short, the same on any thread count.
    #[test]
    fn rows_mut_writes_every_row_in_place() {
        let fill = |n: usize| {
            let mut data = vec![0u32; 1003];
            rows_mut(threads(n), &mut data, 10, |row, slice| {
                for (k, v) in slice.iter_mut().enumerate() {
                    *v = (row * 1000 + k) as u32;
                }
            })
            .unwrap();
            data
        };
        let serial = fill(1);
        assert_eq!(serial[1002], 100_002);
        for n in [2, 7, 32] {
            assert_eq!(fill(n), serial, "{n} threads");
        }
    }

    /// A world states its thread count; one when it states none.
    #[test]
    fn a_world_states_its_threads() {
        let mut w = World::new();
        assert_eq!(Threads::of(&w), Threads::ONE);
        w.insert_resource(threads(6));
        assert_eq!(Threads::of(&w).get(), 6);
    }
}
