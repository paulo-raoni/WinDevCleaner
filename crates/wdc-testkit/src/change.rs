//! Helpers for the tests of slice 4 (changing the disk): a small VHDX volume made by the test,
//! 8.3 short names, the presence dialog read the way a screen-reading agent would, and
//! commands run inside a pseudo-console (ConPTY), as an agent opening a real terminal would.

// Handles are opaque kernel object references passed to Windows, never dereferenced here.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};

use crate::imp::wide;

fn diskpart(script: &str) -> Result<String, String> {
    let file = std::env::temp_dir().join(format!("wdc-test-diskpart-{}-{:?}.txt", std::process::id(), std::thread::current().id()));
    std::fs::write(&file, script).map_err(|e| e.to_string())?;
    let out = Command::new("diskpart").arg("/s").arg(&file).output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&file);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Serialises volume creation inside one test process (letters are chosen while it is held).
static VOLUMES: Mutex<()> = Mutex::new(());

/// A small NTFS volume on a VHDX file, attached for one test and detached when dropped.
/// Needs administrator rights (the CI runner has them). 8.3 short names are on.
pub struct Vhdx {
    pub root: PathBuf,
    pub letter: char,
    file: PathBuf,
}

impl Vhdx {
    pub fn create(name: &str, megabytes: u32) -> Vhdx {
        let _one = VOLUMES.lock().unwrap_or_else(|e| e.into_inner());
        let letter = ('P'..='Z').rev().find(|c| !Path::new(&format!("{c}:\\")).exists()).expect("a free drive letter");
        let file = std::env::temp_dir().join(format!("wdc-test-{name}-{}.vhdx", std::process::id()));
        if file.exists() {
            let _ = diskpart(&format!("select vdisk file=\"{}\"\ndetach vdisk noerr\n", file.display()));
            let _ = std::fs::remove_file(&file);
        }
        diskpart(&format!(
            "create vdisk file=\"{f}\" maximum={megabytes} type=expandable\nselect vdisk file=\"{f}\"\nattach vdisk\n\
             create partition primary\nformat fs=ntfs quick label=WDCTEST\nassign letter={letter}\n",
            f = file.display()
        ))
        .unwrap_or_else(|e| panic!("diskpart could not make the test volume: {e}"));
        let root = PathBuf::from(format!("{letter}:\\"));
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(root.exists(), "volume {letter}: did not appear");
        // Short names on this volume (the system setting must allow per-volume choice).
        let _ = Command::new("fsutil").args(["8dot3name", "set", "2"]).output();
        let out = Command::new("fsutil").args(["8dot3name", "set", &format!("{letter}:"), "0"]).output().unwrap();
        assert!(out.status.success(), "fsutil 8dot3name: {}", String::from_utf8_lossy(&out.stdout));
        Vhdx { root, letter, file }
    }

    /// Bytes free on the volume, read with an API the engine's measurement does not share.
    pub fn free_bytes(&self) -> u64 {
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("(Get-Volume -DriveLetter {}).SizeRemaining", self.letter)])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().expect("Get-Volume SizeRemaining")
    }
}

impl Drop for Vhdx {
    fn drop(&mut self) {
        let _ = diskpart(&format!("select vdisk file=\"{}\"\ndetach vdisk noerr\n", self.file.display()));
        let _ = std::fs::remove_file(&self.file);
    }
}

/// The 8.3 short form of an existing path (`GetShortPathNameW`).
pub fn short_path(p: &Path) -> PathBuf {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let w = wide(p.as_os_str());
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetShortPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    assert!(n > 0 && (n as usize) < buf.len(), "GetShortPathNameW failed: {}", std::io::Error::last_os_error());
    PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]))
}

struct Search {
    pid: u32,
    title: String,
    found: Vec<HWND>,
}

unsafe extern "system" fn top_window(hwnd: HWND, lp: LPARAM) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};
    let s = unsafe { &mut *(lp as *mut Search) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == s.pid {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == s.title {
            s.found.push(hwnd);
        }
    }
    1
}

unsafe extern "system" fn child_text(hwnd: HWND, lp: LPARAM) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETTEXT};
    let texts = unsafe { &mut *(lp as *mut Vec<String>) };
    let mut buf = vec![0u16; 2048];
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(hwnd, WM_GETTEXT, buf.len(), buf.as_mut_ptr() as LPARAM, SMTO_ABORTIFHUNG, 1000, &mut result)
    };
    if ok != 0 && result > 0 {
        texts.push(String::from_utf16_lossy(&buf[..result.min(buf.len())]));
    }
    1
}

