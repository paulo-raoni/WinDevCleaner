//! Installed programs (plan, section 5.5): read from the uninstall registry keys, each with the
//! program's own uninstall command for the person to run, and, after an uninstall, the folders
//! left behind whose name equals the program's name or publisher.
//!
//! Read-only in every way: the registry is opened with `KEY_READ` only, nothing is written to
//! it, no file is moved and no command is run. The only file written is the tool's own
//! `programs.json` in its data folder (the list seen at the last look).
//!
//! The pure parts (skipping patches, parsing dates and sizes, de-duplication, ordering, matching
//! leftovers on given lists) are separate from the registry reading, so they are tested anywhere.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::i18n::{Lang, Msg};

/// Whose uninstall key the program came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Machine,
}

/// Which registry view the key was read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum View {
    #[serde(rename = "64")]
    X64,
    #[serde(rename = "32")]
    X32,
}

/// The registry hive of a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    pub fn scope(self) -> Scope {
        match self {
            Hive::CurrentUser => Scope::User,
            Hive::LocalMachine => Scope::Machine,
        }
    }
}

/// One uninstall key to enumerate: hive, subkey path and registry view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub hive: Hive,
    pub path: String,
    pub view: View,
}

/// The three places Windows keeps uninstall entries.
pub fn default_sources() -> Vec<Source> {
    vec![
        Source { hive: Hive::CurrentUser, path: r"Software\Microsoft\Windows\CurrentVersion\Uninstall".into(), view: View::X64 },
        Source { hive: Hive::LocalMachine, path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall".into(), view: View::X64 },
        Source { hive: Hive::LocalMachine, path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall".into(), view: View::X32 },
    ]
}

/// What one subkey holds, as read, before any filtering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawEntry {
    pub key_name: String,
    pub scope: Option<Scope>,
    pub view: Option<View>,
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub display_version: Option<String>,
    /// `InstallDate` as stored (`YYYYMMDD` when well formed).
    pub install_date: Option<String>,
    /// `EstimatedSize`, in KB, as the installer wrote it.
    pub estimated_size_kb: Option<u32>,
    pub install_location: Option<String>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub system_component: Option<u32>,
    pub parent_key_name: Option<String>,
    pub release_type: Option<String>,
}

/// An installed program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub key_name: String,
    pub scope: Scope,
    pub view: View,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    /// `YYYY-MM-DD`.
    pub install_date: Option<String>,
    /// The size the installer reports (`EstimatedSize`, in bytes). Not a measured size.
    pub reported_size_bytes: Option<u64>,
    pub install_location: Option<String>,
    /// The program's own uninstall command, for the person to run. The tool never runs it.
    pub uninstall_command: Option<String>,
    pub quiet_uninstall_command: Option<String>,
}

/// A missing value is absent, never an empty string.
fn clean(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// `20240131` becomes `2024-01-31`; anything else (wrong length, letters, month 13, 31 February)
/// is absent.
pub fn parse_install_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() != 8 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (y, m, d): (u32, u32, u32) = (raw[0..4].parse().ok()?, raw[4..6].parse().ok()?, raw[6..8].parse().ok()?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if y == 0 || d == 0 || d > days {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// `EstimatedSize` is in KB; the tool gives bytes.
pub fn size_bytes(kb: u32) -> u64 {
    kb as u64 * 1024
}

/// Patches and updates are not programs.
fn is_patch(raw: &RawEntry) -> bool {
    let has_parent = raw.parent_key_name.as_deref().is_some_and(|p| !p.trim().is_empty());
    let release = raw.release_type.as_deref().map(|r| r.trim().to_lowercase());
    has_parent || matches!(release.as_deref(), Some("update") | Some("hotfix") | Some("security update"))
}

/// Filters, converts, de-duplicates and orders raw entries: sized programs largest first, then
/// the ones with no size, by name.
pub fn programs_from_raw(raw: Vec<RawEntry>) -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for r in raw {
        let Some(name) = clean(r.display_name.clone()) else { continue };
        if r.system_component == Some(1) || is_patch(&r) {
            continue;
        }
        let program = Program {
            key_name: r.key_name.clone(),
            scope: r.scope.unwrap_or(Scope::Machine),
            view: r.view.unwrap_or(View::X64),
            name,
            publisher: clean(r.publisher.clone()),
            version: clean(r.display_version.clone()),
            install_date: r.install_date.as_deref().and_then(parse_install_date),
            reported_size_bytes: r.estimated_size_kb.map(size_bytes),
            install_location: clean(r.install_location.clone()),
            uninstall_command: clean(r.uninstall_string.clone()),
            quiet_uninstall_command: clean(r.quiet_uninstall_string.clone()),
        };
        // The same program registered twice (both views, or per-user and per-machine) is one.
        let duplicate = out.iter().any(|p| {
            p.name == program.name && p.version == program.version && p.uninstall_command == program.uninstall_command
        });
        if !duplicate {
            out.push(program);
        }
    }
    out.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.key_name.cmp(&b.key_name));
        match (a.reported_size_bytes, b.reported_size_bytes) {
            (Some(x), Some(y)) => y.cmp(&x).then_with(by_name),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => by_name(),
        }
    });
    out
}

