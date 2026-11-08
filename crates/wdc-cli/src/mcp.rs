//! `wdc mcp`: MCP server on stdio (plan, section 4.6).
//!
//! When the WinDevCleaner app is open, every call goes through the app over the per-user
//! named pipe, so the human sees it live and can pause or disconnect the agent. When the app
//! is closed, the engine runs headless here (`--show` opens the app first). The mode is fixed
//! when the client initializes: an agent the human disconnected gets errors from then on and
//! never falls back to headless.

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

enum Backend {
    /// Not initialized yet.
    Pending,
    /// The app is open: calls go through it.
    App(Box<Client<Box<dyn LineConn + Send>>>),
    /// The app is closed: the engine runs here. Keeps the latest scan for `tree`.
    Headless(Option<Box<ScanReport>>),
    /// The app refused or dropped this agent; every call fails with the reason.
    Blocked(String),
}

struct WdcServer {
    show: bool,
    backend: Arc<Mutex<Backend>>,
}

const INSTRUCTIONS: &str = "WinDevCleaner finds where disk space is on Windows. This version is read-only: \
`scan` measures sizes on disk (hard links counted once, links not followed, cloud files not downloaded) and \
`tree` drills into a folder of the latest scan, `diff` shows what grew since the previous scan of the same folders, \
and `system_items` reports the space Windows manages (Recycle Bin, paging and hibernation files, dumps). When the WinDevCleaner app is open, the user watches every \
call live in the app and can pause or disconnect you.";

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
        Ok(Some(client)) => Backend::App(Box::new(client)),
        Ok(None) => Backend::Headless(None),
        // The app is open but refused us (the human disconnected an agent), or the pipe is
        // not trustworthy: never fall back to headless in that case.
        Err(reason) => Backend::Blocked(reason),
    }
}

fn run_call(backend: &mut Backend, tool: &str, args: Value) -> Result<Value, String> {
    match backend {
        Backend::Pending => Err("the MCP client has not initialized".into()),
        Backend::Blocked(reason) => Err(format!("WinDevCleaner refused this agent: {reason}")),
        Backend::App(client) => match client.call(tool, args) {
            CallOutcome::Ok(v) => Ok(v),
            CallOutcome::Failed(e) => Err(e),
            CallOutcome::Gone(reason) => {
                let msg = format!("WinDevCleaner refused this agent: {reason}");
                *backend = Backend::Blocked(reason);
                Err(msg)
            }
        },
        Backend::Headless(last) => match tool {
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
            other => Err(format!("unknown tool `{other}`")),
        },
    }
}

impl ServerHandler for WdcServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("wdc", wdc_core::VERSION))
            .with_instructions(INSTRUCTIONS)
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
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!("unknown tool `{tool}`"))]).into());
        }
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut b = backend.lock().unwrap();
            run_call(&mut b, &tool, args)
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let response = match result {
            Ok(v) => {
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
                r.structured_content = Some(v);
                r
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
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
    if let Backend::App(client) = &mut *backend.lock().unwrap() {
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
