//! Background mode: lower CPU, I/O and memory priority while the tool works (plan, 4.1).

/// What background mode the current process or thread is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BackgroundState {
    /// The process (or, in the app, the scanning thread) entered background mode.
    pub entered: bool,
    /// The process I/O priority hint read back from Windows (0 = very low), when available.
    pub io_priority: Option<u32>,
    /// The process memory priority read back from Windows (1 = very low), when available.
    pub memory_priority: Option<u32>,
}

/// Puts the whole current process in background mode (`PROCESS_MODE_BACKGROUND_BEGIN`).
/// Used by `wdc.exe`, which is a background tool from start to end.
#[allow(unused_assignments, unused_mut)]
pub fn enter_process_background_mode() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, PROCESS_MODE_BACKGROUND_BEGIN,
        };
        let mut ok = false;
        // GUARD-BEGIN(background-mode)
        ok = unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN) } != 0;
        // GUARD-END(background-mode)
        ok
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Reads back the current process' background state from Windows.
pub fn process_state(entered: bool) -> BackgroundState {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let me = unsafe { GetCurrentProcess() };
        BackgroundState {
            entered,
            io_priority: crate::win::process_io_priority(me),
            memory_priority: crate::win::process_memory_priority(me),
        }
    }
    #[cfg(not(windows))]
    {
        BackgroundState { entered, io_priority: None, memory_priority: None }
    }
}

/// Background mode for the calling thread only, ended on drop. The desktop app scans on a
/// worker thread this way, so its window stays responsive while the scan stays gentle.
pub struct ThreadBackground {
    #[allow(dead_code)]
    entered: bool,
}

impl ThreadBackground {
    pub fn enter() -> ThreadBackground {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
            };
            let entered =
                unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } != 0;
            ThreadBackground { entered }
        }
        #[cfg(not(windows))]
        {
            ThreadBackground { entered: false }
        }
    }

    pub fn entered(&self) -> bool {
        self.entered
    }
}

impl Drop for ThreadBackground {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.entered {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_END,
            };
            unsafe {
                SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END);
            }
        }
    }
}
