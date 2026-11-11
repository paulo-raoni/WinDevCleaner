//! Measurement walker for plan section 6.3: does walking a cold tree with 4 workers beat one
//! worker by more than 2x? It enumerates a folder tree with the same Windows calls as the product
//! scanner (`wdc-core`, `scan/windows.rs`): each directory is opened with `CreateFileW`
//! (`FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`, so no link is ever followed),
//! listed with `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` into a 64 KiB buffer
//! (falling back to `FileIdBothDirectoryInfo` where the extended class is missing), and `.` and
//! `..` are skipped. Each worker sums the files, directories and allocation sizes the listing
//! reports; the totals are merged at the end. The code is copied from the scanner on purpose: the
//! bench must not depend on private items of `wdc-core`.
//!
//! What the number does not cover: the runner's disk is a cloud VM disk behind a VHDX, not the
//! owner's NVMe SSD, so the scaling it shows (queue depth, latency, caching in the host) can be
//! far from the owner's machine; and this walker does not do the product's per-file work (hard-link
//! deduplication, large-file re-checks and records, top-file bookkeeping, the collector's
//! per-directory tree), which is CPU and memory work that parallel workers would also share. The
//! ratio is therefore evidence for or against the slice, not a promise about the product.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_NO_MORE_FILES,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileIdBothDirectoryInfo, FileIdExtdDirectoryInfo, GetFileInformationByHandleEx,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    SYNCHRONIZE,
};

/// What one walk found. Two walks of the same tree must produce equal `Totals` for `files`, `dirs`
/// and `bytes`; `errors` (directories that could not be opened or listed, such as the fixture's
/// unreadable one) is reported but not compared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
}

impl Totals {
    fn merge(&mut self, o: &Totals) {
        self.files += o.files;
        self.dirs += o.dirs;
        self.bytes += o.bytes;
        self.errors += o.errors;
    }

    /// The fields that must be equal between two walks.
    pub fn same_counts(&self, o: &Totals) -> bool {
        self.files == o.files && self.dirs == o.dirs && self.bytes == o.bytes
    }
}

