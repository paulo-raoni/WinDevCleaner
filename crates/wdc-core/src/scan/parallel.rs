//! The parallel walk (`--jobs`, plan 6.3): up to four workers list directories from one shared
//! stack, and the results are merged into the collector in exactly the order one thread would
//! have produced them. So the report is the same as with one job:
//!
//! - A worker never touches the collector. For each directory it fills a [`DirRec`]: the
//!   sub-folders in listing order, the totals of the small files, the links, errors and excluded
//!   folders, and every file of at least [`DEDUP_THRESHOLD`] with its file ID, size and path.
//! - Which name of a hard-linked file counts is decided only at the merge, by walking the tree of
//!   records in the one-thread order (depth first, children in the order the one-thread walk
//!   adds them), with the collector's single seen-set. A worker never decides it, so it does not
//!   matter which worker met a name first.
//! - Folder indexes are assigned at the merge too, so a child still comes after its parent in
//!   `dirs` and the subtree pass of `scan` works unchanged.
//! - The top files use a total order (size on disk, then path), so each worker keeps its own
//!   list and the lists merge into the same result whatever the order.
//!
//! Every worker enters background mode for its own thread. Throttling pauses each worker before
//! each directory it lists. Progress is reported from the calling thread, from counters the
//! workers share.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{
    Collector, DirNode, LinkEntry, LinkKind, Progress, ScanError, Sink, TopFile, TopN, DEDUP_THRESHOLD, MAX_ERRORS,
    MAX_LINKS, NO_PARENT,
};

/// What a platform walker offers the parallel engine.
pub(crate) trait DirWalker: Sync {
    /// A folder to list, as the platform names it (carries what its children need, such as the
    /// volume serial).
    type Path: Send;

    /// A walker with its own buffers, for another worker.
    fn fork(&self) -> Self;

    /// The folder as the person sees it, for progress.
    fn display(path: &Self::Path) -> String;

    /// Lists one folder into `sink`; returns its sub-folders (name, path) in listing order.
    fn list<S: Sink>(&mut self, path: &Self::Path, sink: &mut S) -> Vec<(String, Self::Path)>;
}

/// A file of at least [`DEDUP_THRESHOLD`], kept until the merge decides whether it counts.
struct FileRec {
    key: (u64, u128),
    on_disk: u64,
    logical: u64,
    links: u32,
    path: String,
}

#[derive(Default)]
struct DirExtra {
    links: Vec<LinkEntry>,
    errors: Vec<ScanError>,
    excluded: Vec<String>,
    large: Vec<FileRec>,
}

/// Everything one directory holds, recorded by a worker.
#[derive(Default)]
struct DirRec {
    /// Sub-folders in listing order, with the id of the record each one will have.
    children: Vec<(String, usize)>,
    own_bytes: u64,
    own_files: u32,
    logical: u64,
    placeholders: u64,
    link_count: u64,
    error_count: u64,
    extra: Option<Box<DirExtra>>,
}

impl DirRec {
    fn extra(&mut self) -> &mut DirExtra {
        self.extra.get_or_insert_with(Default::default)
    }
}

/// The sink of one worker: a directory's record plus the worker's own top files.
struct ParSink<'r> {
    rec: &'r mut DirRec,
    top: &'r mut TopN,
}

impl Sink for ParSink<'_> {
    fn link(&mut self, path: String, kind: LinkKind, directory: bool) {
        self.rec.link_count += 1;
        let extra = self.rec.extra();
        if extra.links.len() < MAX_LINKS {
            extra.links.push(LinkEntry { path, kind, directory });
        }
    }

    fn error(&mut self, path: String, code: u32) {
        self.rec.error_count += 1;
        let extra = self.rec.extra();
        if extra.errors.len() < MAX_ERRORS {
            extra.errors.push(ScanError { path, code, kind: super::error_kind(code) });
        }
    }

    fn excluded(&mut self, path: String) {
        self.rec.extra().excluded.push(path);
    }

    fn placeholder(&mut self) {
        self.rec.placeholders += 1;
    }

    fn file(&mut self, key: (u64, u128), on_disk: u64, logical: u64, links: u32, path: impl FnOnce() -> String) {
        if on_disk >= DEDUP_THRESHOLD {
            // Whether this name counts is decided at the merge.
            let path = path();
            self.rec.extra().large.push(FileRec { key, on_disk, logical, links, path });
        } else {
            self.rec.own_bytes += on_disk;
            self.rec.own_files += 1;
            self.rec.logical += logical;
            if self.top.wants(on_disk) {
                self.top.add(TopFile { path: path(), bytes_on_disk: on_disk, logical_bytes: logical, links });
            }
        }
    }
}

struct Queue<P> {
    /// Folders to list, with the id of their record.
    items: Vec<(usize, P)>,
    /// Workers listing a folder right now.
    active: usize,
    stop: bool,
}

/// What the workers count as they go, for progress.
#[derive(Default)]
struct Live {
    dirs: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    current: Mutex<String>,
    current_ms: AtomicU64,
}

