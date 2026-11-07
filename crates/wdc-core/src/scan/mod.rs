//! The read-only scanner (plan, section 4.1).
//!
//! It walks each root one directory at a time, single-threaded, and keeps only per-directory
//! totals, the top-N largest files and the set of large-file IDs (memory is bounded by the
//! number of directories, not files). Sizes are sizes on disk; a file with several names is
//! counted once; links are recorded and never followed; cloud placeholders count 0 and are
//! never read.

use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

#[cfg(windows)]
mod windows;
#[cfg(not(windows))]
mod portable;

/// Files at or above this size on disk are remembered by file ID, so a second name of the
/// same file counts 0 bytes. Smaller files are not tracked, which bounds memory.
pub const DEDUP_THRESHOLD: u64 = 1024 * 1024;

/// Files at or above this size are re-read individually: link count and size come from the
/// file itself, because the size stored in another name's directory entry can be stale.
pub const VERIFY_THRESHOLD: u64 = 64 * 1024 * 1024;

/// No parent: the node is a scan root.
pub const NO_PARENT: u32 = u32::MAX;

/// What to scan and how gently.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub roots: Vec<PathBuf>,
    /// Folders inside the roots that are never read (the personal folders of plan 5.3).
    pub excluded: Vec<PathBuf>,
    /// Pause between directories (`--throttle`).
    pub throttle: Option<Duration>,
    /// How many of the largest files to keep.
    pub top_files: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { roots: Vec::new(), excluded: Vec::new(), throttle: None, top_files: 50 }
    }
}

/// One directory and its totals. Directories form a tree through `parent` indexes; a child
/// always has a larger index than its parent.
#[derive(Clone, Debug, Serialize)]
pub struct DirNode {
    /// The folder name; for a root, its full path.
    pub name: String,
    pub parent: u32,
    pub depth: u16,
    /// Size on disk of the whole subtree.
    pub bytes: u64,
    /// Files in the whole subtree.
    pub files: u64,
    /// Size on disk of the files directly in this folder.
    pub own_bytes: u64,
    pub own_files: u32,
}

/// A large file.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TopFile {
    pub path: String,
    /// Size on disk counted for this name (0 when another name of the file was counted).
    pub bytes_on_disk: u64,
    pub logical_bytes: u64,
    /// Number of names (hard links) of the file, when it was read individually.
    pub links: u32,
}

impl Ord for TopFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes_on_disk
            .cmp(&other.bytes_on_disk)
            .then_with(|| other.path.cmp(&self.path))
    }
}
impl PartialOrd for TopFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Kind of a link that was recorded and not followed.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LinkKind {
    Junction,
    SymbolicLink,
    /// Another name-surrogate reparse point.
    OtherLink,
}

/// A link inside the scanned tree. It is recorded and never followed.
#[derive(Clone, Debug, Serialize)]
pub struct LinkEntry {
    pub path: String,
    pub kind: LinkKind,
    pub directory: bool,
}

/// Something that could not be read. The scan goes on.
#[derive(Clone, Debug, Serialize)]
pub struct ScanError {
    pub path: String,
    /// Windows error code (or OS error number elsewhere).
    pub code: u32,
    pub kind: &'static str,
}

/// Totals of a scan.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Totals {
    /// Size on disk, each file counted once.
    pub bytes_on_disk: u64,
    /// Sum of file lengths, for comparison.
    pub logical_bytes: u64,
    /// File names seen (a file with two names in the scope counts 2 here, its bytes once).
    pub files: u64,
    /// Folders walked, roots included.
    pub dirs: u64,
    /// Links recorded and not followed.
    pub links: u64,
    /// Extra names of large files that were counted 0 bytes because another name was counted.
    pub hardlink_duplicates: u64,
    /// Cloud placeholders and offline files, counted 0 and never read.
    pub placeholders: u64,
    /// Items that could not be read.
    pub errors: u64,
    /// Personal folders skipped.
    pub excluded: u64,
}