/// Reads the one-time code from the presence dialog of process `pid`, as an agent that can read
/// the screen could. Waits for the dialog up to `timeout`.
pub fn read_presence_code(pid: u32, title: &str, timeout: Duration) -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumChildWindows, EnumWindows};
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let mut s = Search { pid, title: title.to_string(), found: Vec::new() };
        unsafe { EnumWindows(Some(top_window), &mut s as *mut Search as LPARAM) };
        for hwnd in s.found {
            let mut texts: Vec<String> = Vec::new();
            unsafe { EnumChildWindows(hwnd, Some(child_text), &mut texts as *mut Vec<String> as LPARAM) };
            for t in texts {
                let digits: Vec<char> = t.chars().collect();
                for w in digits.windows(8) {
                    if w.iter().all(|c| c.is_ascii_digit()) {
                        return Some(w.iter().collect());
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Whether process `pid` shows a window with this title now.
pub fn has_window(pid: u32, title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    let mut s = Search { pid, title: title.to_string(), found: Vec::new() };
    unsafe { EnumWindows(Some(top_window), &mut s as *mut Search as LPARAM) };
    !s.found.is_empty()
}

/// What a command printed in a pseudo-console, and how it ended.
pub struct ConsoleRun {
    pub exit_code: Option<u32>,
    pub output: String,
}

fn env_block(overrides: &[(&str, &OsStr)]) -> Vec<u16> {
    let mut vars: Vec<(String, std::ffi::OsString)> =
        std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v)).collect();
    for (k, v) in overrides {
        vars.retain(|(x, _)| !x.eq_ignore_ascii_case(k));
        vars.push((k.to_string(), v.to_os_string()));
    }
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block = Vec::new();
    for (k, v) in vars {
        block.extend(OsStr::new(&k).encode_wide());
        block.push(b'=' as u16);
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

/// Runs `exe args` in a new pseudo-console (a real interactive console, as Windows Terminal
/// gives). When the output shows `prompt`, types `answer` and Enter. Waits up to `timeout`.
pub fn run_in_pseudo_console(
    exe: &Path,
    args: &[&str],
    env: &[(&str, &OsStr)],
    prompt: &str,
    answer: &str,
    timeout: Duration,
) -> ConsoleRun {
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON};
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
        TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW,
    };

    let (mut in_read, mut in_write, mut out_read, mut out_write): (HANDLE, HANDLE, HANDLE, HANDLE) =
        (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    unsafe {
        assert!(CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) != 0);
        assert!(CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) != 0);
    }
    let mut hpc: HPCON = 0;
    let hr = unsafe { CreatePseudoConsole(COORD { X: 160, Y: 50 }, in_read, out_write, 0, &mut hpc) };
    assert_eq!(hr, 0, "CreatePseudoConsole failed: {hr:#x}");
    unsafe {
        CloseHandle(in_read);
        CloseHandle(out_write);
    }

    let mut size = 0usize;
    unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
    let mut attrs = vec![0u8; size];
    let list = attrs.as_mut_ptr() as *mut core::ffi::c_void;
    unsafe {
        assert!(InitializeProcThreadAttributeList(list, 1, 0, &mut size) != 0);
        assert!(
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const core::ffi::c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) != 0
        );
    }
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    // Null standard handles: otherwise the child uses the test runner's redirected ones
    // instead of the pseudo-console.
    si.StartupInfo.dwFlags = windows_sys::Win32::System::Threading::STARTF_USESTDHANDLES;
    si.lpAttributeList = list;
    let mut cmdline: Vec<u16> = {
        let mut s = format!("\"{}\"", exe.display());
        for a in args {
            s.push_str(&format!(" \"{a}\""));
        }
        OsStr::new(&s).encode_wide().chain([0]).collect()
    };
    let envb = env_block(env);
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            envb.as_ptr() as *const core::ffi::c_void,
            std::ptr::null(),
            &si.StartupInfo,
            &mut pi,
        )
    };
    assert!(ok != 0, "CreateProcessW in a pseudo-console failed: {}", std::io::Error::last_os_error());

    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let reader = {
        let output = output.clone();
        let h = out_read as usize;
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let mut n = 0u32;
                let ok = unsafe { ReadFile(h as HANDLE, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) };
                if ok == 0 || n == 0 {
                    break;
                }
                output.lock().unwrap().extend_from_slice(&buf[..n as usize]);
            }
        })
    };

    let deadline = Instant::now() + timeout;
    let mut answered = false;
    let mut exit_code = None;
    loop {
        if !answered && String::from_utf8_lossy(&output.lock().unwrap()).contains(prompt) {
            let line = format!("{answer}\r");
            let mut n = 0u32;
            unsafe { WriteFile(in_write, line.as_ptr(), line.len() as u32, &mut n, std::ptr::null_mut()) };
            answered = true;
        }
        if unsafe { WaitForSingleObject(pi.hProcess, 100) } == WAIT_OBJECT_0 {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(pi.hProcess, &mut code) };
            exit_code = Some(code);
            break;
        }
        if Instant::now() >= deadline {
            unsafe { TerminateProcess(pi.hProcess, 99) };
            break;
        }
    }
    unsafe {
        ClosePseudoConsole(hpc);
        CloseHandle(in_write);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        DeleteProcThreadAttributeList(list);
    }
    let _ = reader.join();
    unsafe { CloseHandle(out_read) };
    let text = String::from_utf8_lossy(&output.lock().unwrap()).into_owned();
    ConsoleRun { exit_code, output: text }
}
