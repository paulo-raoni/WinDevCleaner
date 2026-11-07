//! Where the tool reads and where it keeps its own data (plan, sections 4.1 and 5.3).

use std::path::PathBuf;

/// The default read scope: roots to walk and folders inside them that are never read.
#[derive(Clone, Debug, Default)]
pub struct ReadScope {
    pub roots: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The tool's data folder: `%LOCALAPPDATA%\WinDevCleaner`, or `WDC_DATA_DIR` when set
/// (tests and portable use). Slice 1 writes nothing outside this folder.
pub fn data_dir() -> PathBuf {
    if let Some(p) = env_path("WDC_DATA_DIR") {
        return p;
    }
    #[cfg(windows)]
    {
        if let Some(p) = env_path("LOCALAPPDATA") {
            return p.join("WinDevCleaner");
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(p) = env_path("XDG_DATA_HOME") {
            return p.join("WinDevCleaner");
        }
        if let Some(p) = env_path("HOME") {
            return p.join(".local").join("share").join("WinDevCleaner");
        }
    }
    std::env::temp_dir().join("WinDevCleaner")
}

fn starts_with_ci(path: &std::path::Path, base: &std::path::Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    let b = base.to_string_lossy().to_lowercase();
    let b = b.trim_end_matches(['\\', '/']);
    p == b || (p.starts_with(b) && matches!(p.as_bytes().get(b.len()), Some(b'\\') | Some(b'/')))
}

/// The default read scope: `%USERPROFILE%` without the personal folders, all of
/// `%ProgramData%`, and any allowed root (such as a redirected `%TEMP%`) outside both.
/// Never the whole disk.
pub fn default_read_scope() -> ReadScope {
    let mut scope = ReadScope::default();
    let profile = env_path("USERPROFILE").or_else(|| env_path("HOME"));
    if let Some(p) = &profile {
        scope.roots.push(p.clone());
    }
    if let Some(p) = env_path("ProgramData") {
        scope.roots.push(p);
    }
    // Allowed roots (act scope, plan 5.3) that live outside the roots above are read too.
    for name in ["LOCALAPPDATA", "APPDATA", "TEMP"] {
        if let Some(p) = env_path(name) {
            if !scope.roots.iter().any(|r| starts_with_ci(&p, r)) {
                scope.roots.push(p);
            }
        }
    }

    let mut excluded: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
            FOLDERID_Pictures, FOLDERID_Videos,
        };
        for id in [
            &FOLDERID_Documents,
            &FOLDERID_Desktop,
            &FOLDERID_Pictures,
            &FOLDERID_Videos,
            &FOLDERID_Music,
            &FOLDERID_Downloads,
        ] {
            if let Some(p) = crate::win::known_folder(id) {
                excluded.push(p);
            }
        }
    }
    for name in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Some(p) = env_path(name) {
            excluded.push(p);
        }
    }
    if let Some(p) = &profile {
        for name in ["Documents", "Desktop", "Pictures", "Videos", "Music", "Downloads", "OneDrive"] {
            excluded.push(p.join(name));
        }
        // "OneDrive - Company" folders of work accounts.
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("OneDrive - ") {
                    excluded.push(e.path());
                }
            }
        }
    }
    excluded.sort();
    excluded.dedup_by(|a, b| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()));
    scope.excluded = excluded;
    scope
}
