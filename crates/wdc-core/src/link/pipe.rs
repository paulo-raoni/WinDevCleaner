//! The named pipe between `wdc mcp` and the app (plan, sections 4.6 and 6.5).
//!
//! The pipe's access list allows only the current Windows user (no inherited or default
//! entries), remote clients are rejected, the first instance must be ours (no squatting), and
//! the client checks that the server process runs as the same user before talking to it.

use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, FALSE, GENERIC_READ, GENERIC_WRITE, HANDLE, TRUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenProcess, OpenProcessToken, ResetEvent, WaitForSingleObject,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::LineConn;
use crate::win::{current_user_sid, last_error, token_user_sid, wide, OwnedHandle};

/// The pipe name for the current user: `\\.\pipe\WinDevCleaner-<SID>`. `WDC_PIPE_SUFFIX`
/// appends a suffix, so tests can run their own app instance without touching a real one.
pub fn pipe_name() -> io::Result<String> {
    pipe_name_with_suffix(&std::env::var("WDC_PIPE_SUFFIX").unwrap_or_default())
}

/// The pipe name for the current user with an explicit suffix.
pub fn pipe_name_with_suffix(suffix: &str) -> io::Result<String> {
    let sid = current_user_sid()?;
    let suffix: String = suffix.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    Ok(format!(r"\\.\pipe\WinDevCleaner-{sid}{suffix}"))
}

/// A security descriptor whose access list grants the current user, and nobody else, full
/// access. `D:P` makes the list protected: nothing is inherited or added by default.
struct OwnerOnly {
    sd: PSECURITY_DESCRIPTOR,
}

impl OwnerOnly {
    fn new() -> io::Result<OwnerOnly> {
        let sid = current_user_sid()?;
        let sddl = wide(std::ffi::OsStr::new(&format!("D:P(A;;GA;;;{sid})")));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnerOnly { sd })
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.sd);
        }
    }
}

/// Creates one listening instance of the pipe with the owner-only access list.
#[allow(unused_assignments, unused_mut)]
pub fn create_instance(name: &str, first: bool) -> io::Result<OwnedHandle> {
    let descriptor = OwnerOnly::new()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.sd,
        bInheritHandle: FALSE,
    };
    let mut sa_ptr: *const SECURITY_ATTRIBUTES = std::ptr::null();
    // GUARD-BEGIN(pipe-dacl)
    sa_ptr = &sa;
    // GUARD-END(pipe-dacl)
    let w = wide(std::ffi::OsStr::new(name));
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            w.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            sa_ptr,
        )
    };
    drop(descriptor);
    OwnedHandle::new(h).ok_or_else(io::Error::last_os_error)
}

fn event() -> io::Result<OwnedHandle> {
    let h = unsafe { CreateEventW(std::ptr::null(), TRUE, FALSE, std::ptr::null()) };
    OwnedHandle::new(h).ok_or_else(io::Error::last_os_error)
}

