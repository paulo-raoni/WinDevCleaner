//! The app's state, shared by the window and the agent link. The window only reads it; scans
//! (by the human or by an agent) run on worker threads in background mode.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::Value;

use wdc_core::activity::{self, Status, What};
use wdc_core::i18n::{Lang, Msg};
use wdc_core::link::{AgentHandler, LinkControl};
use wdc_core::plan::{Draft, Plan};
use wdc_core::presence::{self, Method, Proof};
use wdc_core::quarantine::{self, Entry as QEntry};
use wdc_core::scan::{Progress, ScanReport};
use wdc_core::tools;

/// Threads of a scan started with the "Scan faster" switch.
pub const FASTER_JOBS: usize = wdc_core::scan::MAX_JOBS;

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
/// growth view, its `system_items` the system view, its `programs` the programs view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Space,
    Growth,
    System,
    Programs,
    Explain,
    Plan,
    Quarantine,
}

/// The person's answer on the approval card.
pub enum Decision {
    Approve(Proof),
    Reject,
}

/// A plan waiting for the person's approval (plan, sections 4.6 and 7).
pub struct Approval {
    pub plan: Arc<Plan>,
    pub purge_now: bool,
    /// The agent that asked, or None when the person started it.
    pub agent: Option<String>,
    /// Windows Hello is set up: the card approves with it, else with the click itself.
    pub hello: bool,
    /// Windows Hello is asking now.
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Shared {
    pub view: View,
    /// The latest comparison with the previous scan of the same folders, or why there is none.
    pub diff: Option<Result<Arc<wdc_core::diff::Diff>, String>>,
    pub system: Option<Arc<Vec<wdc_core::system::SystemItem>>>,
    pub programs: Option<Arc<wdc_core::programs::Report>>,
    pub explanation: Option<Arc<wdc_core::explain::Explanation>>,
    pub loaded: Option<Loaded>,
    pub scanning: Option<ScanProgress>,
    /// Folder the view should show, and a counter so the view knows it changed. When an agent
    /// looks at a folder, the view follows it.
    pub focus: Option<u32>,
    pub focus_seq: u64,
    pub link_error: Option<String>,
    pub draft: Option<Result<Arc<Draft>, String>>,
    pub approval: Option<Approval>,
    pub last_apply: Option<Arc<wdc_core::apply::ApplyReport>>,
    pub quarantine: Option<Arc<Vec<QEntry>>>,
    /// The result of the latest restore or purge, as a sentence.
    pub last_change: Option<String>,
    /// A plan, restore or purge is running.
    pub busy: bool,
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
    /// Where the approval card sends the person's answer.
    decision: Mutex<Option<mpsc::Sender<Decision>>>,
}

/// How long an approval card waits for the person.
fn approval_timeout() -> Duration {
    let secs = std::env::var("WDC_APPROVAL_TIMEOUT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(300u64);
    Duration::from_secs(secs.clamp(1, 3600))
}

fn now_secs() -> u64 {
    wdc_core::time::now_unix_ms() / 1000
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
            decision: Mutex::new(None),
        }
    }

    /// Drafts a plan (by the person or an agent) and shows it.
    pub fn draft_plan(&self, opts: &wdc_core::plan::DraftOptions) -> Result<Value, String> {
        let _background = wdc_core::background::ThreadBackground::enter();
        let result = tools::run_plan(opts, &self.data_dir);
        let value = result.as_ref().map(|(d, file)| tools::plan_json(d, file)).map_err(|e| e.clone());
        {
            let mut s = self.shared.lock().unwrap();
            s.draft = Some(result.map(|(d, _)| Arc::new(d)));
            s.view = View::Plan;
        }
        self.ctx.request_repaint();
        value
    }

    pub fn start_draft(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("plan".into())
            .spawn(move || {
                let _ = me.draft_plan(&wdc_core::plan::DraftOptions::default());
            })
            .ok();
    }