/// Marks a worker finished when dropped. A worker that panics never gives back the folder it was
/// listing, so the others would wait for it forever: when dropped while panicking it stops the
/// walk, and the panic reaches the caller when the workers are joined.
struct Done<'a, P> {
    count: &'a Mutex<usize>,
    cv: &'a Condvar,
    queue: &'a Mutex<Queue<P>>,
    wake: &'a Condvar,
}

impl<P> Drop for Done<'_, P> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.queue.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
            self.wake.notify_all();
        }
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.cv.notify_all();
    }
}

struct WorkerOut {
    recs: Vec<(usize, DirRec)>,
    top: TopN,
}

/// Lists the tree under `root_idx` with `c.jobs` workers and merges it into `c`. The root's node
/// already exists in `c`.
pub(crate) fn walk_tree<W: DirWalker>(c: &mut Collector, root_idx: u32, root: W::Path, walker: &W) {
    let jobs = c.jobs;
    let cancel = c.cancel;
    let throttle = c.throttle;
    let top_n = c.top.n;
    let want_progress = c.wants_progress();
    let base = c.totals;

    let queue = Mutex::new(Queue { items: vec![(0usize, root)], active: 0, stop: false });
    let wake = Condvar::new();
    let next_id = std::sync::atomic::AtomicUsize::new(1);
    let live = Live::default();
    let finished = Mutex::new(0usize);
    let finished_cv = Condvar::new();
    let started = Instant::now();

    let outs: Vec<WorkerOut> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..jobs {
            handles.push(scope.spawn(|| {
                // Background mode is per thread: each worker lowers its own priorities.
                let _background = crate::background::ThreadBackground::enter();
                // Counted as finished even if the worker panics, so the caller never waits for it.
                let _done = Done { count: &finished, cv: &finished_cv, queue: &queue, wake: &wake };
                worker(walker, &queue, &wake, &next_id, &live, cancel, throttle, top_n, want_progress, started)
            }));
        }
        // The calling thread reports progress until every worker is done.
        loop {
            let guard = finished.lock().unwrap();
            if *guard >= jobs {
                break;
            }
            let _ = finished_cv.wait_timeout(guard, Duration::from_millis(100)).unwrap();
            if want_progress {
                let current = live.current.lock().unwrap().clone();
                c.report_progress(Progress {
                    dirs: base.dirs + live.dirs.load(Ordering::Relaxed),
                    files: base.files + live.files.load(Ordering::Relaxed),
                    bytes_on_disk: base.bytes_on_disk + live.bytes.load(Ordering::Relaxed),
                    current,
                });
            }
        }
        handles.into_iter().map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });

    let total_ids = next_id.load(Ordering::Relaxed);
    let mut recs: Vec<Option<DirRec>> = Vec::new();
    recs.resize_with(total_ids, || None);
    for out in outs {
        for (id, rec) in out.recs {
            recs[id] = Some(rec);
        }
        for f in out.top.into_vec() {
            c.add_top(f);
        }
    }
    merge(c, root_idx, recs);
}

#[allow(clippy::too_many_arguments)]
fn worker<W: DirWalker>(
    walker: &W,
    queue: &Mutex<Queue<W::Path>>,
    wake: &Condvar,
    next_id: &std::sync::atomic::AtomicUsize,
    live: &Live,
    cancel: Option<&AtomicBool>,
    throttle: Option<Duration>,
    top_n: usize,
    want_progress: bool,
    started: Instant,
) -> WorkerOut {
    let mut walker = walker.fork();
    let mut out = WorkerOut { recs: Vec::new(), top: TopN::new(top_n) };
    loop {
        // Take a folder, or stop when the walk is over or cancelled.
        let (id, path) = {
            let mut q = queue.lock().unwrap();
            loop {
                if cancel.is_some_and(|f| f.load(Ordering::Relaxed)) {
                    q.stop = true;
                }
                if q.stop {
                    wake.notify_all();
                    return out;
                }
                if let Some(item) = q.items.pop() {
                    q.active += 1;
                    break item;
                }
                if q.active == 0 {
                    q.stop = true;
                    wake.notify_all();
                    return out;
                }
                q = wake.wait(q).unwrap();
            }
        };

        if let Some(t) = throttle {
            std::thread::sleep(t);
        }
        let mut rec = DirRec::default();
        let subdirs = {
            let mut sink = ParSink { rec: &mut rec, top: &mut out.top };
            walker.list(&path, &mut sink)
        };
        let mut found = Vec::with_capacity(subdirs.len());
        if !subdirs.is_empty() {
            let first = next_id.fetch_add(subdirs.len(), Ordering::Relaxed);
            for (i, (name, p)) in subdirs.into_iter().enumerate() {
                rec.children.push((name, first + i));
                found.push((first + i, p));
            }
        }

        live.dirs.fetch_add(found.len() as u64, Ordering::Relaxed);
        let large = rec.extra.as_ref().map(|e| e.large.as_slice()).unwrap_or(&[]);
        live.files.fetch_add(rec.own_files as u64 + large.len() as u64, Ordering::Relaxed);
        live.bytes.fetch_add(rec.own_bytes + large.iter().map(|f| f.on_disk).sum::<u64>(), Ordering::Relaxed);
        if want_progress {
            // Now and then, name the folder just listed; never wait for another worker.
            let now = started.elapsed().as_millis() as u64;
            if now.saturating_sub(live.current_ms.load(Ordering::Relaxed)) >= 100 {
                if let Ok(mut cur) = live.current.try_lock() {
                    *cur = W::display(&path);
                    live.current_ms.store(now, Ordering::Relaxed);
                }
            }
        }
        out.recs.push((id, rec));

        let mut q = queue.lock().unwrap();
        q.active -= 1;
        // Pushed in reverse, so the first-listed sub-folder is taken first.
        q.items.extend(found.into_iter().rev());
        drop(q);
        wake.notify_all();
    }
}

