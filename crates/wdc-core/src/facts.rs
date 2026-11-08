//! Facts about the machine that rule conditions read: running programs and the installed
//! NVIDIA display driver. Read-only.

use std::collections::HashSet;

/// Executable names of running processes, lower-case (`code.exe`). None if they cannot be read.
pub fn running_programs() -> Option<HashSet<String>> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        };
        let snap = crate::win::OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
        let mut out = HashSet::new();
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap.raw(), &mut e) } != 0;
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.insert(String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase());
            ok = unsafe { Process32NextW(snap.raw(), &mut e) } != 0;
        }
        Some(out)
    }
    #[cfg(not(windows))]
    {
        // Development hosts: /proc/*/comm, close enough for the platform-independent tests.
        let mut out = HashSet::new();
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            if let Ok(c) = std::fs::read_to_string(e.path().join("comm")) {
                out.insert(c.trim().to_lowercase());
            }
        }
        Some(out)
    }
}

/// The installed NVIDIA display driver version in Windows' form (`32.0.15.6094`), from the
/// display adapters' class key, which standard users can read.
pub fn nvidia_driver_version() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
        const CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
        let read = |sub: &str, value: &str| -> Option<String> {
            let key: Vec<u16> = format!("{CLASS}\\{sub}").encode_utf16().chain(std::iter::once(0)).collect();
            let val: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let mut buf = vec![0u16; 256];
            let mut len = (buf.len() * 2) as u32;
            let r = unsafe {
                RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    key.as_ptr(),
                    val.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut len,
                )
            };
            if r != 0 {
                return None;
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]))
        };
        for i in 0..32 {
            let sub = format!("{i:04}");
            if read(&sub, "ProviderName").map(|p| p.to_ascii_lowercase().contains("nvidia")).unwrap_or(false) {
                if let Some(v) = read(&sub, "DriverVersion") {
                    return Some(v);
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        None
    }
}