pub struct WalkResult {
    pub totals: Totals,
    pub elapsed: Duration,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful `CreateFileW` and is closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Opens a directory for listing, never following a link (as the scanner's `open_dir`).
fn open_dir(path: &[u16]) -> Result<Handle, u32> {
    let mut z = path.to_vec();
    z.push(0);
    // SAFETY: `z` is a NUL-terminated UTF-16 string that outlives the call.
    let h = unsafe {
        CreateFileW(
            z.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        // SAFETY: reads the calling thread's last error.
        Err(unsafe { GetLastError() })
    } else {
        Ok(Handle(h))
    }
}

/// A reparse point that is a link to another name (junction, symbolic link, mount point).
fn is_name_surrogate(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

fn is_dot(name: &[u16]) -> bool {
    name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InfoClass {
    Extended,
    Both,
}

/// Lists a directory handle, calling `f(name, attributes, allocation, reparse_tag)` for every
/// entry except `.` and `..`.
fn list_dir(
    h: &Handle,
    class: &mut InfoClass,
    buf: &mut [u64],
    mut f: impl FnMut(&[u16], u32, u64, u32),
) -> Result<(), u32> {
    let bytes = std::mem::size_of_val(buf) as u32;
    loop {
        let info_class = match class {
            InfoClass::Extended => FileIdExtdDirectoryInfo,
            InfoClass::Both => FileIdBothDirectoryInfo,
        };
        // SAFETY: `buf` is valid for `bytes` bytes and 8-byte aligned.
        let ok = unsafe { GetFileInformationByHandleEx(h.0, info_class, buf.as_mut_ptr().cast(), bytes) };
        if ok == 0 {
            // SAFETY: reads the calling thread's last error.
            let e = unsafe { GetLastError() };
            if e == ERROR_NO_MORE_FILES {
                return Ok(());
            }
            if *class == InfoClass::Extended && (e == ERROR_INVALID_PARAMETER || e == ERROR_NOT_SUPPORTED) {
                *class = InfoClass::Both;
                continue;
            }
            return Err(e);
        }
        let base = buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            // SAFETY: Windows filled `buf` with a chain of 8-byte-aligned records; each record's
            // name length is bounded by the buffer.
            let next = unsafe {
                match class {
                    InfoClass::Extended => {
                        let r = &*(base.add(offset) as *const FILE_ID_EXTD_DIR_INFO);
                        let name = std::slice::from_raw_parts(r.FileName.as_ptr(), r.FileNameLength as usize / 2);
                        if !is_dot(name) {
                            f(name, r.FileAttributes, r.AllocationSize as u64, r.ReparsePointTag);
                        }
                        r.NextEntryOffset
                    }
                    InfoClass::Both => {
                        let r = &*(base.add(offset) as *const FILE_ID_BOTH_DIR_INFO);
                        let name = std::slice::from_raw_parts(r.FileName.as_ptr(), r.FileNameLength as usize / 2);
                        let reparse = r.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                        if !is_dot(name) {
                            // For reparse points this class reports the tag in EaSize.
                            f(name, r.FileAttributes, r.AllocationSize as u64, if reparse { r.EaSize } else { 0 });
                        }
                        r.NextEntryOffset
                    }
                }
            };
            if next == 0 {
                break;
            }
            offset += next as usize;
        }
    }
}

/// Per-worker state: its own buffer, info class and totals.
struct Worker {
    buf: Vec<u64>,
    class: InfoClass,
    totals: Totals,
}

impl Worker {
    fn new() -> Worker {
        Worker { buf: vec![0u64; 64 * 1024 / 8], class: InfoClass::Extended, totals: Totals::default() }
    }

    /// Lists one directory: counts its files and sub-directories, and returns the paths of the
    /// sub-directories to walk (links are counted nowhere and never entered).
    fn process(&mut self, path: &[u16]) -> Vec<Vec<u16>> {
        let h = match open_dir(path) {
            Ok(h) => h,
            Err(_) => {
                self.totals.errors += 1;
                return Vec::new();
            }
        };
        let mut subdirs: Vec<Vec<u16>> = Vec::new();
        let totals = &mut self.totals;
        let result = list_dir(&h, &mut self.class, &mut self.buf, |name, attributes, allocation, tag| {
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && is_name_surrogate(tag) {
                return;
            }
            if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                totals.dirs += 1;
                let mut full = Vec::with_capacity(path.len() + 1 + name.len());
                full.extend_from_slice(path);
                if full.last() != Some(&(b'\\' as u16)) {
                    full.push(b'\\' as u16);
                }
                full.extend_from_slice(name);
                subdirs.push(full);
            } else {
                totals.files += 1;
                totals.bytes += allocation;
            }
        });
        if result.is_err() {
            self.totals.errors += 1;
        }
        subdirs
    }
}

/// The queue shared by the workers. `busy` counts workers that popped a directory and have not yet
/// pushed its sub-directories; both fields change only under the one mutex.
struct Shared {
    stack: Vec<Vec<u16>>,
    busy: usize,
}

/// `\\?\`-prefixed UTF-16 form of `root`, as the scanner's `verbatim`.
fn verbatim(root: &Path) -> std::io::Result<Vec<u16>> {
    let abs = std::path::absolute(root)?;
    let w: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let s = OsString::from_wide(&w).to_string_lossy().into_owned();
    let mut out: Vec<u16> = Vec::with_capacity(w.len() + 8);
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        out.extend_from_slice(&w);
    } else if let Some(rest) = s.strip_prefix(r"\\") {
        out.extend(r"\\?\UNC\".encode_utf16());
        out.extend(rest.encode_utf16());
    } else {
        out.extend(r"\\?\".encode_utf16());
        out.extend_from_slice(&w);
    }
    Ok(out)
}

/// Walks `root` with `workers` threads (at least 1) and returns the merged totals and the wall
/// time. With one worker the walk is depth first on the calling thread's own stack; with more, the
/// workers share one queue of directories.
///
/// Termination. A worker leaves only when, under the mutex, the queue is empty and `busy == 0`.
/// A worker that pops a directory raises `busy` in the same critical section as the pop, and
/// lowers it in the same critical section that pushes the directory's children. So whenever the
/// queue is empty and `busy == 0`, no directory is queued and no worker holds one whose children
/// are still to come: nothing can be pushed any more. A worker that finds the queue empty while
/// `busy > 0` waits on the condvar and is woken by every push and by the last `busy` decrement.
/// Each directory path is pushed once (by the worker that listed its parent, which is listed once)
/// and popped once (a pop removes it), so it is counted once and none is skipped.
pub fn walk(root: &Path, workers: usize) -> Result<WalkResult, String> {
    let workers = workers.max(1);
    let root_w = verbatim(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let start = Instant::now();
    let mut totals = Totals::default();
    if workers == 1 {
        let mut w = Worker::new();
        let mut stack = vec![root_w];
        while let Some(dir) = stack.pop() {
            let children = w.process(&dir);
            stack.extend(children.into_iter().rev());
        }
        totals.merge(&w.totals);
    } else {
        let shared = Mutex::new(Shared { stack: vec![root_w], busy: 0 });
        let cv = Condvar::new();
        let parts: Vec<Totals> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    s.spawn(|| {
                        let mut w = Worker::new();
                        loop {
                            let dir = {
                                let mut g = shared.lock().unwrap();
                                loop {
                                    if let Some(d) = g.stack.pop() {
                                        g.busy += 1;
                                        break Some(d);
                                    }
                                    if g.busy == 0 {
                                        break None;
                                    }
                                    g = cv.wait(g).unwrap();
                                }
                            };
                            let Some(dir) = dir else {
                                // Queue empty and nobody busy: wake the others so they see it too.
                                cv.notify_all();
                                break;
                            };
                            let children = w.process(&dir);
                            let mut g = shared.lock().unwrap();
                            g.stack.extend(children);
                            g.busy -= 1;
                            drop(g);
                            cv.notify_all();
                        }
                        w.totals
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("a walker thread panicked")).collect()
        });
        for p in &parts {
            totals.merge(p);
        }
    }
    Ok(WalkResult { totals, elapsed: start.elapsed() })
}
