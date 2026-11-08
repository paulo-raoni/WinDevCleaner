// Handles are opaque kernel object references passed to Windows, never dereferenced here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindFirstFileW, GetCompressedFileSizeW, GetDiskFreeSpaceW, GetFileAttributesW,
    GetFileInformationByHandleEx, FileStandardInfo, SetFileAttributesW, FILE_STANDARD_INFO,
    INVALID_FILE_SIZE, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{FSCTL_SET_COMPRESSION, FSCTL_SET_SPARSE};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{GetProcessIoCounters, GetProcessTimes, IO_COUNTERS};

pub fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    // Long and Unicode paths: always the verbatim form.
    let abs = std::path::absolute(p).unwrap();
    let s = abs.as_os_str().to_string_lossy();
    if s.starts_with(r"\\?\") {
        wide(abs.as_os_str())
    } else {
        wide(OsStr::new(&format!(r"\\?\{s}")))
    }
}

fn cmd(args: &[&OsStr]) {
    let out = Command::new("cmd").arg("/c").args(args).output().expect("run cmd");
    assert!(
        out.status.success(),
        "cmd {:?} failed: {}{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A directory junction made by Windows' own `mklink /J`.
pub fn junction(link: &Path, target: &Path) {
    cmd(&[OsStr::new("mklink"), OsStr::new("/J"), link.as_os_str(), target.as_os_str()]);
}

/// A directory symbolic link (`mklink /D`; needs the symbolic-link privilege, which the
/// elevated CI runner has).
pub fn symlink_dir(link: &Path, target: &Path) {
    cmd(&[OsStr::new("mklink"), OsStr::new("/D"), link.as_os_str(), target.as_os_str()]);
}

/// A file symbolic link (`mklink`).
pub fn symlink_file(link: &Path, target: &Path) {
    cmd(&[OsStr::new("mklink"), link.as_os_str(), target.as_os_str()]);
}

/// Writes `len` bytes of real (non-sparse, incompressible-pattern) data.
pub fn write_file(path: &Path, len: u64) {
    let mut f = File::create(path).expect("create file");
    let mut chunk = vec![0u8; 1024 * 1024];
    let mut x: u32 = 0x1234_5678;
    for b in chunk.iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    let mut left = len;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        f.write_all(&chunk[..n]).expect("write");
        left -= n as u64;
    }
    f.sync_all().unwrap();
}

/// Appends `len` bytes through this name.
pub fn append(path: &Path, len: u64) {
    let mut f = OpenOptions::new().append(true).open(path).expect("open for append");
    f.write_all(&vec![0xA5u8; len as usize]).expect("append");
    f.sync_all().unwrap();
}

fn fsctl(f: &File, code: u32, input: &[u8]) {
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            f.as_raw_handle() as HANDLE,
            code,
            if input.is_empty() { std::ptr::null() } else { input.as_ptr().cast() },
            input.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    assert!(ok != 0, "FSCTL {code:#x} failed: {}", std::io::Error::last_os_error());
}

/// A sparse file of `logical` bytes with `data` bytes actually written at the start.
pub fn sparse_file(path: &Path, logical: u64, data: u64) {
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).unwrap();
    fsctl(&f, FSCTL_SET_SPARSE, &[]);
    f.set_len(logical).unwrap();
    f.write_all(&vec![0x5Au8; data as usize]).unwrap();
    f.sync_all().unwrap();
}

/// An NTFS-compressed file of `len` highly compressible bytes.
pub fn compressed_file(path: &Path, len: u64) {
    let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).unwrap();
    // COMPRESSION_FORMAT_DEFAULT
    fsctl(&f, FSCTL_SET_COMPRESSION, &1u16.to_le_bytes());
    let pattern: Vec<u8> = b"WinDevCleaner compressible text. ".iter().cycle().take(1024 * 1024).copied().collect();
    let mut left = len;
    while left > 0 {
        let n = left.min(pattern.len() as u64) as usize;
        f.write_all(&pattern[..n]).unwrap();
        left -= n as u64;
    }
    f.sync_all().unwrap();
}

/// Adds file attributes (for example `FILE_ATTRIBUTE_OFFLINE`).
pub fn add_attributes(path: &Path, attrs: u32) {
    let w = wide_path(path);
    let cur = unsafe { GetFileAttributesW(w.as_ptr()) };
    let ok = unsafe { SetFileAttributesW(w.as_ptr(), cur | attrs) };
    assert!(ok != 0, "SetFileAttributesW: {}", std::io::Error::last_os_error());
}

