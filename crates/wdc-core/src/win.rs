//! Thin, safe wrappers over the Windows calls the engine needs (`windows-sys`).

// Handles are opaque kernel object references passed to Windows, never dereferenced here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// An owned Windows handle, closed on drop.
pub struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    /// Wraps a handle returned by `CreateFileW` and similar; `None` when it is invalid.
    pub fn new(h: HANDLE) -> Option<OwnedHandle> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(OwnedHandle(h))
        }
    }
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// SAFETY: a kernel handle can be used from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

/// The calling thread's last Windows error code.
pub fn last_error() -> u32 {
    unsafe { GetLastError() }
}

/// A NUL-terminated UTF-16 copy of `s`.
pub fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// A NUL-terminated UTF-16 copy of a path.
pub fn wide_path(p: &Path) -> Vec<u16> {
    wide(p.as_os_str())
}

/// Turns UTF-16 without a terminator into a `String` (lossy for unpaired surrogates).
pub fn from_wide(w: &[u16]) -> String {
    OsString::from_wide(w).to_string_lossy().into_owned()
}

/// Makes an absolute path in the `\\?\` form, which lifts the 260-character limit and stops
/// Windows from reinterpreting the text. Links are not resolved (unlike `canonicalize`).
pub fn verbatim(path: &Path) -> std::io::Result<Vec<u16>> {
    let abs = std::path::absolute(path)?;
    let w: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let s = String::from_utf16_lossy(&w);
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
    // A trailing separator would make the last name empty; keep `C:\` intact.
    while out.len() > 7 && out.last() == Some(&(b'\\' as u16)) {
        out.pop();
    }
    if out.len() == 6 {
        // `\\?\C:` must stay `\\?\C:\` to name the volume's root folder.
        out.push(b'\\' as u16);
    }
    Ok(out)
}

/// Removes the `\\?\` prefix for display (`\\?\UNC\srv\x` becomes `\\srv\x`).
pub fn display_path(w: &[u16]) -> String {
    let s = from_wide(w);
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// `PathBuf` from a verbatim UTF-16 path (without terminator).
pub fn pathbuf(w: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(w))
}

/// The current user's SID as a string (`S-1-5-21-...`).
pub fn current_user_sid() -> std::io::Result<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let token = OwnedHandle(token);
        token_user_sid(token.raw())
    }
}

/// The user SID of a token, as a string.
///
/// # Safety
/// `token` must be a valid token handle opened with `TOKEN_QUERY`.
pub unsafe fn token_user_sid(token: HANDLE) -> std::io::Result<String> {
    let mut len = 0u32;
    GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
    if len == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let user = &*(buf.as_ptr() as *const TOKEN_USER);
    let mut s: *mut u16 = std::ptr::null_mut();
    if ConvertSidToStringSidW(user.User.Sid, &mut s) == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    let out = from_wide(std::slice::from_raw_parts(s, n));
    LocalFree(s.cast());
    Ok(out)
}

/// Memory figures of a process, in bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryInfo {
    pub working_set: u64,
    pub peak_working_set: u64,
    /// Private committed bytes ("private bytes" in Task Manager's details).
    pub private_bytes: u64,
    pub peak_private_bytes: u64,
}

/// Memory figures of a process handle (`GetProcessMemoryInfo`).
pub fn process_memory(process: HANDLE) -> Option<MemoryInfo> {
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            process,
            &mut c as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            c.cb,
        ) == 0
        {
            return None;
        }
        Some(MemoryInfo {
            working_set: c.WorkingSetSize as u64,
            peak_working_set: c.PeakWorkingSetSize as u64,
            private_bytes: c.PrivateUsage as u64,
            peak_private_bytes: c.PeakPagefileUsage as u64,
        })
    }
}

/// Memory figures of the current process.
pub fn own_memory() -> Option<MemoryInfo> {
    process_memory(unsafe { GetCurrentProcess() })
}

/// The I/O priority hint of a process (0 very low, 1 low, 2 normal, 3 high).
/// Background mode sets it to 0.
pub fn process_io_priority(process: HANDLE) -> Option<u32> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, PROCESSINFOCLASS};
    // ProcessIoPriority; not exported as a named constant by windows-sys.
    const PROCESS_IO_PRIORITY: PROCESSINFOCLASS = 33;
    let mut value: u32 = 0;
    let mut ret = 0u32;
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            PROCESS_IO_PRIORITY,
            &mut value as *mut u32 as *mut core::ffi::c_void,
            4,
            &mut ret,
        )
    };
    if status >= 0 {
        Some(value)
    } else {
        None
    }
}

/// The memory (page) priority of a process (1 very low ... 5 normal).
pub fn process_memory_priority(process: HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Threading::{
        GetProcessInformation, ProcessMemoryPriority, MEMORY_PRIORITY_INFORMATION,
    };
    let mut info = MEMORY_PRIORITY_INFORMATION { MemoryPriority: 0 };
    let ok = unsafe {
        GetProcessInformation(
            process,
            ProcessMemoryPriority,
            &mut info as *mut MEMORY_PRIORITY_INFORMATION as *mut core::ffi::c_void,
            std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32,
        )
    };
    if ok != 0 {
        Some(info.MemoryPriority)
    } else {
        None
    }
}

/// True when the Windows display language is Portuguese.
pub fn ui_language_is_portuguese() -> bool {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    const LANG_PORTUGUESE: u16 = 0x16;
    let id = unsafe { GetUserDefaultUILanguage() };
    id & 0x3ff == LANG_PORTUGUESE
}

/// Local time of day (hour, minute, second).
pub fn local_time() -> (u16, u16, u16) {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    (t.wHour, t.wMinute, t.wSecond)
}

/// A known folder path (Documents, Downloads, ...), when it can be resolved.
pub fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p);
        if hr < 0 || p.is_null() {
            if !p.is_null() {
                CoTaskMemFree(p.cast());
            }
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let out = OsString::from_wide(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p.cast());
        Some(PathBuf::from(out))
    }
}