/// Waits for a client on a listening instance.
pub fn accept(instance: OwnedHandle) -> io::Result<PipeConn> {
    let ev = event()?;
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.raw();
    let ok = unsafe { ConnectNamedPipe(instance.raw(), &mut ov) };
    if ok == 0 {
        match unsafe { GetLastError() } {
            ERROR_PIPE_CONNECTED => {}
            ERROR_IO_PENDING => {
                let mut n = 0u32;
                if unsafe { GetOverlappedResult(instance.raw(), &ov, &mut n, TRUE) } == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            e => return Err(io::Error::from_raw_os_error(e as i32)),
        }
    }
    PipeConn::new(instance, true)
}

/// One end of a connected pipe. Reads are overlapped, so a waiting read never blocks a write
/// and a disconnect from the window is noticed within the poll interval.
pub struct PipeConn {
    h: OwnedHandle,
    read_ev: OwnedHandle,
    write_ev: OwnedHandle,
    read_ov: Box<OVERLAPPED>,
    buf: Box<[u8; 64 * 1024]>,
    pending: bool,
    acc: Vec<u8>,
    server: bool,
}

// SAFETY: the connection is used by one thread at a time; the handles are kernel objects.
unsafe impl Send for PipeConn {}

impl PipeConn {
    fn new(h: OwnedHandle, server: bool) -> io::Result<PipeConn> {
        Ok(PipeConn {
            h,
            read_ev: event()?,
            write_ev: event()?,
            read_ov: Box::new(unsafe { std::mem::zeroed() }),
            buf: Box::new([0u8; 64 * 1024]),
            pending: false,
            acc: Vec::new(),
            server,
        })
    }

    fn take_line(&mut self) -> Option<String> {
        let pos = self.acc.iter().position(|&b| b == b'\n')?;
        let line: Vec<u8> = self.acc.drain(..=pos).collect();
        Some(String::from_utf8_lossy(&line).trim_end().to_string())
    }
}

fn pipe_gone(e: u32) -> bool {
    e == ERROR_BROKEN_PIPE || e == ERROR_PIPE_NOT_CONNECTED
}

impl LineConn for PipeConn {
    fn poll_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        if let Some(l) = self.take_line() {
            return Ok(Some(l));
        }
        if self.acc.len() > 16 * 1024 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
        if !self.pending {
            *self.read_ov = unsafe { std::mem::zeroed() };
            self.read_ov.hEvent = self.read_ev.raw();
            unsafe { ResetEvent(self.read_ev.raw()) };
            let ok = unsafe {
                ReadFile(self.h.raw(), self.buf.as_mut_ptr(), self.buf.len() as u32, std::ptr::null_mut(), &mut *self.read_ov)
            };
            if ok == 0 {
                let e = last_error();
                if e != ERROR_IO_PENDING {
                    return Err(io::Error::from_raw_os_error(e as i32));
                }
            }
            self.pending = true;
        }
        let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
        match unsafe { WaitForSingleObject(self.read_ev.raw(), ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(None),
            _ => return Err(io::Error::last_os_error()),
        }
        let mut n = 0u32;
        let ok = unsafe { GetOverlappedResult(self.h.raw(), &*self.read_ov, &mut n, FALSE) };
        self.pending = false;
        if ok == 0 {
            let e = last_error();
            return Err(if pipe_gone(e) {
                io::Error::new(io::ErrorKind::UnexpectedEof, "the other end closed the pipe")
            } else {
                io::Error::from_raw_os_error(e as i32)
            });
        }
        if n == 0 {
            return Ok(None);
        }
        self.acc.extend_from_slice(&self.buf[..n as usize]);
        Ok(self.take_line())
    }

    fn send_line(&mut self, line: &str) -> io::Result<()> {
        let mut data = line.as_bytes().to_vec();
        data.push(b'\n');
        let mut written = 0usize;
        while written < data.len() {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.write_ev.raw();
            unsafe { ResetEvent(self.write_ev.raw()) };
            let chunk = &data[written..];
            let ok = unsafe { WriteFile(self.h.raw(), chunk.as_ptr(), chunk.len() as u32, std::ptr::null_mut(), &mut ov) };
            if ok == 0 && last_error() != ERROR_IO_PENDING {
                return Err(io::Error::last_os_error());
            }
            let mut n = 0u32;
            if unsafe { GetOverlappedResult(self.h.raw(), &ov, &mut n, TRUE) } == 0 {
                return Err(io::Error::last_os_error());
            }
            written += n as usize;
        }
        Ok(())
    }
}

impl Drop for PipeConn {
    fn drop(&mut self) {
        unsafe {
            if self.pending {
                CancelIoEx(self.h.raw(), &*self.read_ov);
                let mut n = 0u32;
                // Wait for the cancelled read before its buffer and OVERLAPPED are freed.
                GetOverlappedResult(self.h.raw(), &*self.read_ov, &mut n, TRUE);
            }
            if self.server {
                FlushFileBuffers(self.h.raw());
                DisconnectNamedPipe(self.h.raw());
            }
        }
    }
}

/// Outcome of trying to reach the app.
pub enum Connect {
    Connected(PipeConn),
    /// No app is listening (the app is closed).
    NotRunning,
}

/// Connects to the app's pipe. Fails if the pipe's server is not the current user.
pub fn connect(name: &str) -> io::Result<Connect> {
    let w = wide(std::ffi::OsStr::new(name));
    let mut tries = 0;
    let h = loop {
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                // Overlapped, so a waiting read never blocks a write; and the app may only
                // identify us, never act as us.
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if let Some(h) = OwnedHandle::new(h) {
            break h;
        }
        match last_error() {
            2 | 3 => return Ok(Connect::NotRunning), // no such pipe: the app is closed
            231 if tries < 50 => {
                // ERROR_PIPE_BUSY: the app is between two listening instances.
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            e => return Err(io::Error::from_raw_os_error(e as i32)),
        }
    };
    check_server_is_same_user(&h)?;
    Ok(Connect::Connected(PipeConn::new(h, false)?))
}

/// Refuses a pipe created by another user (a squatter impersonating the app).
fn check_server_is_same_user(h: &OwnedHandle) -> io::Result<()> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(h.raw(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let p = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
    let p = OwnedHandle::new(p).ok_or_else(io::Error::last_os_error)?;
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(p.raw(), windows_sys::Win32::Security::TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);
    let server_sid = unsafe { token_user_sid(token.raw()) }?;
    if server_sid != current_user_sid()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the WinDevCleaner pipe belongs to another user; refusing to use it",
        ));
    }
    Ok(())
}

/// Runs the app's accept loop: one listening instance at a time, each connected client
/// served on its own thread. Returns an error only if the first instance cannot be created
/// (another app instance already owns the pipe).
pub fn listen(
    control: std::sync::Arc<super::LinkControl>,
    handler: std::sync::Arc<dyn super::AgentHandler>,
) -> io::Result<std::thread::JoinHandle<()>> {
    let name = pipe_name()?;
    let first = create_instance(&name, true)?;
    Ok(std::thread::Builder::new().name("agent-link".into()).spawn(move || {
        let mut next = Some(first);
        loop {
            let instance = match next.take() {
                Some(i) => i,
                None => match create_instance(&name, false) {
                    Ok(i) => i,
                    Err(_) => {
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                },
            };
            match accept(instance) {
                Ok(mut conn) => {
                    // Keep a listening instance while this client is served, so a second
                    // agent gets a clear refusal instead of "app closed".
                    next = create_instance(&name, false).ok();
                    let (c, h) = (control.clone(), handler.clone());
                    let _ = std::thread::Builder::new().name("agent-session".into()).spawn(move || {
                        super::serve_session(&mut conn, &c, h.as_ref());
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    })?)
}

