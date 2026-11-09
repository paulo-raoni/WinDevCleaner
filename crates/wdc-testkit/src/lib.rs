//! Test fixtures and measurement helpers for WinDevCleaner's Windows CI. Not shipped.
//!
//! Fixtures are built with Windows' own tools or calls (`mklink`, `icacls`, FSCTLs), and
//! expected values are read through APIs other than the scanner's directory enumeration, so a
//! test never checks the scanner against itself.

#[cfg(windows)]
mod change;
#[cfg(windows)]
mod imp;
#[cfg(windows)]
pub use change::*;
#[cfg(windows)]
pub use imp::*;

/// A fresh, empty folder for one test: `<WDC_TEST_ROOT or %TEMP%>\wdc-tests\<name>-<pid>`.
pub fn fresh_dir(name: &str) -> std::path::PathBuf {
    let base = std::env::var_os("WDC_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("wdc-tests");
    let p = base.join(format!("{name}-{}", std::process::id()));
    if p.exists() {
        remove_tree(&p);
    }
    std::fs::create_dir_all(&p).expect("create test folder");
    p
}

/// Removes a test folder without following links inside it (std's remove_dir_all removes
/// junctions and symbolic links themselves, never their targets).
pub fn remove_tree(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// A guarded assertion. Its failure message starts with `GUARD-CHECK failed`, which the guard
/// check (ci/guard-check.sh) looks for to confirm a test failed for the guarded reason, not
/// for an unrelated one. The label names the guard(s) the assertion depends on.
#[macro_export]
macro_rules! guard_assert {
    ($guard:literal, $cond:expr, $($arg:tt)+) => {
        if !$cond {
            panic!("GUARD-CHECK failed [{}]: {}", $guard, format!($($arg)+));
        }
    };
}

#[macro_export]
macro_rules! guard_assert_eq {
    ($guard:literal, $left:expr, $right:expr, $($arg:tt)+) => {
        {
            let (l, r) = (&$left, &$right);
            if l != r {
                panic!("GUARD-CHECK failed [{}]: {:?} != {:?}: {}", $guard, l, r, format!($($arg)+));
            }
        }
    };
}

/// A minimal MCP client over stdio, written from the protocol (JSON-RPC 2.0, one message per
/// line), independent of the server's SDK, so tests check the real wire format.
pub struct McpClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// Starts `wdc mcp` with extra environment variables.
    pub fn start(wdc: &std::path::Path, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> McpClient {
        let mut cmd = std::process::Command::new(wdc);
        cmd.arg("mcp").args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("start wdc mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        McpClient { child, stdin, stdout, next_id: 1 }
    }

    fn send(&mut self, v: &serde_json::Value) {
        use std::io::Write;
        writeln!(self.stdin, "{v}").expect("write to wdc mcp");
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns its `result` (panics on a JSON-RPC error).
    pub fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        use std::io::BufRead;
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read from wdc mcp");
            assert!(n > 0, "wdc mcp closed stdout");
            let v: serde_json::Value = serde_json::from_str(&line).expect("JSON-RPC message");
            if v.get("id") == Some(&serde_json::json!(id)) {
                if let Some(e) = v.get("error") {
                    panic!("{method} failed: {e}");
                }
                return v["result"].clone();
            }
        }
    }

    pub fn notify(&mut self, method: &str) {
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "method": method }));
    }

    /// `initialize` + `notifications/initialized`, as a client named `name`.
    pub fn initialize(&mut self, name: &str) -> serde_json::Value {
        let r = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": name, "version": "1.0.0" }
            }),
        );
        self.notify("notifications/initialized");
        r
    }

    /// `tools/call`; returns the result object (with `isError` and `structuredContent`).
    pub fn call(&mut self, tool: &str, args: serde_json::Value) -> serde_json::Value {
        self.request("tools/call", serde_json::json!({ "name": tool, "arguments": args }))
    }

    /// Closes stdin and waits for the server to exit.
    pub fn close(mut self) -> Option<i32> {
        drop(self.stdin);
        self.child.wait().ok().and_then(|s| s.code())
    }
}
