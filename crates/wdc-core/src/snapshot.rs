//! Snapshots: per-directory totals and the top files of each scan, as one compressed JSON
//! file under `<data>\snapshots\` (plan, section 4.1). Only the last N are kept, and their
//! total size is reported. Snapshots are what `wdc diff` (slice 2) compares.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::scan::ScanReport;

/// Snapshot format version.
pub const FORMAT: u32 = 1;
/// Folders deeper than this below a root are not stored (configurable later).
pub const DEFAULT_DEPTH: u16 = 6;
/// Snapshots kept.
pub const DEFAULT_KEEP: usize = 10;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SnapshotDir {
    pub path: String,
    pub bytes_on_disk: u64,
    pub files: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SnapshotFile {
    pub path: String,
    pub bytes_on_disk: u64,
    pub links: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub format: u32,
    pub tool_version: String,
    pub created: String,
    pub created_unix_ms: u64,
    pub roots: Vec<String>,
    pub bytes_on_disk: u64,
    pub files: u64,
    pub dirs: Vec<SnapshotDir>,
    pub top_files: Vec<SnapshotFile>,
}

/// Where a snapshot was saved and what the snapshot folder now holds.
#[derive(Clone, Debug, Serialize)]
pub struct Saved {
    pub path: String,
    pub kept: usize,
    pub total_bytes: u64,
}

impl Snapshot {
    pub fn from_report(report: &ScanReport, max_depth: u16) -> Snapshot {
        // Paths are built parent-first in one pass: a child always follows its parent.
        let mut paths: Vec<Option<String>> = vec![None; report.dirs.len()];
        let mut dirs = Vec::new();
        for (i, d) in report.dirs.iter().enumerate() {
            if d.depth > max_depth {
                continue;
            }
            let path = if d.parent == crate::scan::NO_PARENT {
                d.name.clone()
            } else {
                match &paths[d.parent as usize] {
                    Some(p) => {
                        let sep = if p.ends_with(['\\', '/']) { "" } else { std::path::MAIN_SEPARATOR_STR };
                        format!("{p}{sep}{}", d.name)
                    }
                    None => continue,
                }
            };
            dirs.push(SnapshotDir { path: path.clone(), bytes_on_disk: d.bytes, files: d.files });
            paths[i] = Some(path);
        }
        Snapshot {
            format: FORMAT,
            tool_version: crate::VERSION.to_string(),
            created: crate::time::rfc3339(report.started_unix_ms),
            created_unix_ms: report.started_unix_ms,
            roots: report.roots.iter().map(|r| r.path.clone()).collect(),
            bytes_on_disk: report.totals.bytes_on_disk,
            files: report.totals.files,
            dirs,
            top_files: report
                .top_files
                .iter()
                .map(|f| SnapshotFile { path: f.path.clone(), bytes_on_disk: f.bytes_on_disk, links: f.links })
                .collect(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let json = serde_json::to_vec(self).expect("snapshot serializes");
        ruzstd::encoding::compress_to_vec(json.as_slice(), ruzstd::encoding::CompressionLevel::Fastest)
    }

    pub fn decode(bytes: &[u8]) -> std::io::Result<Snapshot> {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let mut json = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut json)?;
        serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

pub fn snapshot_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("snapshots")
}

/// Saves a snapshot of `report` and keeps only the newest `keep` snapshots.
pub fn save(report: &ScanReport, data_dir: &Path, max_depth: u16, keep: usize) -> std::io::Result<Saved> {
    let dir = snapshot_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let snap = Snapshot::from_report(report, max_depth);
    let name = format!("scan-{}.json.zst", crate::time::file_stamp(report.started_unix_ms));
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&snap.encode())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;

    let mut existing = list(data_dir)?;
    while existing.len() > keep {
        let (old, _) = existing.remove(0);
        let _ = std::fs::remove_file(old);
    }
    let total_bytes = existing.iter().map(|(_, s)| *s).sum();
    Ok(Saved { path: path.to_string_lossy().into_owned(), kept: existing.len(), total_bytes })
}

/// Snapshot files, oldest first, with their sizes. Only the tool's own file names are listed.
pub fn list(data_dir: &Path) -> std::io::Result<Vec<(PathBuf, u64)>> {
    let dir = snapshot_dir(data_dir);
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("scan-") && name.ends_with(".json.zst") {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((e.path(), size));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan, ScanOptions};

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("wdc-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn round_trips_and_keeps_only_n() {
        let tree = temp("tree");
        std::fs::create_dir_all(tree.join("a").join("b")).unwrap();
        std::fs::write(tree.join("a").join("b").join("f.bin"), vec![7u8; 100_000]).unwrap();
        let data = temp("data");
        let mut report = scan(&ScanOptions { roots: vec![tree.clone()], ..Default::default() }, None, None);
        let mut last = None;
        for i in 0..4 {
            report.started_unix_ms = 1_794_082_500_000 + i * 1000;
            last = Some(save(&report, &data, DEFAULT_DEPTH, 3).unwrap());
        }
        let saved = last.unwrap();
        assert_eq!(saved.kept, 3);
        assert_eq!(list(&data).unwrap().len(), 3);
        let bytes = std::fs::read(&saved.path).unwrap();
        assert_eq!(saved.total_bytes, list(&data).unwrap().iter().map(|x| x.1).sum::<u64>());
        let snap = Snapshot::decode(&bytes).unwrap();
        assert_eq!(snap, Snapshot::from_report(&report, DEFAULT_DEPTH));
        assert_eq!(snap.dirs.len(), 3);
        assert!(snap.dirs[2].path.ends_with("b"));
        let _ = std::fs::remove_dir_all(tree);
        let _ = std::fs::remove_dir_all(data);
    }
}
