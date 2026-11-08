//! The app's state, shared by the window and the agent link. The window only reads it; scans
//! (by the human or by an agent) run on worker threads in background mode.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::Value;

use wdc_core::activity::{self, Status, What};
use wdc_core::i18n::{Lang, Msg};
use wdc_core::link::{AgentHandler, LinkControl};
use wdc_core::scan::{Progress, ScanReport};
use wdc_core::tools;

/// Who started the scan in progress.
#[derive(Clone, Debug, PartialEq)]
pub enum ScanBy {
    Human,
    Agent(String),
}

#[derive(Clone, Debug)]
pub struct ScanProgress {
    pub by: ScanBy,
    pub target: String,
    pub progress: Progress,
}

/// The latest scan and the index the tree view needs.
pub struct Loaded {
    pub report: Arc<ScanReport>,
    pub children: Arc<Vec<Vec<u32>>>,
}

/// Which view the window shows. It follows the agent: its scan shows the tree, its `diff` the
/// growth view, its `system_items` the system view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Space,
    Growth,
    System,
}

#[derive(Default)]
pub struct Shared {
    pub view: View,
    /// The latest comparison with the previous scan of the same folders, or why there is none.
    pub diff: Option<Result<Arc<wdc_core::diff::Diff>, String>>,
    pub system: Option<Arc<Vec<wdc_core::system::SystemItem>>>,
    pub loaded: Option<Loaded>,
    pub scanning: Option<ScanProgress>,
    /// Folder the view should show, and a counter so the view knows it changed. When an agent
    /// looks at a folder, the view follows it.
    pub focus: Option<u32>,
    pub focus_seq: u64,
    pub link_error: Option<String>,
}

pub struct AppCore {
    pub ctx: egui::Context,
    pub data_dir: PathBuf,
    pub lang: Mutex<Lang>,
    pub shared: Mutex<Shared>,
    pub log: activity::Log,
    pub control: Arc<LinkControl>,
    /// One scan at a time, whoever starts it.
    scan_lock: Mutex<()>,
}

impl AppCore {
    pub fn new(ctx: egui::Context, data_dir: PathBuf, lang: Lang) -> AppCore {
        AppCore {
            ctx,
            log: activity::Log::new(&data_dir),
            data_dir,
            lang: Mutex::new(lang),
            shared: Mutex::new(Shared::default()),
            control: LinkControl::new(),
            scan_lock: Mutex::new(()),
        }
    }

    pub fn lang(&self) -> Lang {
        *self.lang.lock().unwrap()
    }

    /// Runs a scan on the calling thread, in background mode for this thread only, and shows
    /// its result in the window.
    pub fn scan(&self, by: ScanBy, args: &tools::ScanArgs, cancel: Option<&AtomicBool>) -> Value {
        let _one_at_a_time = self.scan_lock.lock().unwrap();
        let _background = wdc_core::background::ThreadBackground::enter();
        {
            let mut s = self.shared.lock().unwrap();
            s.scanning = Some(ScanProgress { by: by.clone(), target: args.target.clone(), progress: Progress::default() });
        }
        self.ctx.request_repaint();
        let ctx = self.ctx.clone();
        let mut on_progress = |p: &Progress| {
            if let Some(sp) = self.shared.lock().unwrap().scanning.as_mut() {
                sp.progress = p.clone();
            }
            ctx.request_repaint();
        };
        let (report, summary) = tools::run_scan(args, &self.data_dir, Some(&mut on_progress), cancel);
        let children = report.children_index();
        // What grew since the previous scan of the same folders, ready in the growth view.
        let diff = tools::run_diff(&self.data_dir, wdc_core::diff::DiffOptions::default()).map(Arc::new);
        let system: Option<Vec<wdc_core::system::SystemItem>> =
            summary.get("system_items").and_then(|v| serde_json::from_value(v.clone()).ok());
        {
            let mut s = self.shared.lock().unwrap();
            s.scanning = None;
            s.view = View::Space;
            s.diff = Some(diff);
            if let Some(items) = system {
                s.system = Some(Arc::new(items));
            }
            s.focus = report.roots.iter().find_map(|r| r.node);
            s.focus_seq += 1;
            s.loaded = Some(Loaded { report: Arc::new(report), children: Arc::new(children) });
        }
        self.ctx.request_repaint();
        summary
    }