/// One scanned root.
#[derive(Clone, Debug, Serialize)]
pub struct RootReport {
    pub path: String,
    /// Index of the root's node in [`ScanReport::dirs`], when it could be opened.
    pub node: Option<u32>,
    pub bytes_on_disk: u64,
    pub files: u64,
    /// The root itself is a link; it was recorded and not followed.
    pub is_link: bool,
    pub error: Option<String>,
}

/// Progress during a scan.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub dirs: u64,
    pub files: u64,
    pub bytes_on_disk: u64,
    pub current: String,
}

/// Result of a scan.
#[derive(Clone, Debug, Serialize)]
pub struct ScanReport {
    pub started_unix_ms: u64,
    pub duration_ms: u64,
    pub roots: Vec<RootReport>,
    pub totals: Totals,
    #[serde(skip)]
    pub dirs: Vec<DirNode>,
    pub top_files: Vec<TopFile>,
    pub links: Vec<LinkEntry>,
    pub errors: Vec<ScanError>,
    pub excluded: Vec<String>,
    pub cancelled: bool,
}

/// Records kept while walking; shared by the platform walkers.
pub(crate) struct Collector<'a> {
    pub dirs: Vec<DirNode>,
    pub links: Vec<LinkEntry>,
    pub errors: Vec<ScanError>,
    pub excluded_paths: Vec<String>,
    pub totals: Totals,
    top: BinaryHeap<Reverse<TopFile>>,
    top_n: usize,
    progress: Option<&'a mut dyn FnMut(&Progress)>,
    last_progress: Instant,
    pub cancel: Option<&'a AtomicBool>,
    pub throttle: Option<Duration>,
}

/// Caps on recorded lists, so a pathological tree cannot grow the report without bound.
const MAX_LINKS: usize = 10_000;
const MAX_ERRORS: usize = 10_000;

impl<'a> Collector<'a> {
    pub fn add_dir(&mut self, name: String, parent: u32) -> u32 {
        let depth = if parent == NO_PARENT { 0 } else { self.dirs[parent as usize].depth + 1 };
        let idx = self.dirs.len() as u32;
        self.dirs.push(DirNode { name, parent, depth, bytes: 0, files: 0, own_bytes: 0, own_files: 0 });
        self.totals.dirs += 1;
        idx
    }

    pub fn add_file(&mut self, dir: u32, bytes_on_disk: u64, logical: u64) {
        let d = &mut self.dirs[dir as usize];
        d.own_bytes += bytes_on_disk;
        d.own_files += 1;
        self.totals.files += 1;
        self.totals.bytes_on_disk += bytes_on_disk;
        self.totals.logical_bytes += logical;
    }

    /// Whether a file of this size would enter the top-N list (the path is built only then).
    pub fn wants_top(&self, bytes_on_disk: u64) -> bool {
        if self.top_n == 0 {
            return false;
        }
        if self.top.len() < self.top_n {
            return true;
        }
        self.top.peek().map(|Reverse(min)| bytes_on_disk > min.bytes_on_disk).unwrap_or(true)
    }

    pub fn add_top(&mut self, f: TopFile) {
        self.top.push(Reverse(f));
        if self.top.len() > self.top_n {
            self.top.pop();
        }
    }

    pub fn add_link(&mut self, path: String, kind: LinkKind, directory: bool) {
        self.totals.links += 1;
        if self.links.len() < MAX_LINKS {
            self.links.push(LinkEntry { path, kind, directory });
        }
    }