/// What a read of the sources found.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    pub programs: Vec<Program>,
    /// Sources or keys that could not be read. A key that does not exist is not an error.
    pub errors: Vec<String>,
}

/// Reads the sources (read-only) and returns the programs.
pub fn read_programs(sources: &[Source]) -> Reading {
    let (raw, errors) = read_raw(sources);
    Reading { programs: programs_from_raw(raw), errors }
}

#[cfg(not(windows))]
fn read_raw(_sources: &[Source]) -> (Vec<RawEntry>, Vec<String>) {
    // There is no uninstall registry off Windows.
    (Vec::new(), Vec::new())
}

#[cfg(windows)]
fn read_raw(sources: &[Source]) -> (Vec<RawEntry>, Vec<String>) {
    reg::read_raw(sources)
}

#[cfg(windows)]
mod reg {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_DWORD, REG_EXPAND_SZ, REG_SZ,
    };

    use super::{Hive, RawEntry, Source, View};

    /// A key opened for reading; closed on drop.
    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    enum Value {
        Text(String),
        Number(u32),
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn view_flag(view: View) -> u32 {
        match view {
            View::X64 => KEY_WOW64_64KEY,
            View::X32 => KEY_WOW64_32KEY,
        }
    }

    /// Opens `path` below `parent` with `KEY_READ` and the view flag, nothing else.
    /// `Ok(None)` when the key does not exist.
    fn open(parent: HKEY, path: &str, view: View) -> Result<Option<Key>, u32> {
        let mut handle: HKEY = null_mut();
        let w = wide(path);
        let r = unsafe { RegOpenKeyExW(parent, w.as_ptr(), 0, KEY_READ | view_flag(view), &mut handle) };
        match r {
            ERROR_SUCCESS => Ok(Some(Key(handle))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            other => Err(other),
        }
    }

    fn subkey_names(key: &Key) -> Result<Vec<String>, u32> {
        let mut names = Vec::new();
        let mut index = 0u32;
        loop {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            let r = unsafe { RegEnumKeyExW(key.0, index, buf.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) };
            match r {
                ERROR_SUCCESS => names.push(String::from_utf16_lossy(&buf[..len as usize])),
                ERROR_NO_MORE_ITEMS => return Ok(names),
                other => return Err(other),
            }
            index += 1;
        }
    }

    /// One value; absent when missing, of another type or unreadable.
    fn read_value(key: &Key, name: &str) -> Option<Value> {
        let w = wide(name);
        let mut ty = 0u32;
        let mut size = 0u32;
        let r = unsafe { RegQueryValueExW(key.0, w.as_ptr(), null(), &mut ty, null_mut(), &mut size) };
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
            return None;
        }
        // The value may grow between the two calls: retry with the size Windows reports.
        let mut buf = vec![0u8; size as usize + 2];
        for _ in 0..4 {
            let mut got = buf.len() as u32;
            let r = unsafe { RegQueryValueExW(key.0, w.as_ptr(), null(), &mut ty, buf.as_mut_ptr(), &mut got) };
            match r {
                ERROR_SUCCESS => {
                    buf.truncate(got as usize);
                    return match ty {
                        REG_SZ | REG_EXPAND_SZ => {
                            let units: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                            Some(Value::Text(String::from_utf16_lossy(&units[..end])))
                        }
                        REG_DWORD if buf.len() >= 4 => Some(Value::Number(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))),
                        _ => None,
                    };
                }
                ERROR_MORE_DATA => buf = vec![0u8; got as usize + 2],
                _ => return None,
            }
        }
        None
    }

    fn text(key: &Key, name: &str) -> Option<String> {
        match read_value(key, name) {
            Some(Value::Text(s)) => Some(s),
            _ => None,
        }
    }

    fn number(key: &Key, name: &str) -> Option<u32> {
        match read_value(key, name) {
            Some(Value::Number(n)) => Some(n),
            _ => None,
        }
    }

    pub fn read_raw(sources: &[Source]) -> (Vec<RawEntry>, Vec<String>) {
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        for source in sources {
            let hive = match source.hive {
                Hive::CurrentUser => HKEY_CURRENT_USER,
                Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            };
            let label = format!("{:?}\\{} ({:?})", source.hive, source.path, source.view);
            let root = match open(hive, &source.path, source.view) {
                Ok(Some(k)) => k,
                Ok(None) => continue,
                Err(code) => {
                    errors.push(format!("{label}: error {code}"));
                    continue;
                }
            };
            let names = match subkey_names(&root) {
                Ok(n) => n,
                Err(code) => {
                    errors.push(format!("{label}: error {code}"));
                    continue;
                }
            };
            for name in names {
                let key = match open(root.0, &name, source.view) {
                    Ok(Some(k)) => k,
                    Ok(None) => continue,
                    Err(code) => {
                        errors.push(format!("{label}\\{name}: error {code}"));
                        continue;
                    }
                };
                entries.push(RawEntry {
                    key_name: name,
                    scope: Some(source.hive.scope()),
                    view: Some(source.view),
                    display_name: text(&key, "DisplayName"),
                    publisher: text(&key, "Publisher"),
                    display_version: text(&key, "DisplayVersion"),
                    install_date: text(&key, "InstallDate"),
                    estimated_size_kb: number(&key, "EstimatedSize"),
                    install_location: text(&key, "InstallLocation"),
                    uninstall_string: text(&key, "UninstallString"),
                    quiet_uninstall_string: text(&key, "QuietUninstallString"),
                    system_component: number(&key, "SystemComponent"),
                    parent_key_name: text(&key, "ParentKeyName"),
                    release_type: text(&key, "ReleaseType"),
                });
            }
        }
        (entries, errors)
    }
}

