//! Worker threads for the speech engine.
//!
//! A take runs about 1,200 short parallel steps. A thread pool that puts
//! idle threads to sleep pays about 20 µs to wake them for each step after a
//! short serial gap. These workers spin on a counter while steps keep coming,
//! so a step starts in about a microsecond, and they park after
//! `SPIN_FOR` without work, so an idle app uses no CPU.

use parking_lot::{Condvar, Mutex};
use std::cell::Cell;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Longer than the gaps inside a take, short enough to be invisible as idle CPU.
const SPIN_FOR: Duration = Duration::from_millis(3);

struct Job {
    run: *const (dyn Fn(usize) + Sync),
    tasks: usize,
}

struct Shared {
    generation: AtomicUsize,
    job: AtomicPtr<Job>,
    next: AtomicUsize,
    /// Workers that have not finished the current job yet.
    busy: AtomicUsize,
    parked: AtomicUsize,
    lock: Mutex<()>,
    wake: Condvar,
}

pub struct Team {
    shared: Arc<Shared>,
    workers: usize,
    /// One caller at a time; a nested call from inside a task runs inline.
    submit: Mutex<()>,
}

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

impl Team {
    /// `threads` includes the calling thread, which works on every job too.
    fn new(threads: usize) -> Self {
        let shared = Arc::new(Shared {
            generation: AtomicUsize::new(0),
            job: AtomicPtr::new(std::ptr::null_mut()),
            next: AtomicUsize::new(0),
            busy: AtomicUsize::new(0),
            parked: AtomicUsize::new(0),
            lock: Mutex::new(()),
            wake: Condvar::new(),
        });
        let workers = threads.saturating_sub(1);
        for index in 0..workers {
            let for_worker = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name(format!("tdt-speech-{index}"))
                .spawn(move || work(&for_worker));
            if spawned.is_err() {
                // Fewer threads only make the engine slower.
                return Self::with_workers(shared, index);
            }
        }
        Self::with_workers(shared, workers)
    }

    fn with_workers(shared: Arc<Shared>, workers: usize) -> Self {
        Self {
            shared,
            workers,
            submit: Mutex::new(()),
        }
    }

    fn for_each(&self, tasks: usize, run: &(dyn Fn(usize) + Sync)) {
        if tasks <= 1 || self.workers == 0 || INSIDE.get() {
            (0..tasks).for_each(run);
            return;
        }
        let _one_caller = self.submit.lock();
        INSIDE.set(true);
        // SAFETY: the job only lives until every worker has left it (busy == 0
        // below), so erasing the closure's lifetime cannot outlive the borrow.
        let run: &'static (dyn Fn(usize) + Sync) = unsafe { std::mem::transmute(run) };
        let job = Job { run, tasks };
        let s = &*self.shared;
        s.job
            .store(&job as *const Job as *mut Job, Ordering::Relaxed);
        s.next.store(0, Ordering::Relaxed);
        s.busy.store(self.workers, Ordering::Relaxed);
        s.generation.fetch_add(1, Ordering::SeqCst);
        if s.parked.load(Ordering::SeqCst) > 0 {
            let _lock = s.lock.lock();
            s.wake.notify_all();
        }
        run_tasks(s, &job);
        while s.busy.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        INSIDE.set(false);
    }
}

fn run_tasks(s: &Shared, job: &Job) {
    // SAFETY: the caller keeps the closure alive until every worker is done.
    let run = unsafe { &*job.run };
    loop {
        let task = s.next.fetch_add(1, Ordering::Relaxed);
        if task >= job.tasks {
            break;
        }
        run(task);
    }
}

fn work(s: &Shared) {
    INSIDE.set(true);
    // A fixed start, so a worker that starts late still sees the first job.
    let mut seen = 0;
    loop {
        let mut idle_since = Instant::now();
        let mut spins = 0u32;
        loop {
            let generation = s.generation.load(Ordering::Acquire);
            if generation != seen {
                seen = generation;
                break;
            }
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(256) && idle_since.elapsed() > SPIN_FOR {
                let mut lock = s.lock.lock();
                s.parked.fetch_add(1, Ordering::SeqCst);
                while s.generation.load(Ordering::SeqCst) == seen {
                    s.wake.wait(&mut lock);
                }
                s.parked.fetch_sub(1, Ordering::SeqCst);
                idle_since = Instant::now();
            } else {
                std::hint::spin_loop();
            }
        }
        // SAFETY: the job pointer was published before the generation changed.
        let job = unsafe { &*s.job.load(Ordering::Relaxed) };
        run_tasks(s, job);
        s.busy.fetch_sub(1, Ordering::Release);
    }
}

