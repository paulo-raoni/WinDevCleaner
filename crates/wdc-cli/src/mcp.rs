//! `wdc mcp`: MCP server on stdio (plan, section 4.6).
//!
//! When the WinDevCleaner app is open, every call goes through the app over the per-user
//! named pipe, so the human sees it live and can pause or disconnect the agent. When the app
//! is closed, the engine runs headless here (`--show` opens the app first). The mode is chosen
//! when the client initializes, and a headless server tries the app's pipe again before each
//! call: when the app has been opened since, it switches to the app (and to blocked if the app
//! refuses it). It only ever moves from headless towards the app: an agent the human
//! disconnected gets errors from then on, and a server attached to the app never falls back
//! to headless. Every tool result says which of the three it is (`app`).

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;

use wdc_core::link::{CallOutcome, Client, LineConn};
use wdc_core::scan::ScanReport;
use wdc_core::tools;

type AppClient = Client<Box<dyn LineConn + Send>>;

/// Opens the app's pipe and says hello as the given client; `Ok(None)` when the app is closed.
type Probe<'a> = &'a dyn Fn(&str) -> Result<Option<AppClient>, String>;

enum Backend {
    /// Not initialized yet.
    Pending,
    /// The app is open: calls go through it. `upgraded` is set when the server started
    /// headless and switched to the app later, until a `scan` runs in the app.
    App { client: Box<AppClient>, upgraded: bool },
    /// The app is closed: the engine runs here. Keeps the latest scan for `tree`, and the
    /// client name for the day the app opens.
    Headless { last: Option<Box<ScanReport>>, client: String },
    /// The app refused or dropped this agent; every call fails with the reason.
    Blocked(String),
}

/// What a result says about the app: `app` (and `app_reason` when blocked).
struct AppMode {
    label: &'static str,
    reason: Option<String>,
}

impl Backend {
    fn mode(&self) -> AppMode {
        match self {
            Backend::App { .. } => AppMode { label: "connected", reason: None },
            Backend::Blocked(reason) => AppMode { label: "blocked", reason: Some(reason.clone()) },
            // Before initialize nothing has run; headless is what a call would find.
            Backend::Headless { .. } | Backend::Pending => AppMode { label: "headless", reason: None },
        }
    }
}

/// Puts `app` (and `app_reason`) at the top level of a result; a result that is not an
/// object is wrapped under `result`.
fn stamp(v: Value, mode: &AppMode) -> Value {
    let mut obj = match v {
        Value::Object(m) => m,
        other => {
            let mut m = serde_json::Map::new();
            m.insert("result".into(), other);
            m
        }
    };
    // GUARD-BEGIN(mcp-app-field)
    obj.insert("app".into(), Value::String(mode.label.into()));
    // GUARD-END(mcp-app-field)
    if let Some(reason) = &mode.reason {
        obj.insert("app_reason".into(), Value::String(reason.clone()));
    }
    Value::Object(obj)
}

struct WdcServer {
    show: bool,
    backend: Arc<Mutex<Backend>>,
}

const INSTRUCTIONS: &str = "WinDevCleaner finds where disk space is on Windows and cleans it safely. \
`scan` measures sizes on disk (hard links counted once, links not followed, cloud files not downloaded) and \
`tree` drills into a folder of the latest scan, `diff` shows what grew since the previous scan of the same folders, \
`explain` says why a path is here, which rule knows it and what deleting it would free \
(for NVIDIA update artifacts it also gives `manual_steps`: the exact steps for the person, the tool never deletes them), \
and `system_items` reports the space Windows manages (Recycle Bin, paging and hibernation files, dumps). \
`programs` lists installed programs with the size each installer reports and its own uninstall command for the person to run (you cannot run it), then the folders an uninstall left behind as candidates a person must confirm. \
`plan` drafts what built-in rules may move to quarantine now, each item explained, and `quarantine` lists what is held. \
You cannot apply, purge or restore: `request_apply` asks the person, who approves in the app with Windows Hello; \
with the app closed it returns `needs-human` and nothing changes. \
Every result carries `app`: `connected`, `headless` or `blocked` (with `app_reason`).";