/// Applies the records to the collector in the order one thread would have: a folder, then its
/// sub-folders added in reverse listing order and visited from the last added.
fn merge(c: &mut Collector, root_idx: u32, mut recs: Vec<Option<DirRec>>) {
    let mut stack: Vec<(u32, usize)> = vec![(root_idx, 0)];
    while let Some((dir, id)) = stack.pop() {
        // A folder nobody listed (the walk was cancelled) stays empty, as with one job.
        let Some(rec) = recs[id].take() else { continue };
        {
            let d: &mut DirNode = &mut c.dirs[dir as usize];
            d.own_bytes += rec.own_bytes;
            d.own_files += rec.own_files;
        }
        c.totals.files += rec.own_files as u64;
        c.totals.bytes_on_disk += rec.own_bytes;
        c.totals.logical_bytes += rec.logical;
        c.totals.placeholders += rec.placeholders;
        c.totals.links += rec.link_count;
        c.totals.errors += rec.error_count;
        if let Some(extra) = rec.extra {
            let extra = *extra;
            for l in extra.links {
                if c.links.len() < MAX_LINKS {
                    c.links.push(l);
                }
            }
            for e in extra.errors {
                if c.errors.len() < MAX_ERRORS {
                    c.errors.push(e);
                }
            }
            for p in extra.excluded {
                c.add_excluded(p);
            }
            for f in extra.large {
                if c.collect_large {
                    c.large.push(TopFile {
                        path: f.path.clone(),
                        bytes_on_disk: f.on_disk,
                        logical_bytes: f.logical,
                        links: f.links,
                    });
                }
                let counted = c.count_once(f.key, f.on_disk);
                c.add_file(dir, counted, f.logical);
                if c.wants_top(counted) {
                    c.add_top(TopFile { path: f.path, bytes_on_disk: counted, logical_bytes: f.logical, links: f.links });
                }
            }
        }
        for (name, child_id) in rec.children.into_iter().rev() {
            let idx = c.add_dir(name, dir);
            stack.push((idx, child_id));
        }
    }
    debug_assert!(c.dirs.iter().all(|d| d.parent == NO_PARENT || (d.parent as usize) < c.dirs.len()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ScanOptions;

    /// A walker over numbered folders: the root (0) has three sub-folders, 1 to 3, and they have
    /// none. Listing `panic_on` panics, as a worker would on a bug in a platform walker.
    struct Fake {
        panic_on: usize,
    }

    impl DirWalker for Fake {
        type Path = usize;
        fn fork(&self) -> Self {
            Fake { panic_on: self.panic_on }
        }
        fn display(path: &usize) -> String {
            path.to_string()
        }
        fn list<S: Sink>(&mut self, path: &usize, _sink: &mut S) -> Vec<(String, usize)> {
            if *path == self.panic_on {
                panic!("test: worker panics listing folder {path}");
            }
            if *path == 0 {
                return (1..=3).map(|i| (format!("d{i}"), i)).collect();
            }
            Vec::new()
        }
    }

    fn run(panic_on: usize) -> std::thread::Result<()> {
        std::panic::catch_unwind(|| {
            let options = ScanOptions { jobs: 3, ..Default::default() };
            let mut c = Collector::new(&options, None, None);
            let root = c.add_dir("root".into(), NO_PARENT);
            walk_tree(&mut c, root, 0usize, &Fake { panic_on });
        })
    }

    #[test]
    fn a_walk_without_a_panic_finishes() {
        assert!(run(usize::MAX).is_ok());
    }

    #[test]
    fn a_panicking_worker_does_not_hang_the_others() {
        let (tx, rx) = std::sync::mpsc::channel();
        // Detached on purpose: if the walk hangs, the test fails by the timeout below.
        std::thread::spawn(move || {
            let _ = tx.send(run(2));
        });
        let outcome = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the walk hung after a worker panicked");
        assert!(outcome.is_err(), "the panic must reach the caller");
    }
}