    pub fn add_error(&mut self, path: String, code: u32) {
        self.totals.errors += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(ScanError { path, code, kind: error_kind(code) });
        }
    }

    pub fn add_excluded(&mut self, path: String) {
        self.totals.excluded += 1;
        self.excluded_paths.push(path);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
    }

    /// Called once per directory: throttling and progress reports.
    pub fn between_dirs(&mut self, current: impl FnOnce() -> String) {
        if let Some(t) = self.throttle {
            std::thread::sleep(t);
        }
        if self.progress.is_some() && self.last_progress.elapsed() >= Duration::from_millis(100) {
            self.last_progress = Instant::now();
            let p = Progress {
                dirs: self.totals.dirs,
                files: self.totals.files,
                bytes_on_disk: self.totals.bytes_on_disk,
                current: current(),
            };
            if let Some(cb) = self.progress.as_mut() {
                cb(&p);
            }
        }
    }
}

fn error_kind(code: u32) -> &'static str {
    #[cfg(windows)]
    {
        match code {
            5 => "access-denied",
            2 | 3 => "not-found",
            32 | 33 => "in-use",
            1920 | 1921 => "cannot-access",
            _ => "other",
        }
    }
    #[cfg(not(windows))]
    {
        match code {
            13 | 1 => "access-denied",
            2 => "not-found",
            _ => "other",
        }
    }
}

/// Scans the roots. Never changes anything on disk.
pub fn scan<'a>(
    options: &ScanOptions,
    progress: Option<&'a mut dyn FnMut(&Progress)>,
    cancel: Option<&'a AtomicBool>,
) -> ScanReport {
    let started_unix_ms = crate::time::now_unix_ms();
    let started = Instant::now();
    let mut c = Collector {
        dirs: Vec::new(),
        links: Vec::new(),
        errors: Vec::new(),
        excluded_paths: Vec::new(),
        totals: Totals::default(),
        top: BinaryHeap::new(),
        top_n: options.top_files,
        progress,
        last_progress: Instant::now(),
        cancel,
        throttle: options.throttle,
    };
    let mut roots = Vec::new();
    #[cfg(windows)]
    let mut walker = windows::Walker::new(&options.excluded);
    #[cfg(not(windows))]
    let mut walker = portable::Walker::new(&options.excluded);
    for root in &options.roots {
        if c.cancelled() {
            break;
        }
        roots.push(walker.walk_root(root, &mut c));
    }

    // Subtree totals: children always come after their parent, so one pass from the end
    // adds every folder into its parent.
    for d in c.dirs.iter_mut() {
        d.bytes = d.own_bytes;
        d.files = d.own_files as u64;
    }
    for i in (0..c.dirs.len()).rev() {
        let (parent, bytes, files) = {
            let d = &c.dirs[i];
            (d.parent, d.bytes, d.files)
        };
        if parent != NO_PARENT {
            let p = &mut c.dirs[parent as usize];
            p.bytes += bytes;
            p.files += files;
        }
    }
    for r in roots.iter_mut() {
        if let Some(n) = r.node {
            r.bytes_on_disk = c.dirs[n as usize].bytes;
            r.files = c.dirs[n as usize].files;
        }
    }

    let mut top: Vec<TopFile> = c.top.into_iter().map(|Reverse(f)| f).collect();
    top.sort_by(|a, b| b.cmp(a));
    let cancelled = c.cancel.map(|f| f.load(Ordering::Relaxed)).unwrap_or(false);
    ScanReport {
        started_unix_ms,
        duration_ms: started.elapsed().as_millis() as u64,
        roots,
        totals: c.totals,
        dirs: c.dirs,
        top_files: top,
        links: c.links,
        errors: c.errors,
        excluded: c.excluded_paths,
        cancelled,
    }
}

impl ScanReport {
    /// Full path of a directory node.
    pub fn dir_path(&self, idx: u32) -> String {
        let mut parts = Vec::new();
        let mut i = idx;
        while i != NO_PARENT {
            let d = &self.dirs[i as usize];
            parts.push(d.name.as_str());
            i = d.parent;
        }
        parts.reverse();
        let mut out = String::new();
        for (n, p) in parts.iter().enumerate() {
            if n > 0 && !out.ends_with(['\\', '/']) {
                out.push(std::path::MAIN_SEPARATOR);
            }
            out.push_str(p);
        }
        out
    }