// ---------------------------------------------------------------------------------------------
// Leftovers

/// What the tool remembers of a program between two looks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remembered {
    pub name: String,
    pub publisher: Option<String>,
    pub scope: Scope,
}

pub fn remember(programs: &[Program]) -> Vec<Remembered> {
    programs.iter().map(|p| Remembered { name: p.name.clone(), publisher: p.publisher.clone(), scope: p.scope }).collect()
}

const LIST_FILE: &str = "programs.json";

/// The list saved by the previous look; `None` when there is none (or it cannot be read).
pub fn load_list(data_dir: &Path) -> Option<Vec<Remembered>> {
    let text = std::fs::read_to_string(data_dir.join(LIST_FILE)).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(v.get("programs")?.clone()).ok()
}

/// Saves the list of this look, for the next one.
pub fn save_list(data_dir: &Path, list: &[Remembered]) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let body = json!({
        "saved_at": crate::time::rfc3339(crate::time::now_unix_ms()),
        "programs": list,
    });
    let tmp = data_dir.join(format!("{LIST_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&body).unwrap_or_default())?;
    std::fs::rename(&tmp, data_dir.join(LIST_FILE))
}

/// A folder directly inside one of the roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub root: PathBuf,
    pub name: String,
}

impl Child {
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.name)
    }
}

/// `%APPDATA%`, `%LOCALAPPDATA%` and `%LOCALAPPDATA%\Programs`.
pub fn leftover_roots() -> Vec<PathBuf> {
    let env = |n: &str| std::env::var_os(n).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut roots = Vec::new();
    if let Some(a) = env("APPDATA") {
        roots.push(a);
    }
    if let Some(l) = env("LOCALAPPDATA") {
        roots.push(l.join("Programs"));
        roots.push(l);
    }
    roots
}

/// The folders that are direct children of the roots (links are not folders here).
pub fn list_children(roots: &[PathBuf]) -> Vec<Child> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for entry in rd.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(Child { root: root.clone(), name: name.to_string() });
                }
            }
        }
    }
    out
}

/// Equal ignoring case, for deciding that two programs are the same one.
fn same_text(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A folder name equals a program name or publisher: ignoring case and nothing else. `Foo` never
/// equals `Foo Bar`, `FooBar` or `Foo 2`.
#[allow(unused_mut, unused_assignments)]
fn name_equals(folder: &str, target: &str) -> bool {
    let mut equal = false;
    // GUARD-BEGIN(leftover-exact)
    equal = folder.to_lowercase() == target.to_lowercase();
    // GUARD-END(leftover-exact)
    equal
}

/// A publisher folder is reported only when no program still installed has that publisher.
#[allow(unused_mut, unused_assignments)]
fn publisher_is_free(publisher: &str, current: &[Remembered]) -> bool {
    let mut free = true;
    // GUARD-BEGIN(leftover-publisher)
    free = !current.iter().any(|c| c.publisher.as_deref().is_some_and(|p| same_text(p, publisher)));
    // GUARD-END(leftover-publisher)
    free
}

/// Which field of the program the folder is named after.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    Name,
    Publisher,
}

