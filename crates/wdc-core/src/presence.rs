//! Proof that a person is present (plan, section 7), required by `apply`, `purge` and
//! `restore` on every face.
//!
//! - Windows Hello (PIN, face or fingerprint) through `UserConsentVerifier`, when it is set up;
//! - otherwise a random one-time code shown only in a desktop dialog, never printed to the
//!   console, which the person types into the console.
//!
//! An interactive console alone is not proof: agents can open real terminals. The app proves
//! presence with Windows Hello, or, where Hello is not set up, with a click on its approval
//! card, which an agent connected through MCP cannot make.
//!
//! Limits, stated plainly: this stops accidents and casual misuse. An agent that also controls
//! the screen could read the dialog; Windows Hello is the barrier for that case.

use std::time::Duration;

use crate::i18n::{Lang, Msg};

/// Title of the dialog that shows the code (fixed, in every language).
pub const DIALOG_TITLE: &str = "WinDevCleaner presence code";
/// How long the console waits for the code.
pub const CODE_TIMEOUT: Duration = Duration::from_secs(180);

/// How presence was proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    WindowsHello,
    DesktopCode,
    /// A click on the app's approval card (only where Windows Hello is not set up).
    AppApproval,
}

/// Proof that a person approved, now. Only a face with a person in front of it creates one:
/// `wdc.exe` after `console_proof`, the app after Windows Hello or a click on its card. The
/// MCP server never does.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Proof {
    pub method: Method,
    pub at_ms: u64,
}

impl Proof {
    pub fn new(method: Method) -> Proof {
        Proof { method, at_ms: crate::time::now_unix_ms() }
    }
}

/// Why presence was not proven.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum PresenceError {
    /// No desktop to show the dialog on (a service, a remote shell without a desktop).
    NoDesktop,
    Canceled,
    WrongCode,
    TimedOut,
    /// The console input ended without a code.
    NoInput,
    HelloFailed { detail: String },
    Unsupported,
}

impl PresenceError {
    pub fn text(&self, lang: Lang) -> String {
        match self {
            PresenceError::NoDesktop => lang.t(Msg::PresenceNoDesktop).into(),
            PresenceError::Canceled => lang.t(Msg::PresenceCanceled).into(),
            PresenceError::WrongCode => lang.t(Msg::PresenceWrongCode).into(),
            PresenceError::TimedOut => lang.t(Msg::PresenceTimedOut).into(),
            PresenceError::NoInput => lang.t(Msg::PresenceNoInput).into(),
            PresenceError::HelloFailed { detail } => lang.fmt(Msg::PresenceHelloFailed, &[("detail", detail)]),
            PresenceError::Unsupported => lang.t(Msg::PresenceUnsupported).into(),
        }
    }
}

/// Eight random digits from the system's cryptographic generator.
pub fn random_code() -> String {
    let mut b = [0u8; 8];
    platform::random(&mut b);
    b.iter().map(|x| char::from(b'0' + x % 10)).collect()
}

/// Compares without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether Windows Hello can verify the user on this machine.
pub fn hello_available() -> bool {
    if std::env::var_os("WDC_NO_HELLO").is_some() {
        return false;
    }
    platform::hello_available()
}

/// Asks Windows Hello, with its prompt owned by `hwnd`.
pub fn hello(hwnd: isize, message: &str) -> Result<Proof, PresenceError> {
    platform::hello(hwnd, message).map(|_| Proof::new(Method::WindowsHello))
}

enum Event {
    Line(String),
    NoInput,
    DialogFailed,
    DialogClosed,
}

/// Proof for `wdc.exe`: Windows Hello when set up, else the desktop code typed into the console.
#[allow(unused_mut, unused_assignments)]
pub fn console_proof(message: &str, lang: Lang) -> Result<Proof, PresenceError> {
    if hello_available() {
        eprintln!("{}", lang.t(Msg::PresenceHelloPrompt));
        return hello(platform::console_window(), message);
    }
    let code = random_code();
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let dialog = platform::show_code(DIALOG_TITLE, &lang.fmt(Msg::PresenceDialogText, &[("code", &code)]), {
        let tx = tx.clone();
        move |ok| {
            let _ = tx.send(if ok { Event::DialogClosed } else { Event::DialogFailed });
        }
    });
    eprintln!("{}", lang.t(Msg::PresenceTypeCode));
    std::thread::spawn(move || {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 => {
                let _ = tx.send(Event::Line(line));
            }
            _ => {
                let _ = tx.send(Event::NoInput);
            }
        }
    });
    let deadline = std::time::Instant::now() + CODE_TIMEOUT;
    let result = loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(Event::Line(l)) => {
                let typed: String = l.chars().filter(|c| !c.is_whitespace()).collect();
                let mut ok = false;
                // GUARD-BEGIN(presence-code)
                ok = same(&typed, &code);
                // GUARD-END(presence-code)
                break if ok { Ok(Proof::new(Method::DesktopCode)) } else { Err(PresenceError::WrongCode) };
            }
            Ok(Event::NoInput) => break Err(PresenceError::NoInput),
            Ok(Event::DialogFailed) => break Err(PresenceError::NoDesktop),
            // Closing the dialog does not cancel: the code may already be remembered.
            Ok(Event::DialogClosed) => continue,
            Err(_) => break Err(PresenceError::TimedOut),
        }
    };
    platform::close_dialog(dialog);
    result
}