    /// Starts a scan of the default read scope for the human (the Scan button).
    pub fn start_human_scan(self: &Arc<Self>) {
        if self.shared.lock().unwrap().scanning.is_some() {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                if let Ok(args) = tools::parse_scan_args(&Value::Null) {
                    me.scan(ScanBy::Human, &args, None);
                }
            })
            .ok();
    }

    /// Reads the system items for the human (the button of the system view).
    pub fn start_system_items(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("system-items".into())
            .spawn(move || {
                let _background = wdc_core::background::ThreadBackground::enter();
                let items = wdc_core::system::system_items();
                me.shared.lock().unwrap().system = Some(Arc::new(items));
                me.ctx.request_repaint();
            })
            .ok();
    }

    pub fn set_view(&self, v: View) {
        self.shared.lock().unwrap().view = v;
        self.ctx.request_repaint();
    }

    pub fn pause_agent(&self) {
        if let Some(client) = self.control.state().client {
            self.control.pause();
            self.log.push(&client, What::Paused, Status::Info);
        }
        self.ctx.request_repaint();
    }

    pub fn resume_agent(&self) {
        if let Some(client) = self.control.state().client {
            self.control.resume();
            self.log.push(&client, What::Resumed, Status::Info);
        }
        self.ctx.request_repaint();
    }

    pub fn disconnect_agent(&self) {
        self.control.disconnect();
        self.ctx.request_repaint();
    }

    pub fn allow_agents(&self) {
        self.control.allow();
        self.ctx.request_repaint();
    }
}

/// Short display of a call's target.
fn call_target(tool: &str, args: &Value) -> String {
    match tool {
        tools::SCAN => tools::parse_scan_args(args).map(|a| a.target).unwrap_or_default(),
        tools::TREE => args.get("path").and_then(Value::as_str).unwrap_or_default().to_string(),
        tools::DIFF | tools::SYSTEM_ITEMS => String::new(),
        _ => String::new(),
    }
}

impl AgentHandler for AppCore {
    fn connected(&self, client: &str) {
        self.log.push(client, What::Connected, Status::Info);
        self.ctx.request_repaint();
    }

    fn disconnected(&self, client: &str, reason: &str) {
        self.log.push(client, What::Disconnected { reason: reason.to_string() }, Status::Info);
        self.ctx.request_repaint();
    }

    #[allow(unused_mut, unused_assignments)]
    fn call_started(&self, client: &str, tool: &str, args: &Value, waiting: bool) -> u64 {
        let status = if waiting { Status::Waiting } else { Status::Running };
        let mut entry = 0;
        // Every agent call is shown in the activity panel as it happens.
        // GUARD-BEGIN(activity-mirror)
        entry = self.log.push(client, What::Call { tool: tool.to_string(), target: call_target(tool, args) }, status);
        // GUARD-END(activity-mirror)
        self.ctx.request_repaint();
        entry
    }

    fn call_resumed(&self, entry: u64) {
        self.log.update(entry, Status::Running, None, None);
        self.ctx.request_repaint();
    }

