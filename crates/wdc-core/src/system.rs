//! System items (plan, section 4.1): space Windows itself manages, read through its own APIs
//! rather than by walking, and always report-only. Each item names the Windows setting or tool
//! that manages it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::{Lang, Msg};

/// Which system item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RecycleBin,
    PageFile,
    SwapFile,
    HibernationFile,
    ErrorReports,
    CrashDumps,
    MemoryDumps,
    LiveKernelReports,
    SystemRestore,
}

impl Kind {
    pub fn name(self) -> Msg {
        match self {
            Kind::RecycleBin => Msg::SysRecycleBin,
            Kind::PageFile => Msg::SysPageFile,
            Kind::SwapFile => Msg::SysSwapFile,
            Kind::HibernationFile => Msg::SysHibernation,
            Kind::ErrorReports => Msg::SysErrorReports,
            Kind::CrashDumps => Msg::SysCrashDumps,
            Kind::MemoryDumps => Msg::SysMemoryDumps,
            Kind::LiveKernelReports => Msg::SysLiveKernelReports,
            Kind::SystemRestore => Msg::SysSystemRestore,
        }
    }

    /// The Windows setting or tool that manages the item.
    pub fn managed_by(self) -> Msg {
        match self {
            Kind::RecycleBin => Msg::SysRecycleBinHow,
            Kind::PageFile => Msg::SysPageFileHow,
            Kind::SwapFile => Msg::SysSwapFileHow,
            Kind::HibernationFile => Msg::SysHibernationHow,
            Kind::ErrorReports => Msg::SysErrorReportsHow,
            Kind::CrashDumps => Msg::SysCrashDumpsHow,
            Kind::MemoryDumps | Kind::LiveKernelReports => Msg::SysMemoryDumpsHow,
            Kind::SystemRestore => Msg::SysSystemRestoreHow,
        }
    }
}

/// How the size was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Measured,
    /// The item does not exist on this machine.
    Absent,
    /// Windows only shows it to administrators; version 1 runs without admin rights.
    NeedsAdmin,
    /// Part of the item could be read and part needs an administrator: `bytes` is at least
    /// that much, never the whole.
    PartialNeedsAdmin,
    Error,
}

/// One system item. Never actionable: the tool only reports it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemItem {
    pub kind: Kind,
    /// English name, for JSON.
    pub name: String,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub items: Option<u64>,
    pub status: Status,
    /// English text of the Windows setting or tool that manages it, for JSON.
    pub managed_by: String,
    /// English remark on the status, when the status alone does not say enough (a lower bound,
    /// hibernation off). Absent from older JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn item(kind: Kind, path: Option<String>, bytes: Option<u64>, items: Option<u64>, status: Status) -> SystemItem {
    SystemItem {
        kind,
        name: kind.name().text(Lang::En).to_string(),
        path,
        bytes,
        items,
        status,
        managed_by: kind.managed_by().text(Lang::En).to_string(),
        note: match status {
            Status::PartialNeedsAdmin => Some(PARTIAL_NOTE.to_string()),
            _ => None,
        },
    }
}

const PARTIAL_NOTE: &str = "bytes is a lower bound: the rest needs an administrator";

/// The item for a hibernation file that does not exist on the system drive: hibernation is off.
pub fn hibernation_off(path: String) -> SystemItem {
    let mut it = item(Kind::HibernationFile, Some(path), Some(0), Some(0), Status::Absent);
    it.note = Some(Lang::En.t(Msg::SysHibernationOff).to_string());
    it
}

/// The status of a measured folder: `root_unreadable` when the folder itself could not be
/// opened, `kinds` the kind of each error met below it (`ScanError::kind`). A folder with
/// access errors below it is only partly measured.
pub fn folder_status(root_unreadable: bool, kinds: &[&str]) -> Status {
    let denied = kinds.contains(&"access-denied");
    if root_unreadable {
        return if kinds.first() == Some(&"access-denied") { Status::NeedsAdmin } else { Status::Error };
    }
    let mut status = Status::Measured;
    // GUARD-BEGIN(partial-needs-admin)
    if denied {
        status = Status::PartialNeedsAdmin;
    }
    // GUARD-END(partial-needs-admin)
    status
}