/// Denies listing a folder to Everyone (the elevated runner's backup privilege is disabled
/// by default, so this applies to the tests too).
pub fn deny_listing(dir: &Path) {
    let out = Command::new("icacls").arg(dir).args(["/deny", "*S-1-1-0:(RD)"]).output().unwrap();
    assert!(out.status.success(), "icacls: {}", String::from_utf8_lossy(&out.stdout));
}

pub fn allow_listing(dir: &Path) {
    let _ = Command::new("icacls").arg(dir).args(["/remove:d", "*S-1-1-0"]).output();
}

/// Opens a file with no sharing at all, as a program holding it exclusively would.
pub fn hold_exclusive(path: &Path) -> File {
    OpenOptions::new().read(true).write(true).share_mode(0).open(path).expect("open exclusively")
}

/// Size facts read from the file itself through a handle (the oracle for the scanner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleFacts {
    pub allocation: u64,
    pub length: u64,
    pub links: u32,
}

pub fn facts_by_handle(path: &Path) -> HandleFacts {
    // FILE_READ_ATTRIBUTES only, so it works on files held open exclusively.
    let f = OpenOptions::new()
        .access_mode(0x80)
        .share_mode(7)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
        .expect("open for attributes");
    let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            f.as_raw_handle() as HANDLE,
            FileStandardInfo,
            &mut info as *mut FILE_STANDARD_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    assert!(ok != 0);
    HandleFacts { allocation: info.AllocationSize as u64, length: info.EndOfFile as u64, links: info.NumberOfLinks }
}

/// Bytes allocated to a compressed or sparse file, as Windows reports them.
pub fn compressed_size(path: &Path) -> u64 {
    let w = wide_path(path);
    let mut high = 0u32;
    let low = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut high) };
    assert!(!(low == INVALID_FILE_SIZE && unsafe { GetLastError() } != 0));
    ((high as u64) << 32) | low as u64
}

/// The size this name's directory entry holds (`FindFirstFileW`), which can be stale for a
/// file with several names.
pub fn directory_entry_length(path: &Path) -> u64 {
    let w = wide_path(path);
    let mut d: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe { FindFirstFileW(w.as_ptr(), &mut d) };
    assert!(h as isize != -1, "FindFirstFileW failed");
    unsafe { FindClose(h) };
    ((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64
}

/// Cluster size of the volume holding `path`.
pub fn cluster_size(path: &Path) -> u64 {
    let abs = std::path::absolute(path).unwrap();
    let s = abs.to_string_lossy();
    let root = format!("{}\\", &s[..2]);
    let w = wide(OsStr::new(&root));
    let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    let ok = unsafe { GetDiskFreeSpaceW(w.as_ptr(), &mut spc, &mut bps, &mut free, &mut total) };
    assert!(ok != 0);
    spc as u64 * bps as u64
}

/// Measurements of one run of a program.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Measurement {
    pub wall_ms: f64,
    pub user_ms: f64,
    pub kernel_ms: f64,
    pub peak_working_set_bytes: u64,
    pub peak_private_bytes: u64,
    pub read_operations: u64,
    pub read_bytes: u64,
    pub other_operations: u64,
    /// I/O priority of the process sampled every few milliseconds while it ran (0 = very low).
    pub io_priority_samples: Vec<u32>,
    pub exit_code: Option<i32>,
    #[serde(skip)]
    pub stdout: String,
    #[serde(skip)]
    pub stderr: String,
}

fn ft_ms(ft: windows_sys::Win32::Foundation::FILETIME) -> f64 {
    (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) as f64 / 10_000.0
}

/// The I/O priority hint of a process, read with `NtQueryInformationProcess`.
pub fn io_priority(process: HANDLE) -> Option<u32> {
    use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
    let mut v = 0u32;
    let mut ret = 0u32;
    let st = unsafe { NtQueryInformationProcess(process, 33, &mut v as *mut u32 as *mut core::ffi::c_void, 4, &mut ret) };
    if st >= 0 {
        Some(v)
    } else {
        None
    }
}

/// Runs a program to completion, sampling its I/O priority while it runs, and reads its CPU
/// time, peak memory and I/O counters from Windows after it exits.
pub fn measure(command: &mut Command) -> Measurement {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command.spawn().expect("spawn measured program");
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let h = child.as_raw_handle() as HANDLE;
    let mut samples = Vec::new();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        // Skip the first 50 ms: the program enters background mode in its first lines.
        if start.elapsed() > Duration::from_millis(50) {
            if let Some(p) = io_priority(h) {
                samples.push(p);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut m = Measurement { wall_ms, io_priority_samples: samples, exit_code: status.code(), ..Default::default() };
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb) != 0 {
            m.peak_working_set_bytes = c.PeakWorkingSetSize as u64;
            m.peak_private_bytes = c.PeakPagefileUsage as u64;
        }
        let (mut a, mut b, mut k, mut u) = std::mem::zeroed();
        if GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u) != 0 {
            m.kernel_ms = ft_ms(k);
            m.user_ms = ft_ms(u);
        }
        let mut io: IO_COUNTERS = std::mem::zeroed();
        if GetProcessIoCounters(h, &mut io) != 0 {
            m.read_operations = io.ReadOperationCount;
            m.read_bytes = io.ReadTransferCount;
            m.other_operations = io.OtherOperationCount;
        }
    }
    m.stdout = t_out.join().unwrap();
    m.stderr = t_err.join().unwrap();
    m
}

