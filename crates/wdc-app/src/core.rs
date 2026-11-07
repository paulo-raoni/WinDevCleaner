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

#[derive(Default)]
pub struct Shared {
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
        {
            let mut s = self.shared.lock().unwrap();
            s.scanning = None;
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
                } else if let Some(bytes) = v.get("bytes_on_disk").and_then(Value::as_u64) {
                    wdc_core::fmt::bytes(bytes)
                } else {
                    String::new()
                };
                let data = if totals.is_null() { None } else { Some(serde_json::json!({ "totals": totals })) };
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