    /// Shows the approval card and waits for the person, the timeout, or the agent leaving.
    /// None when nobody answered.
    fn ask_approval(&self, plan: Arc<Plan>, purge_now: bool, agent: Option<String>, cancel: Option<&AtomicBool>) -> Result<Option<Decision>, String> {
        let (tx, rx) = mpsc::channel();
        {
            let mut d = self.decision.lock().unwrap();
            if d.is_some() {
                return Err("another approval is already waiting for the person".into());
            }
            *d = Some(tx);
        }
        let hello = presence::hello_available();
        {
            let mut s = self.shared.lock().unwrap();
            s.approval = Some(Approval { plan, purge_now, agent, hello, busy: false, error: None });
            s.view = View::Plan;
        }
        self.ctx.request_repaint();
        let deadline = Instant::now() + approval_timeout();
        let answer = loop {
            if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) || Instant::now() >= deadline {
                break None;
            }
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(d) => break Some(d),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            }
        };
        *self.decision.lock().unwrap() = None;
        self.shared.lock().unwrap().approval = None;
        self.ctx.request_repaint();
        Ok(answer)
    }

    /// The card's Approve button: Windows Hello when set up, else the click is the proof.
    pub fn approve(self: &Arc<Self>) {
        let (hello, message) = {
            let mut s = self.shared.lock().unwrap();
            let Some(a) = s.approval.as_mut() else { return };
            if a.busy {
                return;
            }
            a.error = None;
            a.busy = a.hello;
            let lang = *self.lang.lock().unwrap();
            let msg = lang.fmt(
                Msg::CliApplyWhat,
                &[("n", &a.plan.items.len().to_string()), ("size", &wdc_core::fmt::bytes(a.plan.bytes_on_disk())), ("hash", a.plan.short_hash())],
            );
            (a.hello, msg)
        };
        if !hello {
            self.send_decision(Decision::Approve(Proof::new(Method::AppApproval)));
            return;
        }
        let hwnd = presence::foreground_window();
        let me = self.clone();
        std::thread::Builder::new()
            .name("hello".into())
            .spawn(move || match presence::hello(hwnd, &message) {
                Ok(p) => me.send_decision(Decision::Approve(p)),
                Err(e) => {
                    let lang = me.lang();
                    if let Some(a) = me.shared.lock().unwrap().approval.as_mut() {
                        a.busy = false;
                        a.error = Some(e.text(lang));
                    }
                    me.ctx.request_repaint();
                }
            })
            .ok();
    }

    pub fn reject(&self) {
        self.send_decision(Decision::Reject);
    }

    fn send_decision(&self, d: Decision) {
        if let Some(tx) = self.decision.lock().unwrap().as_ref() {
            let _ = tx.send(d);
        }
    }

    fn run_apply(&self, plan: &Plan, purge_now: bool, proof: &Proof) -> Result<Arc<wdc_core::apply::ApplyReport>, String> {
        let _background = wdc_core::background::ThreadBackground::enter();
        let opts = wdc_core::apply::ApplyOptions { purge_now, explained: true };
        let rules = wdc_core::rules::RuleSet::built_in();
        let ctx = wdc_core::rules::Context::system();
        let r = wdc_core::apply::apply(plan, &rules, &ctx, &self.data_dir, &opts, proof, now_secs())
            .map(Arc::new)
            .map_err(|r| r.0.text(self.lang()));
        {
            let mut s = self.shared.lock().unwrap();
            if let Ok(rep) = &r {
                s.last_apply = Some(rep.clone());
            }
            s.quarantine = Some(Arc::new(quarantine::list(&self.data_dir)));
        }
        self.ctx.request_repaint();
        r
    }

    /// The person's "Move to quarantine" on the drafted plan: the same card, then apply.
    pub fn start_human_apply(self: &Arc<Self>, purge_now: bool) {
        let plan = match self.shared.lock().unwrap().draft.as_ref() {
            Some(Ok(d)) => Arc::new(d.plan.clone()),
            _ => return,
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("apply".into())
            .spawn(move || {
                if let Ok(Some(Decision::Approve(proof))) = me.ask_approval(plan.clone(), purge_now, None, None) {
                    let _ = me.run_apply(&plan, purge_now, &proof);
                }
            })
            .ok();
    }

    pub fn refresh_quarantine(&self) {
        self.shared.lock().unwrap().quarantine = Some(Arc::new(quarantine::list(&self.data_dir)));
        self.ctx.request_repaint();
    }

    /// Restore or purge from the quarantine view. The click is made by the person in the app;
    /// Windows Hello is asked too when it is set up.
    pub fn start_quarantine_change(self: &Arc<Self>, restore: Option<String>, early: bool) {
        let me = self.clone();
        let hwnd = presence::foreground_window();
        self.shared.lock().unwrap().busy = true;
        std::thread::Builder::new()
            .name("quarantine".into())
            .spawn(move || {
                let lang = me.lang();
                let proof = if presence::hello_available() {
                    presence::hello(hwnd, lang.t(Msg::AppTabQuarantine))
                } else {
                    Ok(Proof::new(Method::AppApproval))
                };
                let text = match proof {
                    Err(e) => e.text(lang),
                    Ok(proof) => match &restore {
                        Some(id) => {
                            let r = quarantine::restore(&me.data_dir, id, &wdc_core::rules::Context::system(), &proof);
                            wdc_core::apply::log(&me.data_dir, "restore", &serde_json::json!({ "id": id, "result": r.as_ref().map(|e| &e.original) }));
                            match r {
                                Ok(e) => lang.fmt(Msg::CliRestored, &[("path", &e.original)]),
                                Err(why) => lang.fmt(Msg::CliRefused, &[("why", &why.text(lang))]),
                            }
                        }
                        None => {
                            let r = quarantine::purge(&me.data_dir, None, early, now_secs(), &proof);
                            wdc_core::apply::log(&me.data_dir, "purge", &r);
                            let n = r.items.iter().filter(|i| i.purged).count();
                            lang.fmt(Msg::CliPurged, &[("n", &n.to_string()), ("size", &wdc_core::fmt::bytes(r.purged_bytes))])
                        }
                    },
                };
                let mut s = me.shared.lock().unwrap();
                s.busy = false;
                s.last_change = Some(text);
                s.quarantine = Some(Arc::new(quarantine::list(&me.data_dir)));
                drop(s);
                me.ctx.request_repaint();
            })
            .ok();
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

    /// Starts a scan of the default read scope for the human (the Scan button). With `faster`
    /// (the "Scan faster" switch) the scan walks with [`FASTER_JOBS`] threads, each in
    /// background mode, and returns the same report.
    pub fn start_human_scan(self: &Arc<Self>, faster: bool) {
        if self.shared.lock().unwrap().scanning.is_some() {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                if let Ok(mut args) = tools::parse_scan_args(&Value::Null) {
                    if faster {
                        args.options.jobs = FASTER_JOBS;
                    }
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

    /// Reads the installed programs for the human (the button of the programs view).
    pub fn start_programs(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("programs".into())
            .spawn(move || {
                let _background = wdc_core::background::ThreadBackground::enter();
                let (report, _) = tools::run_programs(&me.data_dir);
                me.shared.lock().unwrap().programs = Some(Arc::new(report));
                me.ctx.request_repaint();
            })
            .ok();
    }

    /// Explains a folder for the human (the "?" next to it in the tree).
    pub fn start_explain(self: &Arc<Self>, path: String) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("explain".into())
            .spawn(move || {
                let _background = wdc_core::background::ThreadBackground::enter();
                let e = tools::run_explain(&path, &me.data_dir);
                let mut s = me.shared.lock().unwrap();
                s.explanation = Some(Arc::new(e));
                s.view = View::Explain;
                drop(s);
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
        tools::DIFF | tools::SYSTEM_ITEMS | tools::PROGRAMS => String::new(),
        tools::EXPLAIN => args.get("path").and_then(Value::as_str).unwrap_or_default().to_string(),
        tools::REQUEST_APPLY => args.get("plan").and_then(Value::as_str).unwrap_or_default().to_string(),
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
                let value = tools::system_items_value(&items);
                {
                    let mut s = self.shared.lock().unwrap();
                    s.system = Some(Arc::new(items));
                    s.view = View::System;
                }
                self.ctx.request_repaint();
                Ok(value)
            }
            tools::PROGRAMS => {
                let _background = wdc_core::background::ThreadBackground::enter();
                let (report, value) = tools::run_programs(&self.data_dir);
                {
                    // The window shows the list the agent is reading.
                    let mut s = self.shared.lock().unwrap();
                    s.programs = Some(Arc::new(report));
                    s.view = View::Programs;
                }
                self.ctx.request_repaint();
                Ok(value)
            }
            tools::EXPLAIN => {
                let path = tools::parse_explain_args(args)?;
                let _background = wdc_core::background::ThreadBackground::enter();
                let e = tools::run_explain(&path, &self.data_dir);
                let value = serde_json::to_value(&e).map_err(|e| e.to_string());
                {
                    // The window shows the explanation the agent is reading.
                    let mut s = self.shared.lock().unwrap();
                    s.explanation = Some(Arc::new(e));
                    s.view = View::Explain;
                }
                self.ctx.request_repaint();
                value
            }
            tools::PLAN => {
                let opts = tools::parse_plan_args(args)?;
                self.draft_plan(&opts)
            }
            tools::QUARANTINE => {
                let value = tools::run_quarantine(&self.data_dir);
                self.refresh_quarantine();
                self.set_view(View::Quarantine);
                Ok(value)
            }
            tools::REQUEST_APPLY => {
                let (hash, purge_now) = tools::parse_request_apply_args(args)?;
                let plan = Plan::find_in(&self.data_dir, &hash)?;
                if let Some(r) = plan.refusal(now_secs()) {
                    return Ok(serde_json::json!({ "status": "refused", "reason": r, "changed": false }));
                }
                let plan = Arc::new(plan);
                let client = self.control.state().client.unwrap_or_default();
                // The agent waits here while the person decides on the card.
                match self.ask_approval(plan.clone(), purge_now, Some(client), Some(cancel))? {
                    Some(Decision::Approve(proof)) => {
                        let report = self.run_apply(&plan, purge_now, &proof)?;
                        Ok(serde_json::json!({
                            "status": "approved",
                            "changed": report.moved > 0,
                            "moved": report.moved,
                            "moved_bytes": report.moved_bytes,
                            "skipped": report.skipped,
                            "skipped_bytes": report.skipped_bytes,
                            "purged_bytes": report.purged_bytes,
                            "report": report.as_ref(),
                        }))
                    }
                    Some(Decision::Reject) => Ok(serde_json::json!({ "status": "rejected", "changed": false })),
                    None => Ok(serde_json::json!({ "status": "timed-out", "changed": false })),
                }
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
                } else if let Some(freed) = v.get("expected_freed_bytes").and_then(Value::as_u64) {
                    lang.fmt(Msg::ExplainFreed, &[("size", &wdc_core::fmt::bytes(freed))])
                } else if let Some(delta) = v.get("delta_bytes").and_then(Value::as_i64) {
                    let sign = if delta < 0 { "-" } else { "+" };
                    format!("{sign}{}", wdc_core::fmt::bytes(delta.unsigned_abs()))
                } else if let Some(status) = v.get("status").and_then(Value::as_str) {
                    status.to_string()
                } else if let Some(bytes) = v.get("total_bytes").and_then(Value::as_u64) {
                    wdc_core::fmt::bytes(bytes)
                } else if let Some(bytes) = v.get("bytes_on_disk").and_then(Value::as_u64) {
                    wdc_core::fmt::bytes(bytes)
                } else if let (Some(n), Some(left)) = (v.get("count").and_then(Value::as_u64), v.get("leftovers").and_then(Value::as_array)) {
                    lang.fmt(Msg::AppActProgramsResult, &[("n", &n.to_string()), ("left", &left.len().to_string())])
                } else {
                    String::new()
                };
                let data = if !totals.is_null() {
                    Some(serde_json::json!({ "totals": totals }))
                } else if v.get("expected_freed_bytes").is_some() {
                    Some(serde_json::json!({
                        "expected_freed_bytes": v["expected_freed_bytes"],
                        "rule": v["rule"]["id"],
                        "actionable": v["actionable"],
                    }))
                } else if v.get("delta_bytes").is_some() {
                    Some(serde_json::json!({
                        "delta_bytes": v["delta_bytes"],
                        "changes": v["dirs"].as_array().map(|a| a.len()).unwrap_or(0),
                    }))
                } else if v.get("status").is_some() {
                    Some(serde_json::json!({ "status": v["status"], "changed": v["changed"] }))
                } else if v.get("total_bytes").is_some() {
                    Some(serde_json::json!({ "total_bytes": v["total_bytes"] }))
                } else if v.get("leftovers").is_some() {
                    Some(serde_json::json!({
                        "count": v["count"],
                        "reported_bytes_total": v["reported_bytes_total"],
                        "leftovers": v["leftovers"].as_array().map(|a| a.len()).unwrap_or(0),
                    }))
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