/// Whether the total of `items` is a lower bound: some item was only partly measured.
pub fn total_is_lower_bound(items: &[SystemItem]) -> bool {
    items.iter().any(|i| i.status == Status::PartialNeedsAdmin)
}

/// The size column for an item: its size, "at least N; ..." for a partial one, or the reason
/// there is none. Shared by the command line and the app.
pub fn size_text(lang: Lang, it: &SystemItem) -> String {
    match (it.kind, it.status, it.bytes) {
        (_, Status::PartialNeedsAdmin, Some(b)) => {
            lang.fmt(Msg::SysStatusPartial, &[("size", &crate::fmt::bytes(b))])
        }
        (Kind::HibernationFile, Status::Absent, _) => lang.t(Msg::SysHibernationOff).to_string(),
        (_, _, bytes) => match (bytes, it.status.note()) {
            (Some(b), None) => crate::fmt::bytes(b),
            (_, Some(note)) => lang.t(note).to_string(),
            (None, None) => String::new(),
        },
    }
}

/// Whether the human views list the item: absent ones are left out, except the hibernation
/// file, whose absence says hibernation is off.
pub fn is_listed(it: &SystemItem) -> bool {
    it.status != Status::Absent || it.kind == Kind::HibernationFile
}

/// Folders of reports and dumps, measured with the read-only scanner (small trees).
fn folder(kind: Kind, path: PathBuf) -> SystemItem {
    let display = path.display().to_string();
    if !path.exists() {
        return item(kind, Some(display), Some(0), Some(0), Status::Absent);
    }
    let opts = crate::scan::ScanOptions { roots: vec![path], top_files: 0, ..Default::default() };
    let r = crate::scan::scan(&opts, None, None);
    let root_unreadable = matches!(r.roots.first(), Some(root) if root.node.is_none());
    let kinds: Vec<&str> = r.errors.iter().map(|e| e.kind).collect();
    let status = folder_status(root_unreadable, &kinds);
    let (bytes, files) = if status == Status::Error { (None, None) } else { (Some(r.totals.bytes_on_disk), Some(r.totals.files)) };
    item(kind, Some(display), bytes, files, status)
}

fn env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Reads every system item. Read-only and quick: no walk of the whole disk.
pub fn system_items() -> Vec<SystemItem> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        out.push(recycle_bin());
        for drive in imp::fixed_drives() {
            for (kind, name) in
                [(Kind::PageFile, "pagefile.sys"), (Kind::SwapFile, "swapfile.sys"), (Kind::HibernationFile, "hiberfil.sys")]
            {
                match imp::root_file(kind, drive, name) {
                    Ok(it) => out.push(it),
                    // Hibernation is off only when the file is not there (file or path not found).
                    Err(2 | 3) if kind == Kind::HibernationFile && drive == imp::system_drive() => {
                        out.push(hibernation_off(format!("{drive}:\\{name}")));
                    }
                    // Any other failure on the hibernation file is an unreadable item, not "off".
                    Err(code) if kind == Kind::HibernationFile && drive == imp::system_drive() => {
                        let status = if code == 5 { Status::NeedsAdmin } else { Status::Error };
                        out.push(item(kind, Some(format!("{drive}:\\{name}")), None, None, status));
                    }
                    Err(_) => {}
                }
            }
        }
    }
    if let Some(local) = env("LOCALAPPDATA") {
        out.push(folder(Kind::ErrorReports, local.join("Microsoft").join("Windows").join("WER")));
        out.push(folder(Kind::CrashDumps, local.join("CrashDumps")));
    }
    if let Some(pd) = env("ProgramData") {
        out.push(folder(Kind::ErrorReports, pd.join("Microsoft").join("Windows").join("WER")));
    }
    if let Some(win) = env("SystemRoot").or_else(|| env("WINDIR")) {
        out.push(folder(Kind::LiveKernelReports, win.join("LiveKernelReports")));
        out.push(folder(Kind::MemoryDumps, win.join("Minidump")));
        let dump = win.join("MEMORY.DMP");
        let display = dump.display().to_string();
        out.push(match std::fs::metadata(&dump) {
            Ok(m) => item(Kind::MemoryDumps, Some(display), Some(m.len()), Some(1), Status::Measured),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                item(Kind::MemoryDumps, Some(display), Some(0), Some(0), Status::Absent)
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                item(Kind::MemoryDumps, Some(display), None, None, Status::NeedsAdmin)
            }
            Err(_) => item(Kind::MemoryDumps, Some(display), None, None, Status::Error),
        });
    }
    #[cfg(windows)]
    {
        // Shadow storage is only readable by administrators (vssadmin, WMI).
        out.push(item(Kind::SystemRestore, None, None, None, Status::NeedsAdmin));
    }
    out
}