/// The instructions for the mode in use: the sentence about watching is only said when it is true.
fn instructions_for(backend: &Backend) -> String {
    let mode = match backend {
        Backend::App { .. } => {
            "The WinDevCleaner app is open and this session is attached to it (`app`: `connected`): the person watches every call live in the app and can pause or disconnect you."
                .to_string()
        }
        Backend::Blocked(reason) => format!(
            "The WinDevCleaner app is open but refused this agent (`app`: `blocked`): {reason}. Every call fails; the person has to allow agents again in the app (or close the other agent), and nothing runs headless meanwhile."
        ),
        Backend::Headless { .. } | Backend::Pending => {
            "The WinDevCleaner app is not open (`app`: `headless`): the engine runs here and the person is not watching your calls and cannot pause or disconnect you. \
Ask the person to open the WinDevCleaner app; before each call this server looks for it, and once it is open the next call goes through the app and is watched live. \
The scan held here is not carried over to the app: after that switch call `scan` again before `tree`."
                .to_string()
        }
    };
    format!("{INSTRUCTIONS} {mode}")
}

impl WdcServer {
    fn tool_list() -> Vec<Tool> {
        tools::specs()
            .into_iter()
            .map(|s| {
                let schema = match (s.schema)() {
                    Value::Object(m) => m,
                    _ => serde_json::Map::new(),
                };
                Tool::new(s.name, s.description, Arc::new(schema))
            })
            .collect()
    }
}

/// Opens the app's pipe; `Ok(None)` when the app is closed.
#[cfg(windows)]
fn try_app(client_name: &str) -> Result<Option<Client<Box<dyn LineConn + Send>>>, String> {
    use wdc_core::link::pipe::{connect, pipe_name, Connect};
    let name = pipe_name().map_err(|e| e.to_string())?;
    match connect(&name) {
        Ok(Connect::NotRunning) => Ok(None),
        Ok(Connect::Connected(conn)) => {
            let boxed: Box<dyn LineConn + Send> = Box::new(conn);
            Client::handshake(boxed, client_name).map(Some)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(windows))]
fn try_app(_client_name: &str) -> Result<Option<Client<Box<dyn LineConn + Send>>>, String> {
    Ok(None)
}

/// Starts `WinDevCleaner.exe` from the folder of `wdc.exe` and waits for its pipe.
fn open_app_and_wait(client_name: &str) -> Result<Option<Client<Box<dyn LineConn + Send>>>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let app = exe.with_file_name(if cfg!(windows) { "WinDevCleaner.exe" } else { "WinDevCleaner" });
    std::process::Command::new(&app)
        .spawn()
        .map_err(|e| format!("could not open {}: {e}", app.display()))?;
    for _ in 0..150 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(c) = try_app(client_name)? {
            return Ok(Some(c));
        }
    }
    Err("the WinDevCleaner app did not start listening in 15 s".into())
}

fn choose_backend(client_name: &str, show: bool) -> Backend {
    let found = match try_app(client_name) {
        Ok(None) if show => open_app_and_wait(client_name),
        other => other,
    };
    match found {
        Ok(Some(client)) => Backend::App { client: Box::new(client), upgraded: false },
        Ok(None) => Backend::Headless { last: None, client: client_name.to_string() },
        // The app is open but refused us (the human disconnected an agent), or the pipe is
        // not trustworthy: never fall back to headless in that case.
        Err(reason) => Backend::Blocked(reason),
    }
}

/// Before a call while headless: looks for the app again. Open and welcoming: the server
/// attaches under the same client name; open but refusing: blocked, like `choose_backend`.
/// Only a headless server moves; an attached or blocked one never becomes headless.
fn upgrade_if_headless(backend: &mut Backend, probe: Probe) {
    let client = match backend {
        Backend::Headless { client, .. } => client.clone(),
        _ => return,
    };
    // GUARD-BEGIN(mcp-upgrade)
    match probe(&client) {
        Ok(None) => {}
        Ok(Some(c)) => *backend = Backend::App { client: Box::new(c), upgraded: true },
        Err(reason) => *backend = Backend::Blocked(reason),
    }
    // GUARD-END(mcp-upgrade)
}

