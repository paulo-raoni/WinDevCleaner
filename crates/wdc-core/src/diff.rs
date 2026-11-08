//! "What grew since last time?" (plan, sections 4.1 and 4.4): compares two snapshots.
//!
//! New and grown folders come first, even when no rule knows them. A folder whose change is
//! almost all explained by one of its subfolders is left out, so the list points at where the
//! growth really is instead of repeating every ancestor of it. A new folder is reported once,
//! at the top of the new subtree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::snapshot::{self, Snapshot};

/// Kind of change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    New,
    Grown,
    Shrunk,
    Removed,
}

/// A folder that changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DirChange {
    pub path: String,
    pub change: Change,
    pub old_bytes: u64,
    pub new_bytes: u64,
    pub delta_bytes: i64,
    pub old_files: u64,
    pub new_files: u64,
    /// The rule that knows this folder, if any.
    #[serde(default)]
    pub rule: Option<String>,
    /// No rule knows this folder: it is what an agent investigates next.
    #[serde(default)]
    pub unknown: bool,
}

/// A large file that is new among the largest files, or grew.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub change: Change,
    pub old_bytes: u64,
    pub new_bytes: u64,
    pub delta_bytes: i64,
}

/// One side of a diff.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Side {
    pub snapshot: String,
    pub created: String,
    pub bytes_on_disk: u64,
    pub files: u64,
}

/// The result of comparing two snapshots.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diff {
    pub from: Side,
    pub to: Side,
    pub roots: Vec<String>,
    pub delta_bytes: i64,
    /// Changes of at least `min_bytes`: new and grown first, then removed and shrunk.
    pub dirs: Vec<DirChange>,
    pub files: Vec<FileChange>,
    /// Changes left out by `limit`.
    pub more: usize,
    pub min_bytes: u64,
}

/// What to show.
#[derive(Clone, Copy, Debug)]
pub struct DiffOptions {
    /// Smallest change shown.
    pub min_bytes: u64,
    /// Most folders shown.
    pub limit: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions { min_bytes: 1024 * 1024, limit: 50 }
    }
}