/// CPU time used so far by a running process, in milliseconds.
pub fn cpu_ms(process: HANDLE) -> f64 {
    unsafe {
        let (mut a, mut b, mut k, mut u) = std::mem::zeroed();
        if GetProcessTimes(process, &mut a, &mut b, &mut k, &mut u) != 0 {
            ft_ms(k) + ft_ms(u)
        } else {
            0.0
        }
    }
}

/// Current and peak memory of a running process.
pub fn memory(process: HANDLE) -> (u64, u64, u64, u64) {
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(process, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        (c.WorkingSetSize as u64, c.PeakWorkingSetSize as u64, c.PrivateUsage as u64, c.PeakPagefileUsage as u64)
    }
}

/// Private working set of a running process: its own pages in RAM, the figure Task Manager
/// shows in its "Memory" column (`PROCESS_MEMORY_COUNTERS_EX2.PrivateWorkingSetSize`).
pub fn private_working_set(process: HANDLE) -> u64 {
    use windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX2;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX2 = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
        GetProcessMemoryInfo(process, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb);
        c.PrivateWorkingSetSize as u64
    }
}

/// Runs `f` on this thread while impersonating another local user (network logon).
pub fn as_other_user<T>(user: &str, password: &str, f: impl FnOnce() -> T) -> T {
    use windows_sys::Win32::Security::{
        ImpersonateLoggedOnUser, LogonUserW, RevertToSelf, LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT,
    };
    let u = wide(OsStr::new(user));
    let d = wide(OsStr::new("."));
    let p = wide(OsStr::new(password));
    let mut token: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        LogonUserW(u.as_ptr(), d.as_ptr(), p.as_ptr(), LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT, &mut token)
    };
    assert!(ok != 0, "LogonUserW({user}) failed: {}", std::io::Error::last_os_error());
    let ok = unsafe { ImpersonateLoggedOnUser(token) };
    assert!(ok != 0, "ImpersonateLoggedOnUser failed: {}", std::io::Error::last_os_error());
    let out = f();
    unsafe {
        RevertToSelf();
        CloseHandle(token);
    }
    out
}

/// Opens a pipe (or file) by name with the given access; returns the Windows error on failure.
pub fn try_open(name: &str, access: u32) -> Result<(), u32> {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
    let w = wide(OsStr::new(name));
    let h = unsafe { CreateFileW(w.as_ptr(), access, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h as isize == -1 {
        Err(unsafe { GetLastError() })
    } else {
        unsafe { CloseHandle(h) };
        Ok(())
    }
}

/// Path to a binary built in the same target folder as the running test.
pub fn target_bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    // target\<profile>\deps\test.exe -> target\<profile>\<name>.exe
    let dir = exe.parent().unwrap().parent().unwrap();
    let p = dir.join(format!("{name}.exe"));
    assert!(p.exists(), "{} is not built; run `cargo build` for the workspace first", p.display());
    p
}

/// Waits until a named pipe has a listening instance, without connecting to it.
pub fn wait_for_pipe(name: &str, timeout: Duration) -> bool {
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
    let w = wide(OsStr::new(name));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if unsafe { WaitNamedPipeW(w.as_ptr(), 200) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Sends a file to the Recycle Bin, as Explorer's Delete does (`SHFileOperationW` with undo).
pub fn send_to_recycle_bin(path: &Path) {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    // A list of NUL-terminated names ending with an extra NUL.
    let mut from: Vec<u16> = std::path::absolute(path).unwrap().as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
    let r = unsafe { SHFileOperationW(&mut op) };
    assert_eq!(r, 0, "SHFileOperationW failed: {r}");
    assert!(op.fAnyOperationsAborted == 0);
}

/// Length of a file in a volume's root as PowerShell lists it (`Get-ChildItem -Force`), or
/// None when the file does not exist. An oracle independent of the engine's API.
pub fn powershell_root_file_length(drive: char, name: &str) -> Option<u64> {
    let script = format!(
        "$f = Get-ChildItem -Force -LiteralPath '{drive}:\\' -Filter '{name}' -ErrorAction SilentlyContinue; if ($f) {{ $f.Length }}"
    );
    let out = Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}