    /// Children of every node, largest first.
    pub fn children_index(&self) -> Vec<Vec<u32>> {
        let mut children: Vec<Vec<u32>> = vec![Vec::new(); self.dirs.len()];
        for (i, d) in self.dirs.iter().enumerate() {
            if d.parent != NO_PARENT {
                children[d.parent as usize].push(i as u32);
            }
        }
        for list in children.iter_mut() {
            list.sort_by(|a, b| self.dirs[*b as usize].bytes.cmp(&self.dirs[*a as usize].bytes));
        }
        children
    }

    /// Finds the node of a folder by its full path (case-insensitive), if it was scanned.
    pub fn find_dir(&self, path: &str) -> Option<u32> {
        let want = normalize(path);
        for (ri, r) in self.roots.iter().enumerate() {
            let _ = ri;
            let Some(root) = r.node else { continue };
            let root_path = normalize(&r.path);
            if want == root_path {
                return Some(root);
            }
            let Some(rest) = want.strip_prefix(&root_path) else { continue };
            let rest = rest.trim_start_matches('\\');
            if rest.len() == want.len() {
                continue;
            }
            let mut cur = root;
            let children = self.children_of_lookup();
            for part in rest.split('\\').filter(|s| !s.is_empty()) {
                let next = children
                    .get(&cur)
                    .and_then(|list| list.iter().find(|&&c| self.dirs[c as usize].name.eq_ignore_ascii_case(part)))
                    .copied();
                match next {
                    Some(n) => cur = n,
                    None => return None,
                }
            }
            return Some(cur);
        }
        None
    }

    fn children_of_lookup(&self) -> std::collections::HashMap<u32, Vec<u32>> {
        let mut m: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        for (i, d) in self.dirs.iter().enumerate() {
            if d.parent != NO_PARENT {
                m.entry(d.parent).or_default().push(i as u32);
            }
        }
        m
    }

    /// The largest folders down to `max_depth` below each root, as (path, bytes, files).
    pub fn largest_dirs(&self, max_depth: u16, limit: usize) -> Vec<(String, u64, u64)> {
        let mut idx: Vec<u32> = (0..self.dirs.len() as u32)
            .filter(|&i| {
                let d = &self.dirs[i as usize];
                d.depth >= 1 && d.depth <= max_depth
            })
            .collect();
        idx.sort_by(|a, b| self.dirs[*b as usize].bytes.cmp(&self.dirs[*a as usize].bytes));
        idx.truncate(limit);
        idx.into_iter()
            .map(|i| (self.dir_path(i), self.dirs[i as usize].bytes, self.dirs[i as usize].files))
            .collect()
    }

    /// JSON summary for agents and `--json` (English, stable field names).
    pub fn summary_json(&self, max_depth: u16, dir_limit: usize) -> serde_json::Value {
        let tree: Vec<serde_json::Value> = self
            .largest_dirs(max_depth, dir_limit)
            .into_iter()
            .map(|(path, bytes, files)| {
                serde_json::json!({ "path": path, "bytes_on_disk": bytes, "files": files })
            })
            .collect();
        serde_json::json!({
            "started": crate::time::rfc3339(self.started_unix_ms),
            "duration_ms": self.duration_ms,
            "cancelled": self.cancelled,
            "roots": self.roots,
            "totals": self.totals,
            "largest_dirs": tree,
            "top_files": self.top_files,
            "links": self.links.iter().take(200).collect::<Vec<_>>(),
            "errors": self.errors.iter().take(200).collect::<Vec<_>>(),
            "excluded": self.excluded,
        })
    }

