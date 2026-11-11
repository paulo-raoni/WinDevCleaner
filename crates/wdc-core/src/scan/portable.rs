//! Development walker for non-Windows hosts. WinDevCleaner is a Windows tool; this exists
//! only so the platform-independent parts (tree totals, snapshots, MCP) can be built and
//! tested in the Linux cloud session. It follows the same rules with POSIX data: size on
//! disk from allocated blocks, one count per (device, inode), symbolic links not followed.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::parallel::{self, DirWalker};
use super::{Collector, LinkKind, RootReport, SerialSink, Sink, NO_PARENT};

pub(crate) struct Walker {
    excluded: Vec<PathBuf>,
}

impl Walker {
    pub fn new(excluded: &[PathBuf]) -> Walker {
        Walker { excluded: excluded.to_vec() }
    }

    pub fn walk_root(&mut self, root: &Path, c: &mut Collector) -> RootReport {
        let display = super::root_display(root);
        let mut report =
            RootReport { path: display.clone(), node: None, bytes_on_disk: 0, files: 0, is_link: false, error: None };
        let md = match std::fs::symlink_metadata(root) {
            Ok(m) => m,
            Err(e) => {
                report.error = Some(e.to_string());
                c.add_error(display, e.raw_os_error().unwrap_or(0) as u32);
                return report;
            }
        };
        if md.file_type().is_symlink() {
            report.is_link = true;
            c.add_link(display, LinkKind::SymbolicLink, true);
            return report;
        }
        let root_idx = c.add_dir(display, NO_PARENT);
        report.node = Some(root_idx);
        let root_path = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        if c.jobs > 1 {
            parallel::walk_tree(c, root_idx, root_path, self);
            return report;
        }
        let mut stack = vec![(root_idx, root_path)];
        while let Some((dir, path)) = stack.pop() {
            if c.cancelled() {
                break;
            }
            c.between_dirs(|| path.to_string_lossy().into_owned());
            let subdirs = self.list(&path, &mut SerialSink { c, dir });
            for (name, full) in subdirs.into_iter().rev() {
                let idx = c.add_dir(name, dir);
                stack.push((idx, full));
            }
        }
        report
    }
}

impl DirWalker for Walker {
    type Path = PathBuf;

    fn fork(&self) -> Walker {
        Walker { excluded: self.excluded.clone() }
    }

    fn display(path: &PathBuf) -> String {
        path.to_string_lossy().into_owned()
    }

    fn list<S: Sink>(&mut self, path: &PathBuf, sink: &mut S) -> Vec<(String, PathBuf)> {
        let rd = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(e) => {
                sink.error(path.to_string_lossy().into_owned(), e.raw_os_error().unwrap_or(0) as u32);
                return Vec::new();
            }
        };
        let mut subdirs = Vec::new();
        for entry in rd.flatten() {
            let full = entry.path();
            let Ok(md) = std::fs::symlink_metadata(&full) else { continue };
            let ft = md.file_type();
            if ft.is_symlink() {
                let is_dir = std::fs::metadata(&full).map(|m| m.is_dir()).unwrap_or(false);
                sink.link(full.to_string_lossy().into_owned(), LinkKind::SymbolicLink, is_dir);
                continue;
            }
            if ft.is_dir() {
                if self.excluded.iter().any(|e| e == &full) {
                    sink.excluded(full.to_string_lossy().into_owned());
                } else {
                    subdirs.push((entry.file_name().to_string_lossy().into_owned(), full));
                }
                continue;
            }
            sink.file(
                (md.dev(), md.ino() as u128),
                md.blocks() * 512,
                md.len(),
                md.nlink() as u32,
                || full.to_string_lossy().into_owned(),
            );
        }
        subdirs
    }
}