fn key(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn parent_key(k: &str) -> Option<String> {
    let i = k.rfind('\\')?;
    Some(k[..i].to_string()).filter(|p| !p.is_empty())
}

fn side(path: &str, s: &Snapshot) -> Side {
    Side { snapshot: path.to_string(), created: s.created.clone(), bytes_on_disk: s.bytes_on_disk, files: s.files }
}

/// Compares `old` with `new`.
#[allow(unused_mut, unused_assignments)]
pub fn diff(old_path: &str, old: &Snapshot, new_path: &str, new: &Snapshot, opts: DiffOptions) -> Diff {
    let old_dirs: HashMap<String, &snapshot::SnapshotDir> = old.dirs.iter().map(|d| (key(&d.path), d)).collect();
    let new_dirs: HashMap<String, &snapshot::SnapshotDir> = new.dirs.iter().map(|d| (key(&d.path), d)).collect();
    let roots: Vec<String> = new.roots.iter().map(|r| key(r)).collect();

    // Change of every folder known on either side (0 when absent on one side).
    let mut delta: HashMap<&str, i64> = HashMap::new();
    for (k, d) in &new_dirs {
        let before = old_dirs.get(k).map(|o| o.bytes_on_disk).unwrap_or(0);
        delta.insert(k.as_str(), d.bytes_on_disk as i64 - before as i64);
    }
    for (k, d) in &old_dirs {
        if !new_dirs.contains_key(k) {
            delta.insert(k.as_str(), -(d.bytes_on_disk as i64));
        }
    }
    // Largest same-direction change among each folder's children.
    let mut grown_child: HashMap<String, i64> = HashMap::new();
    let mut shrunk_child: HashMap<String, i64> = HashMap::new();
    for (k, &d) in &delta {
        if let Some(p) = parent_key(k) {
            if d > 0 {
                let e = grown_child.entry(p).or_insert(0);
                *e = (*e).max(d);
            } else if d < 0 {
                let e = shrunk_child.entry(p).or_insert(0);
                *e = (*e).min(d);
            }
        }
    }
    let explained_by_child = |k: &str, d: i64| -> bool {
        if d > 0 {
            grown_child.get(k).map(|&c| c * 10 >= d * 9).unwrap_or(false)
        } else {
            shrunk_child.get(k).map(|&c| c * 10 <= d * 9).unwrap_or(false)
        }
    };

    let mut changes: Vec<DirChange> = Vec::new();
    for (k, d) in &new_dirs {
        // A root's change is the total, shown apart.
        if roots.contains(k) {
            continue;
        }
        let old_d = old_dirs.get(k);
        let mut change = None;
        match old_d {
            Some(o) if o.bytes_on_disk != d.bytes_on_disk => {
                // GUARD-BEGIN(diff-grown)
                change = Some(if d.bytes_on_disk > o.bytes_on_disk { Change::Grown } else { Change::Shrunk });
                // GUARD-END(diff-grown)
            }
            Some(_) => {}
            None => {
                // Only the top of a new subtree: its parent existed before.
                let top = parent_key(k).map(|p| old_dirs.contains_key(&p)).unwrap_or(false);
                // GUARD-BEGIN(diff-new)
                if top {
                    change = Some(Change::New);
                }
                // GUARD-END(diff-new)
            }
        }
        let Some(change) = change else { continue };
        let before = old_d.map(|o| o.bytes_on_disk).unwrap_or(0);
        let dd = d.bytes_on_disk as i64 - before as i64;
        if dd.unsigned_abs() < opts.min_bytes || (change != Change::New && explained_by_child(k, dd)) {
            continue;
        }
        changes.push(DirChange {
            path: d.path.clone(),
            change,
            old_bytes: before,
            new_bytes: d.bytes_on_disk,
            delta_bytes: dd,
            old_files: old_d.map(|o| o.files).unwrap_or(0),
            new_files: d.files,
            rule: None,
            unknown: false,
        });
    }
    for (k, o) in &old_dirs {
        if new_dirs.contains_key(k) {
            continue;
        }
        // Only the top of a removed subtree: its parent still exists.
        let top = parent_key(k).map(|p| new_dirs.contains_key(&p)).unwrap_or(false);
        if top && o.bytes_on_disk >= opts.min_bytes {
            changes.push(DirChange {
                path: o.path.clone(),
                change: Change::Removed,
                old_bytes: o.bytes_on_disk,
                new_bytes: 0,
                delta_bytes: -(o.bytes_on_disk as i64),
                old_files: o.files,
                new_files: 0,
                rule: None,
                unknown: false,
            });
        }
    }
    // New and grown first (largest growth first), then removed and shrunk (largest first).
    changes.sort_by(|a, b| {
        let ka = (a.delta_bytes <= 0, -(a.delta_bytes.abs()), a.path.clone());
        let kb = (b.delta_bytes <= 0, -(b.delta_bytes.abs()), b.path.clone());
        ka.cmp(&kb)
    });
    let more = changes.len().saturating_sub(opts.limit);
    changes.truncate(opts.limit);

    // Large files: new among the largest, or grown.
    let old_files: HashMap<String, u64> = old.top_files.iter().map(|f| (key(&f.path), f.bytes_on_disk)).collect();
    let mut files: Vec<FileChange> = new
        .top_files
        .iter()
        .filter_map(|f| {
            let before = old_files.get(&key(&f.path)).copied();
            let dd = f.bytes_on_disk as i64 - before.unwrap_or(0) as i64;
            if dd <= 0 || (dd as u64) < opts.min_bytes {
                return None;
            }
            Some(FileChange {
                path: f.path.clone(),
                change: if before.is_some() { Change::Grown } else { Change::New },
                old_bytes: before.unwrap_or(0),
                new_bytes: f.bytes_on_disk,
                delta_bytes: dd,
            })
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.delta_bytes));
    files.truncate(opts.limit);

    Diff {
        from: side(old_path, old),
        to: side(new_path, new),
        roots: new.roots.clone(),
        delta_bytes: new.bytes_on_disk as i64 - old.bytes_on_disk as i64,
        dirs: changes,
        files,
        more,
        min_bytes: opts.min_bytes,
    }
}

impl Diff {
    /// Names the rule that knows each grown or new folder, and flags the ones no rule knows.
    pub fn annotate(&mut self, rules: &crate::rules::RuleSet, ctx: &crate::rules::Context) {
        for c in self.dirs.iter_mut() {
            c.rule = rules.covers(&c.path, ctx);
            c.unknown = c.rule.is_none() && c.delta_bytes > 0;
        }
    }
}

/// Why a diff could not be made.
#[derive(Debug, PartialEq)]
pub enum DiffError {
    /// Fewer than two snapshots of the same folders.
    NotEnoughSnapshots,
    Io(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NotEnoughSnapshots => {
                write!(f, "need two scans of the same folders; scan again later and compare")
            }
            DiffError::Io(e) => write!(f, "{e}"),
        }
    }
}

fn load(p: &Path) -> Result<Snapshot, DiffError> {
    let bytes = std::fs::read(p).map_err(|e| DiffError::Io(format!("{}: {e}", p.display())))?;
    Snapshot::decode(&bytes).map_err(|e| DiffError::Io(format!("{}: {e}", p.display())))
}