fn run_call(backend: &mut Backend, tool: &str, args: Value, probe: Probe) -> Result<Value, String> {
    upgrade_if_headless(backend, probe);
    match backend {
        Backend::Pending => Err("the MCP client has not initialized".into()),
        Backend::Blocked(reason) => Err(format!("WinDevCleaner refused this agent: {reason}")),
        Backend::App { client, upgraded } => match client.call(tool, args) {
            CallOutcome::Ok(v) => {
                if tool == tools::SCAN {
                    *upgraded = false;
                }
                Ok(v)
            }
            CallOutcome::Failed(e) if *upgraded && tool == tools::TREE => Err(format!(
                "{e} (the app was opened after this session started: the scan made here before is not carried over; call `scan` again)"
            )),
            CallOutcome::Failed(e) => Err(e),
            CallOutcome::Gone(reason) => {
                let msg = format!("WinDevCleaner refused this agent: {reason}");
                *backend = Backend::Blocked(reason);
                Err(msg)
            }
        },
        Backend::Headless { last, .. } => match tool {
            tools::SCAN => {
                let args = tools::parse_scan_args(&args)?;
                let (report, summary) = tools::run_scan(&args, &wdc_core::scope::data_dir(), None, None);
                *last = Some(Box::new(report));
                Ok(summary)
            }
            tools::TREE => {
                let (path, limit) = tools::parse_tree_args(&args)?;
                tools::run_tree(last.as_deref(), &path, limit)
            }
            tools::DIFF => {
                let opts = tools::parse_diff_args(&args)?;
                let d = tools::run_diff(&wdc_core::scope::data_dir(), opts)?;
                serde_json::to_value(d).map_err(|e| e.to_string())
            }
            tools::SYSTEM_ITEMS => Ok(tools::run_system_items()),
            tools::PROGRAMS => Ok(tools::run_programs(&wdc_core::scope::data_dir()).1),
            tools::EXPLAIN => {
                let path = tools::parse_explain_args(&args)?;
                let e = tools::run_explain(&path, &wdc_core::scope::data_dir());
                serde_json::to_value(e).map_err(|e| e.to_string())
            }
            tools::PLAN => {
                let opts = tools::parse_plan_args(&args)?;
                let (d, file) = tools::run_plan(&opts, &wdc_core::scope::data_dir())?;
                Ok(tools::plan_json(&d, &file))
            }
            tools::QUARANTINE => Ok(tools::run_quarantine(&wdc_core::scope::data_dir())),
            // Without the app there is no person to ask: nothing changes, ever.
            tools::REQUEST_APPLY => {
                let (plan, _) = tools::parse_request_apply_args(&args)?;
                Ok(tools::needs_human(&plan))
            }
            other => Err(format!("unknown tool `{other}`")),
        },
    }
}

impl ServerHandler for WdcServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("wdc", wdc_core::VERSION))
            .with_instructions(instructions_for(&self.backend.lock().unwrap()))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        context.peer.set_peer_info(request.clone());
        let name = request
            .client_info
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| request.client_info.name.clone());
        let show = self.show;
        let backend = self.backend.clone();
        // Connecting may wait for the app to start; keep it off the async runtime.
        let chosen = tokio::task::spawn_blocking(move || choose_backend(&name, show))
            .await
            .unwrap_or_else(|e| Backend::Blocked(e.to_string()));
        *backend.lock().unwrap() = chosen;
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(Self::tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let tool = request.name.to_string();
        if !tools::specs().iter().any(|s| s.name == tool) {
            let mode = self.backend.lock().unwrap().mode();
            let mut r = CallToolResult::error(vec![ContentBlock::text(format!("unknown tool `{tool}`"))]);
            r.structured_content = Some(stamp(Value::Object(Default::default()), &mode));
            return Ok(r.into());
        }
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut b = backend.lock().unwrap();
            let result = run_call(&mut b, &tool, args, &|name| try_app(name));
            // After the call: it may have turned an attached agent into a blocked one.
            (result, b.mode())
        })
        .await
        .unwrap_or_else(|e| {
            let reason = e.to_string();
            (Err(reason.clone()), AppMode { label: "blocked", reason: Some(reason) })
        });
        let (result, mode) = result;
        let response = match result {
            Ok(v) => {
                let v = stamp(v, &mode);
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
                r.structured_content = Some(v);
                r
            }
            Err(e) => {
                let mut r = CallToolResult::error(vec![ContentBlock::text(e)]);
                r.structured_content = Some(stamp(Value::Object(Default::default()), &mode));
                r
            }
        };
        Ok(response.into())
    }
}