    /// Children of a scanned folder, largest first, for the `tree` tool.
    pub fn tree_json(&self, path: &str, limit: usize) -> Option<serde_json::Value> {
        let node = self.find_dir(path)?;
        let d = &self.dirs[node as usize];
        let lookup = self.children_of_lookup();
        let mut kids = lookup.get(&node).cloned().unwrap_or_default();
        kids.sort_by(|a, b| self.dirs[*b as usize].bytes.cmp(&self.dirs[*a as usize].bytes));
        let total = kids.len();
        let children: Vec<serde_json::Value> = kids
            .into_iter()
            .take(limit)
            .map(|k| {
                let c = &self.dirs[k as usize];
                serde_json::json!({ "name": c.name, "bytes_on_disk": c.bytes, "files": c.files })
            })
            .collect();
        Some(serde_json::json!({
            "path": self.dir_path(node),
            "bytes_on_disk": d.bytes,
            "files": d.files,
            "files_directly_inside": { "bytes_on_disk": d.own_bytes, "files": d.own_files },
            "children": children,
            "children_total": total,
        }))
    }
}

fn normalize(p: &str) -> String {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Display form of a root path given by the user.
pub(crate) fn root_display(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> ScanReport {
        let mut c = Collector {
            dirs: Vec::new(),
            links: Vec::new(),
            errors: Vec::new(),
            excluded_paths: Vec::new(),
            totals: Totals::default(),
            top: BinaryHeap::new(),
            top_n: 2,
            progress: None,
            last_progress: Instant::now(),
            cancel: None,
            throttle: None,
        };
        let r = c.add_dir(r"C:\root".into(), NO_PARENT);
        let a = c.add_dir("a".into(), r);
        let b = c.add_dir("b".into(), a);
        c.add_file(r, 10, 10);
        c.add_file(a, 100, 100);
        c.add_file(b, 1000, 1000);
        for (n, size) in [("x", 5u64), ("y", 50), ("z", 500)] {
            if c.wants_top(size) {
                c.add_top(TopFile { path: n.into(), bytes_on_disk: size, logical_bytes: size, links: 1 });
            }
        }
        let mut dirs = c.dirs;
        for d in dirs.iter_mut() {
            d.bytes = d.own_bytes;
            d.files = d.own_files as u64;
        }
        for i in (0..dirs.len()).rev() {
            let (p, by, f) = (dirs[i].parent, dirs[i].bytes, dirs[i].files);
            if p != NO_PARENT {
                dirs[p as usize].bytes += by;
                dirs[p as usize].files += f;
            }
        }
        let mut top: Vec<TopFile> = c.top.into_iter().map(|Reverse(f)| f).collect();
        top.sort_by(|a, b| b.cmp(a));
        ScanReport {
            started_unix_ms: 0,
            duration_ms: 0,
            roots: vec![RootReport {
                path: r"C:\root".into(),
                node: Some(0),
                bytes_on_disk: 1110,
                files: 3,
                is_link: false,
                error: None,
            }],
            totals: c.totals,
            dirs,
            top_files: top,
            links: vec![],
            errors: vec![],
            excluded: vec![],
            cancelled: false,
        }
    }

    #[test]
    fn subtree_totals_add_up() {
        let r = tree();
        assert_eq!(r.dirs[0].bytes, 1110);
        assert_eq!(r.dirs[1].bytes, 1100);
        assert_eq!(r.dirs[2].bytes, 1000);
        assert_eq!(r.dirs[0].files, 3);
        assert_eq!(r.totals.bytes_on_disk, 1110);
    }

    #[test]
    fn keeps_only_top_n_largest_files() {
        let r = tree();
        let names: Vec<_> = r.top_files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, vec!["z", "y"]);
    }

    #[test]
    fn finds_dirs_by_path_case_insensitively() {
        let r = tree();
        assert_eq!(r.find_dir(r"C:\root"), Some(0));
        assert_eq!(r.find_dir(r"c:\ROOT\A\b"), Some(2));
        assert_eq!(r.find_dir(r"C:\root\missing"), None);
        assert_eq!(r.find_dir(r"C:\rootx"), None);
        let t = r.tree_json(r"C:\root", 10).unwrap();
        assert_eq!(t["children"][0]["name"], "a");
        assert_eq!(t["children"][0]["bytes_on_disk"], 1100);
    }
}