impl Field {
    pub fn text(self) -> Msg {
        match self {
            Field::Name => Msg::ProgFieldName,
            Field::Publisher => Msg::ProgFieldPublisher,
        }
    }
}

/// A folder left behind: a candidate only, never actionable, never in a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leftover {
    pub folder: String,
    /// The uninstalled program.
    pub program: String,
    pub publisher: Option<String>,
    /// Which field of the program the folder is named after.
    pub matched: Field,
    pub class: crate::rules::Class,
    pub candidate: bool,
    pub actionable: bool,
    /// The command that explains the folder.
    pub explain: String,
    /// English text for JSON.
    pub note: String,
}

/// The explain command for a folder.
pub fn explain_command(folder: &str) -> String {
    format!("wdc explain \"{folder}\"")
}

/// Folders left by programs in `previous` and absent from `current`.
pub fn leftovers(previous: &[Remembered], current: &[Remembered], children: &[Child]) -> Vec<Leftover> {
    let mut out: Vec<Leftover> = Vec::new();
    for gone in previous {
        let still_installed = current.iter().any(|c| {
            same_text(&c.name, &gone.name)
                && match (&c.publisher, &gone.publisher) {
                    (Some(a), Some(b)) => same_text(a, b),
                    (None, None) => true,
                    _ => false,
                }
        });
        if still_installed {
            continue;
        }
        for child in children {
            let matched = if name_equals(&child.name, &gone.name) {
                Some(Field::Name)
            } else if gone.publisher.as_deref().is_some_and(|p| name_equals(&child.name, p) && publisher_is_free(p, current)) {
                Some(Field::Publisher)
            } else {
                None
            };
            let Some(matched) = matched else { continue };
            let folder = child.path().display().to_string();
            if out.iter().any(|l| same_text(&l.folder, &folder)) {
                continue;
            }
            out.push(Leftover {
                note: Lang::En.fmt(Msg::ProgLeftoverNote, &[("explain", &explain_command(&folder))]),
                explain: explain_command(&folder),
                folder,
                program: gone.name.clone(),
                publisher: gone.publisher.clone(),
                matched,
                class: crate::rules::Class::InstallLeftover,
                candidate: true,
                actionable: false,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// One look

/// Everything one `programs` run reports.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub programs: Vec<Program>,
    pub leftovers: Vec<Leftover>,
    /// There was no list from a previous look, so there is nothing to compare with yet.
    pub first_look: bool,
    /// The leftovers were compared with the previous list. False when some key could not be
    /// read (a program could look uninstalled only because its key was unreadable) or on the
    /// first look.
    pub leftovers_checked: bool,
    pub errors: Vec<String>,
}

impl Report {
    /// The sum of the sizes the installers report.
    pub fn reported_bytes_total(&self) -> u64 {
        self.programs.iter().filter_map(|p| p.reported_size_bytes).sum()
    }

    /// English, stable JSON for agents and `--json`.
    pub fn to_json(&self) -> Value {
        json!({
            "count": self.programs.len(),
            "reported_bytes_total": self.reported_bytes_total(),
            "size_note": Lang::En.t(Msg::ProgSizeNote),
            "note": Lang::En.t(Msg::ProgReadOnlyNote),
            "programs": self.programs,
            "first_look": self.first_look,
            "leftovers_checked": self.leftovers_checked,
            "leftovers": self.leftovers,
            "errors": self.errors,
        })
    }
}

/// Reads the sources, compares with the list saved at the previous look, saves this one.
/// Nothing is changed anywhere else.
pub fn look(sources: &[Source], data_dir: &Path, roots: &[PathBuf]) -> Report {
    let Reading { programs, errors } = read_programs(sources);
    let current = remember(&programs);
    let previous = load_list(data_dir);
    let mut report = Report { first_look: previous.is_none(), errors, ..Default::default() };
    if report.errors.is_empty() {
        if let Some(previous) = &previous {
            report.leftovers = leftovers(previous, &current, &list_children(roots));
            report.leftovers_checked = true;
        }
        if let Err(e) = save_list(data_dir, &current) {
            report.errors.push(format!("could not save {LIST_FILE}: {e}"));
        }
    }
    report.programs = programs;
    report
}