#[cfg(windows)]
mod platform {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, MessageBoxW, PostMessageW, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, WM_CLOSE,
    };

    use super::PresenceError;

    pub fn random(buf: &mut [u8]) {
        let st = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        assert_eq!(st, 0, "the system random generator failed");
    }

    pub fn console_window() -> isize {
        unsafe { GetConsoleWindow() as isize }
    }

    /// Runs `f` on a new thread initialised for WinRT (multithreaded), so the caller's own COM
    /// state (the app's UI thread) is never touched.
    fn in_mta<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
            }
            f()
        })
        .join()
        .expect("Windows Hello thread")
    }

    pub fn hello_available() -> bool {
        use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability};
        in_mta(|| {
            UserConsentVerifier::CheckAvailabilityAsync()
                .and_then(|op| op.join())
                .map(|a| a == UserConsentVerifierAvailability::Available)
                .unwrap_or(false)
        })
    }

    pub fn hello(hwnd: isize, message: &str) -> Result<(), PresenceError> {
        use windows::core::HSTRING;
        use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
        let message = message.to_string();
        let r = in_mta(move || -> windows::core::Result<UserConsentVerificationResult> {
            let interop = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
            let op: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe {
                interop.RequestVerificationForWindowAsync(windows::Win32::Foundation::HWND(hwnd as *mut _), &HSTRING::from(message))?
            };
            op.join()
        });
        match r {
            Ok(v) if v == UserConsentVerificationResult::Verified => Ok(()),
            Ok(v) if v == UserConsentVerificationResult::Canceled => Err(PresenceError::Canceled),
            Ok(v) => Err(PresenceError::HelloFailed { detail: format!("result {}", v.0) }),
            Err(e) => Err(PresenceError::HelloFailed { detail: e.message().to_string() }),
        }
    }

    /// Shows the code in a message box on its own thread; returns that thread's id.
    pub fn show_code(title: &str, text: &str, done: impl FnOnce(bool) + Send + 'static) -> Arc<AtomicU32> {
        let tid = Arc::new(AtomicU32::new(0));
        let t = tid.clone();
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        std::thread::spawn(move || {
            t.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
            let r = unsafe {
                MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND)
            };
            done(r != 0);
        });
        tid
    }

    unsafe extern "system" fn close_one(hwnd: HWND, _: LPARAM) -> i32 {
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0 as WPARAM, 0) };
        1
    }

    pub fn close_dialog(tid: Arc<AtomicU32>) {
        let id = tid.load(Ordering::SeqCst);
        if id != 0 {
            unsafe { EnumThreadWindows(id, Some(close_one), 0) };
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io::Read;

    use super::PresenceError;

    pub fn random(buf: &mut [u8]) {
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(buf)).expect("random source");
    }

    pub fn console_window() -> isize {
        0
    }

    pub fn hello_available() -> bool {
        false
    }

    pub fn hello(_hwnd: isize, _message: &str) -> Result<(), PresenceError> {
        Err(PresenceError::Unsupported)
    }

    /// No desktop dialog outside Windows: presence cannot be proven.
    pub fn show_code(_title: &str, _text: &str, done: impl FnOnce(bool) + Send + 'static) -> u32 {
        done(false);
        0
    }

    pub fn close_dialog(_: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_eight_digits_and_differ() {
        let a = random_code();
        let b = random_code();
        assert_eq!(a.len(), 8);
        assert!(a.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(a, b);
        assert!(same("12345678", "12345678"));
        assert!(!same("12345678", "12345679"));
        assert!(!same("1234567", "12345678"));
    }
}