static TEAM: OnceLock<Team> = OnceLock::new();
static THREADS: AtomicUsize = AtomicUsize::new(0);

/// Sets the team size before the first parallel step. Later calls do nothing.
pub fn init(threads: usize) {
    THREADS.store(threads.max(1), Ordering::Relaxed);
    let _ = team();
}

fn team() -> &'static Team {
    TEAM.get_or_init(|| {
        let threads = match THREADS.load(Ordering::Relaxed) {
            0 => std::thread::available_parallelism().map_or(4, |n| n.get()),
            n => n,
        };
        Team::new(threads)
    })
}

/// Threads that work on each step, the caller included.
pub fn threads() -> usize {
    team().workers + 1
}

/// `run(task)` for every task in `0..tasks`, spread over the team.
pub fn for_each(tasks: usize, run: impl Fn(usize) + Sync) {
    team().for_each(tasks, &run);
}

/// A buffer that parallel tasks write in disjoint ranges.
pub struct SendPtr<T>(*mut T);
impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SendPtr<T> {}
// SAFETY: every task touches its own disjoint range.
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> SendPtr<T> {
    pub fn new(data: &mut [T]) -> Self {
        Self(data.as_mut_ptr())
    }

    /// # Safety
    /// The range must be inside the buffer, no other task may use it at the
    /// same time, and the buffer must outlive the returned slice.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice_mut<'a>(&self, start: usize, len: usize) -> &'a mut [T] {
        std::slice::from_raw_parts_mut(self.0.add(start), len)
    }
}

/// `run(index, chunk)` for consecutive chunks of `chunk` elements.
pub fn chunks_mut<T: Send>(data: &mut [T], chunk: usize, run: impl Fn(usize, &mut [T]) + Sync) {
    let (len, ptr) = (data.len(), SendPtr::new(data));
    let chunk = chunk.max(1);
    for_each(len.div_ceil(chunk), |index| {
        let start = index * chunk;
        let end = (start + chunk).min(len);
        // SAFETY: chunks are disjoint and inside `data`, which outlives this call.
        let slice = unsafe { ptr.slice_mut(start, end - start) };
        run(index, slice);
    });
}

/// `run(task)` for every task, results in task order.
pub fn map<T: Send>(tasks: usize, run: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let mut slots: Vec<Option<T>> = (0..tasks).map(|_| None).collect();
    chunks_mut(&mut slots, 1, |index, slot| slot[0] = Some(run(index)));
    slots
        .into_iter()
        .map(|slot| slot.expect("every task ran"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{chunks_mut, for_each, map};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn every_task_runs_exactly_once_across_many_steps() {
        for round in 0..2000 {
            let tasks = round % 37;
            let counts: Vec<AtomicUsize> = (0..tasks).map(|_| AtomicUsize::new(0)).collect();
            for_each(tasks, |t| {
                counts[t].fetch_add(1, Ordering::Relaxed);
            });
            assert!(
                counts.iter().all(|c| c.load(Ordering::Relaxed) == 1),
                "round {round}"
            );
        }
    }

    #[test]
    fn nested_steps_run_inline_and_results_keep_their_order() {
        let sums = map(16, |i| {
            let mut inner = vec![0usize; 10];
            chunks_mut(&mut inner, 3, |chunk, values| {
                values.iter_mut().for_each(|v| *v = chunk + i)
            });
            inner.iter().sum::<usize>()
        });
        for (i, sum) in sums.iter().enumerate() {
            // Chunks 0, 1, 2 hold three values each, chunk 3 holds one.
            assert_eq!(*sum, 12 + 10 * i);
        }
    }

    #[test]
    fn steps_after_the_workers_park_still_finish() {
        for_each(64, |_| {});
        std::thread::sleep(std::time::Duration::from_millis(20));
        let count = AtomicUsize::new(0);
        for_each(64, |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(count.load(Ordering::Relaxed), 64);
    }
}