    fn call(&self, tool: &str, args: &Value, cancel: &AtomicBool) -> Result<Value, String> {
        match tool {
            tools::SCAN => {
                let parsed = tools::parse_scan_args(args)?;
                let client = self.control.state().client.unwrap_or_default();
                Ok(self.scan(ScanBy::Agent(client), &parsed, Some(cancel)))
            }
            tools::TREE => {
                let (path, limit) = tools::parse_tree_args(args)?;
                let mut s = self.shared.lock().unwrap();
                let result = {
                    let report = s.loaded.as_ref().map(|l| l.report.clone());
                    tools::run_tree(report.as_deref(), &path, limit)
                };
                if result.is_ok() {
                    // The view follows the agent to the folder it is looking at.
                    let node = s.loaded.as_ref().and_then(|l| l.report.find_dir(&path));
                    s.focus = node;
                    s.focus_seq += 1;
                }
                drop(s);
                self.ctx.request_repaint();
                result
            }
            tools::DIFF => {
                let opts = tools::parse_diff_args(args)?;
                let result = tools::run_diff(&self.data_dir, opts).map(Arc::new);
                let value = result.as_ref().map(|d| serde_json::to_value(d.as_ref()).unwrap_or(Value::Null)).map_err(|e| e.clone());
                {
                    // The window shows the comparison the agent is reading.
                    let mut s = self.shared.lock().unwrap();
                    s.diff = Some(result);
                    s.view = View::Growth;
                }
                self.ctx.request_repaint();
                value
            }
            tools::SYSTEM_ITEMS => {
                let _background = wdc_core::background::ThreadBackground::enter();
                let items = wdc_core::system::system_items();
                let value = serde_json::json!({ "total_bytes": wdc_core::system::total(&items), "items": items });
                {
                    let mut s = self.shared.lock().unwrap();
                    s.system = Some(Arc::new(items));
                    s.view = View::System;
                }
                self.ctx.request_repaint();
                Ok(value)
            }
            other => Err(format!("unknown tool `{other}`")),
        }
    }

    fn call_finished(&self, entry: u64, result: &Result<Value, String>) {
        let lang = self.lang();
        match result {
            Ok(v) => {
                let totals = tools::scan_totals(v);
                let text = if let Some(bytes) = totals.get("bytes_on_disk").and_then(Value::as_u64) {
                    let files = totals.get("files").and_then(Value::as_u64).unwrap_or(0);
                    lang.fmt(
                        Msg::AppActScanResult,
                        &[("size", &wdc_core::fmt::bytes(bytes)), ("files", &wdc_core::fmt::count(files))],
                    )
                } else if let Some(delta) = v.get("delta_bytes").and_then(Value::as_i64) {
                    let sign = if delta < 0 { "-" } else { "+" };
                    format!("{sign}{}", wdc_core::fmt::bytes(delta.unsigned_abs()))
                } else if let Some(bytes) = v.get("total_bytes").and_then(Value::as_u64) {
                    wdc_core::fmt::bytes(bytes)
                } else if let Some(bytes) = v.get("bytes_on_disk").and_then(Value::as_u64) {
                    wdc_core::fmt::bytes(bytes)
                } else {
                    String::new()
                };
                let data = if !totals.is_null() {
                    Some(serde_json::json!({ "totals": totals }))
                } else if v.get("delta_bytes").is_some() {
                    Some(serde_json::json!({
                        "delta_bytes": v["delta_bytes"],
                        "changes": v["dirs"].as_array().map(|a| a.len()).unwrap_or(0),
                    }))
                } else if v.get("total_bytes").is_some() {
                    Some(serde_json::json!({ "total_bytes": v["total_bytes"] }))
                } else {
                    None
                };
                self.log.update(entry, Status::Done, Some(text), data);
            }
            Err(e) => self.log.update(entry, Status::Failed, Some(e.clone()), None),
        }
        self.ctx.request_repaint();
    }
}

/// Opens the agent link (Windows named pipe). The app works without it; the window shows
/// why agents cannot connect.
pub fn start_agent_link(app: &Arc<AppCore>) {
    #[cfg(windows)]
    {
        let handler: Arc<dyn AgentHandler> = app.clone();
        if let Err(e) = wdc_core::link::pipe::listen(app.control.clone(), handler) {
            app.shared.lock().unwrap().link_error = Some(e.to_string());
        }
    }
    #[cfg(not(windows))]
    {
        app.shared.lock().unwrap().link_error = Some("the agent link needs Windows".into());
    }
}