/// Recycle Bin of every drive, as the shell counts it (`SHQueryRecycleBinW`).
#[cfg(windows)]
#[allow(unused_mut, unused_assignments)]
pub fn recycle_bin() -> SystemItem {
    use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
    let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, i64Size: 0, i64NumItems: 0 };
    let mut hr = 0;
    // GUARD-BEGIN(recycle-bin)
    hr = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    // GUARD-END(recycle-bin)
    if hr < 0 {
        return item(Kind::RecycleBin, None, None, None, Status::Error);
    }
    item(Kind::RecycleBin, None, Some(info.i64Size as u64), Some(info.i64NumItems as u64), Status::Measured)
}

#[cfg(windows)]
mod imp {
    use super::{item, Kind, Status, SystemItem};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileW, GetDriveTypeW, GetLogicalDrives, WIN32_FIND_DATAW,
    };

    const DRIVE_FIXED: u32 = 3;

    /// Letter of the drive Windows is installed on (`%SystemDrive%`, `C` when unset).
    pub fn system_drive() -> char {
        std::env::var("SystemDrive")
            .ok()
            .and_then(|v| v.chars().next())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('C')
    }

    /// Letters of the fixed drives (`C`, `D`, ...).
    pub fn fixed_drives() -> Vec<char> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| (b'A' + i) as char)
            .filter(|d| {
                let root: Vec<u16> = format!("{d}:\\").encode_utf16().chain(std::iter::once(0)).collect();
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
            })
            .collect()
    }

    /// A file in a volume's root read from its directory entry (`FindFirstFileW`), without
    /// opening it: the paging and hibernation files are always in use. `Err` is the Windows error
    /// code of the failed lookup (2 or 3: the file is not there).
    #[allow(unused_mut, unused_assignments)]
    pub fn root_file(kind: Kind, drive: char, name: &str) -> Result<SystemItem, u32> {
        let path = format!("{drive}:\\{name}");
        let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut d: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
        let h = unsafe { FindFirstFileW(w.as_ptr(), &mut d) };
        if h as isize == -1 {
            return Err(crate::win::last_error());
        }
        unsafe { FindClose(h) };
        let mut bytes = 0u64;
        // GUARD-BEGIN(system-file-size)
        bytes = ((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64;
        // GUARD-END(system-file-size)
        Ok(item(kind, Some(path), Some(bytes), Some(1), Status::Measured))
    }
}

impl Status {
    /// Text shown instead of a size, when there is none.
    pub fn note(self) -> Option<Msg> {
        match self {
            Status::Measured => None,
            Status::Absent => Some(Msg::SysStatusAbsent),
            Status::NeedsAdmin | Status::PartialNeedsAdmin => Some(Msg::SysStatusNeedsAdmin),
            Status::Error => Some(Msg::SysStatusError),
        }
    }
}

/// Total of the measured items.
pub fn total(items: &[SystemItem]) -> u64 {
    items.iter().filter_map(|i| i.bytes).sum()
}