pub fn run(show: bool) -> ExitCode {
    // Agents may run scans: the whole process stays in background mode.
    wdc_core::background::enter_process_background_mode();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("wdc mcp: {e}");
            return ExitCode::from(2);
        }
    };
    let backend = Arc::new(Mutex::new(Backend::Pending));
    let server = WdcServer { show, backend: backend.clone() };
    let outcome = runtime.block_on(async move {
        let service = server.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        service.waiting().await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    });
    if let Backend::App { client, .. } = &mut *backend.lock().unwrap() {
        client.bye();
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wdc mcp: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wdc_core::link::ServerMsg;

    /// An app that welcomes every hello and answers nothing else.
    struct Welcoming(Option<String>);
    impl LineConn for Welcoming {
        fn poll_line(&mut self, _: Duration) -> std::io::Result<Option<String>> {
            Ok(self.0.take())
        }
        fn send_line(&mut self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn welcome() -> AppClient {
        let line = serde_json::to_string(&ServerMsg::Welcome { app_version: "test".into() }).unwrap();
        let conn: Box<dyn LineConn + Send> = Box::new(Welcoming(Some(line)));
        Client::handshake(conn, "unit").unwrap()
    }

    fn label(b: &Backend) -> &'static str {
        b.mode().label
    }

    #[test]
    fn a_blocked_agent_stays_blocked_when_the_app_probe_finds_nothing() {
        let mut b = Backend::Blocked("disconnected by the user".into());
        let r = run_call(&mut b, tools::QUARANTINE, Value::Null, &|_| Ok(None));
        assert!(r.unwrap_err().contains("refused this agent"));
        assert_eq!(label(&b), "blocked", "a blocked agent never becomes headless");
        assert_eq!(b.mode().reason.as_deref(), Some("disconnected by the user"));
    }

    #[test]
    fn a_blocked_agent_stays_blocked_even_when_the_app_would_welcome_it() {
        let mut b = Backend::Blocked("disconnected by the user".into());
        let calls = std::cell::Cell::new(0);
        upgrade_if_headless(&mut b, &|_| {
            calls.set(calls.get() + 1);
            Ok(Some(welcome()))
        });
        assert_eq!(calls.get(), 0, "the app is not even asked");
        assert_eq!(label(&b), "blocked");
    }

    #[test]
    fn a_headless_server_moves_to_the_app_or_to_blocked_and_never_back() {
        let headless = || Backend::Headless { last: None, client: "unit".into() };
        let mut b = headless();
        upgrade_if_headless(&mut b, &|_| Ok(None));
        assert_eq!(label(&b), "headless", "the app is still closed");

        upgrade_if_headless(&mut b, &|name| {
            assert_eq!(name, "unit", "the same client name is used");
            Ok(Some(welcome()))
        });
        assert_eq!(label(&b), "connected");
        assert!(matches!(b, Backend::App { upgraded: true, .. }));

        upgrade_if_headless(&mut b, &|_| Ok(None));
        assert_eq!(label(&b), "connected", "an attached server never becomes headless");

        let mut b = headless();
        upgrade_if_headless(&mut b, &|_| Err("another agent is connected".into()));
        assert_eq!(label(&b), "blocked");
        assert_eq!(b.mode().reason.as_deref(), Some("another agent is connected"));
    }

    #[test]
    fn results_and_instructions_say_the_mode() {
        let m = Backend::Blocked("no".into()).mode();
        let v = stamp(serde_json::json!({ "x": 1 }), &m);
        assert_eq!((v["app"].as_str(), v["app_reason"].as_str(), v["x"].as_u64()), (Some("blocked"), Some("no"), Some(1)));
        let v = stamp(serde_json::json!([1]), &Backend::Pending.mode());
        assert_eq!(v["app"], "headless");
        assert!(v.get("app_reason").is_none());

        let headless = instructions_for(&Backend::Headless { last: None, client: "c".into() });
        assert!(headless.contains("not watching") && headless.contains("open the WinDevCleaner app"));
        assert!(!headless.contains("watches every call"));
        let attached = instructions_for(&Backend::App { client: Box::new(welcome()), upgraded: false });
        assert!(attached.contains("watches every call live"));
        assert!(!attached.contains("not watching"));
        assert!(instructions_for(&Backend::Blocked("r".into())).contains("refused this agent"));
    }
}