/// Picks the snapshots to compare: `to` (default: the newest) and `from` (default: the newest
/// older snapshot of the same folders), and compares them.
pub fn diff_snapshots(
    data_dir: &Path,
    from: Option<&Path>,
    to: Option<&Path>,
    opts: DiffOptions,
) -> Result<Diff, DiffError> {
    let list: Vec<PathBuf> = snapshot::list(data_dir).map_err(|e| DiffError::Io(e.to_string()))?.into_iter().map(|(p, _)| p).collect();
    let to_path = match to {
        Some(p) => p.to_path_buf(),
        None => list.last().cloned().ok_or(DiffError::NotEnoughSnapshots)?,
    };
    let new = load(&to_path)?;
    let (from_path, old) = match from {
        Some(p) => (p.to_path_buf(), load(p)?),
        None => {
            let same_roots = |s: &Snapshot| {
                let a: Vec<String> = s.roots.iter().map(|r| key(r)).collect();
                let b: Vec<String> = new.roots.iter().map(|r| key(r)).collect();
                a == b
            };
            let mut found = None;
            for p in list.iter().rev() {
                if *p == to_path || p.file_name() >= to_path.file_name() {
                    continue;
                }
                let s = load(p)?;
                if same_roots(&s) {
                    found = Some((p.clone(), s));
                    break;
                }
            }
            found.ok_or(DiffError::NotEnoughSnapshots)?
        }
    };
    Ok(diff(&from_path.to_string_lossy(), &old, &to_path.to_string_lossy(), &new, opts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{SnapshotDir, SnapshotFile};

    const MIB: u64 = 1024 * 1024;

    fn snap(dirs: &[(&str, u64)], files: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            format: 1,
            tool_version: "t".into(),
            created: "c".into(),
            created_unix_ms: 0,
            roots: vec![r"C:\root".into()],
            bytes_on_disk: dirs.first().map(|d| d.1).unwrap_or(0),
            files: 0,
            dirs: dirs.iter().map(|(p, b)| SnapshotDir { path: p.to_string(), bytes_on_disk: *b, files: 1 }).collect(),
            top_files: files.iter().map(|(p, b)| SnapshotFile { path: p.to_string(), bytes_on_disk: *b, links: 1 }).collect(),
        }
    }

    fn old() -> Snapshot {
        snap(
            &[
                (r"C:\root", 30 * MIB),
                (r"C:\root\cache", 10 * MIB),
                (r"C:\root\cache\a", 10 * MIB),
                (r"C:\root\gone", 5 * MIB),
                (r"C:\root\gone\deep", 5 * MIB),
                (r"C:\root\same", 15 * MIB),
            ],
            &[(r"C:\root\same\x.bin", 15 * MIB)],
        )
    }

    fn new() -> Snapshot {
        snap(
            &[
                (r"C:\root", 60 * MIB),
                (r"C:\root\cache", 30 * MIB),
                (r"C:\root\cache\a", 30 * MIB),
                (r"C:\root\fresh", 15 * MIB),
                (r"C:\root\fresh\sub", 15 * MIB),
                (r"C:\root\same", 15 * MIB),
            ],
            &[(r"C:\root\same\x.bin", 15 * MIB), (r"C:\root\fresh\sub\vm.vhdx", 15 * MIB)],
        )
    }

    #[test]
    fn new_and_grown_first_at_the_place_that_changed() {
        let d = diff("old", &old(), "new", &new(), DiffOptions::default());
        let got: Vec<(&str, Change, i64)> = d.dirs.iter().map(|c| (c.path.as_str(), c.change, c.delta_bytes)).collect();
        assert_eq!(
            got,
            vec![
                // cache grew because of cache\a: only the deepest place is shown.
                (r"C:\root\cache\a", Change::Grown, 20 * MIB as i64),
                // A new subtree is shown once, at its top.
                (r"C:\root\fresh", Change::New, 15 * MIB as i64),
                (r"C:\root\gone", Change::Removed, -(5 * MIB as i64)),
            ]
        );
        assert_eq!(d.delta_bytes, 30 * MIB as i64);
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].change, Change::New);
        assert!(d.files[0].path.ends_with("vm.vhdx"));
    }

    #[test]
    fn small_changes_are_left_out() {
        let d = diff("old", &old(), "new", &new(), DiffOptions { min_bytes: 16 * MIB, limit: 50 });
        assert_eq!(d.dirs.len(), 1);
        assert_eq!(d.dirs[0].path, r"C:\root\cache\a");
    }

    #[test]
    fn nothing_changed_means_an_empty_diff() {
        let d = diff("a", &old(), "b", &old(), DiffOptions::default());
        assert!(d.dirs.is_empty() && d.files.is_empty());
        assert_eq!(d.delta_bytes, 0);
    }
}
