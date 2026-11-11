//! The window. It reads the shared state each frame; egui redraws only on input or when a
//! worker asks for it, so an idle window uses no CPU.

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};

use wdc_core::activity::{Entry, Status, What};
use wdc_core::fmt;
use wdc_core::i18n::{Lang, Msg};
use wdc_core::scan::ScanReport;

use crate::core::{AppCore, ScanBy, View};

/// Children shown per folder before "... and N more".
const MAX_CHILDREN: usize = 300;

pub struct WdcApp {
    core: Arc<AppCore>,
    seen_focus: u64,
    /// Folders to force open once, so the view follows the agent.
    open_path: HashSet<u32>,
    focus_node: Option<u32>,
    /// A folder whose "?" was pressed this frame.
    pending_explain: Option<u32>,
}

impl WdcApp {
    pub fn new(core: Arc<AppCore>) -> WdcApp {
        WdcApp { core, seen_focus: 0, open_path: HashSet::new(), focus_node: None, pending_explain: None }
    }
}

fn status_text(lang: Lang, s: Status) -> (&'static str, Color32) {
    match s {
        Status::Running => (lang.t(Msg::AppStatusRunning), Color32::from_rgb(0x2f, 0x80, 0xed)),
        Status::Waiting => (lang.t(Msg::AppStatusWaiting), Color32::from_rgb(0xd9, 0x8c, 0x00)),
        Status::Done => (lang.t(Msg::AppStatusDone), Color32::from_rgb(0x27, 0xae, 0x60)),
        Status::Failed => (lang.t(Msg::AppStatusFailed), Color32::from_rgb(0xeb, 0x57, 0x57)),
        Status::Info => (lang.t(Msg::AppStatusInfo), Color32::GRAY),
    }
}

fn describe(lang: Lang, e: &Entry) -> String {
    match &e.what {
        What::Connected => lang.t(Msg::AppActConnected).to_string(),
        What::Disconnected { reason } => lang.fmt(Msg::AppActDisconnected, &[("reason", reason)]),
        What::Paused => lang.t(Msg::AppActPaused).to_string(),
        What::Resumed => lang.t(Msg::AppActResumed).to_string(),
        What::Call { tool, target } => match tool.as_str() {
            wdc_core::tools::SCAN => lang.fmt(Msg::AppActScanning, &[("roots", target)]),
            wdc_core::tools::TREE => lang.fmt(Msg::AppActTree, &[("path", target)]),
            wdc_core::tools::DIFF => lang.t(Msg::AppActDiff).to_string(),
            wdc_core::tools::SYSTEM_ITEMS => lang.t(Msg::AppActSystem).to_string(),
            wdc_core::tools::PROGRAMS => lang.t(Msg::AppActPrograms).to_string(),
            wdc_core::tools::EXPLAIN => lang.fmt(Msg::AppActExplain, &[("path", target)]),
            wdc_core::tools::PLAN => lang.t(Msg::AppActPlan).to_string(),
            wdc_core::tools::QUARANTINE => lang.t(Msg::AppActQuarantine).to_string(),
            wdc_core::tools::REQUEST_APPLY => lang.fmt(Msg::AppActRequestApply, &[("hash", target)]),
            other => format!("{other} {target}"),
        },
    }
}

impl eframe::App for WdcApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let core = self.core.clone();
        let lang = core.lang();
        let link = core.control.state();

        // Toolbar.
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("WinDevCleaner");
                ui.separator();
                let scanning = core.shared.lock().unwrap().scanning.is_some();
                if ui.add_enabled(!scanning, egui::Button::new(lang.t(Msg::AppScan))).clicked() {
                    core.start_human_scan();
                }
                ui.label(RichText::new(lang.t(Msg::AppReadOnlyNote)).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut chosen = lang;
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(chosen.tag())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut chosen, Lang::En, "English");
                            ui.selectable_value(&mut chosen, Lang::PtBr, "Português (Brasil)");
                        });
                    ui.label(lang.t(Msg::AppLanguage));
                    if chosen != lang {
                        *core.lang.lock().unwrap() = chosen;
                    }
                });
            });
            ui.add_space(4.0);
        });

        // Agent banner with the human's controls, always visible.
        egui::Panel::top("agent").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                match &link.client {
                    Some(client) => {
                        ui.label(
                            RichText::new(lang.fmt(Msg::AppAgentConnected, &[("client", client)]))
                                .strong()
                                .color(Color32::from_rgb(0x2f, 0x80, 0xed)),
                        );
                    }
                    None if link.refusing => {
                        ui.label(RichText::new(lang.t(Msg::AppAgentsRefused)).color(Color32::from_rgb(0xd9, 0x8c, 0x00)));
                    }
                    None => {
                        ui.label(RichText::new(lang.t(Msg::AppNoAgent)).weak());
                    }
                }
                let connected = link.client.is_some();
                if link.paused {
                    if ui.add_enabled(connected, egui::Button::new(lang.t(Msg::AppResumeAgent))).clicked() {
                        core.resume_agent();
                    }
                } else if ui.add_enabled(connected, egui::Button::new(lang.t(Msg::AppPauseAgent))).clicked() {
                    core.pause_agent();
                }
                if ui.add_enabled(connected, egui::Button::new(lang.t(Msg::AppDisconnectAgent))).clicked() {
                    core.disconnect_agent();
                }
                if link.refusing && !connected && ui.button(lang.t(Msg::AppAllowAgents)).clicked() {
                    core.allow_agents();
                }
                if link.paused {
                    ui.label(RichText::new(lang.t(Msg::AppAgentPausedBanner)).color(Color32::from_rgb(0xd9, 0x8c, 0x00)));
                }
                if let Some(err) = &core.shared.lock().unwrap().link_error {
                    ui.label(RichText::new(lang.fmt(Msg::AppLinkUnavailable, &[("error", err)])).color(Color32::from_rgb(0xeb, 0x57, 0x57)));
                }
            });
            ui.add_space(4.0);
        });

        // Status line.
        egui::Panel::bottom("status").show(ui, |ui| {
            let s = core.shared.lock().unwrap();
            if let Some(sp) = &s.scanning {
                let who = match &sp.by {
                    ScanBy::Human => lang.t(Msg::AppHumanScan).to_string(),
                    ScanBy::Agent(name) => name.clone(),
                };
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "{} ({who})  {}",
                        lang.fmt(
                            Msg::AppScanning,
                            &[("files", &fmt::count(sp.progress.files)), ("size", &fmt::bytes(sp.progress.bytes_on_disk))]
                        ),
                        if sp.progress.current.is_empty() { &sp.target } else { &sp.progress.current }
                    ));
                });
            } else if let Some(l) = &s.loaded {
                let t = &l.report.totals;
                ui.label(lang.fmt(
                    Msg::AppScanSummary,
                    &[
                        ("size", &fmt::bytes(t.bytes_on_disk)),
                        ("files", &fmt::count(t.files)),
                        ("dirs", &fmt::count(t.dirs)),
                        ("secs", &format!("{:.1}", l.report.duration_ms as f64 / 1000.0)),
                    ],
                ));
            } else {
                ui.label(" ");
            }
        });

        // Activity panel.
        egui::Panel::right("activity").default_size(380.0).show(ui, |ui| {
            ui.heading(lang.t(Msg::AppActivity));
            ui.separator();
            let entries = core.log.entries();
            if entries.is_empty() {
                ui.label(RichText::new(lang.t(Msg::AppNoActivity)).weak());
            }
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for e in &entries {
                    let (status, color) = status_text(lang, e.status);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&e.clock).monospace().weak());
                        ui.label(RichText::new(&e.client).strong());
                        ui.label(describe(lang, e));
                        ui.label(RichText::new(status).color(color));
                        if let Some(r) = &e.result {
                            ui.label(RichText::new(r).weak());
                        }
                    });
                }
            });
        });

        // Where is my space?
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let view = core.shared.lock().unwrap().view;
            ui.horizontal(|ui| {
                for (v, label) in [
                    (View::Space, Msg::AppWhereIsMySpace),
                    (View::Growth, Msg::AppTabGrowth),
                    (View::System, Msg::AppTabSystem),
                    (View::Programs, Msg::AppTabPrograms),
                    (View::Explain, Msg::AppTabExplain),
                    (View::Plan, Msg::AppTabPlan),
                    (View::Quarantine, Msg::AppTabQuarantine),
                ] {
                    if ui.selectable_label(view == v, RichText::new(lang.t(label)).heading()).clicked() {
                        if v == View::Quarantine {
                            core.refresh_quarantine();
                        }
                        core.set_view(v);
                    }
                }
            });
            ui.separator();
            match view {
                View::Growth => {
                    growth_view(ui, lang, &core);
                    return;
                }
                View::System => {
                    system_view(ui, lang, &core);
                    return;
                }
                View::Programs => {
                    programs_view(ui, lang, &core);
                    return;
                }
                View::Explain => {
                    explain_view(ui, lang, &core);
                    return;
                }
                View::Plan => {
                    plan_view(ui, lang, &core);
                    return;
                }
                View::Quarantine => {
                    quarantine_view(ui, lang, &core);
                    return;
                }
                View::Space => {}
            }
            let (loaded, focus, focus_seq) = {
                let s = core.shared.lock().unwrap();
                (s.loaded.as_ref().map(|l| (l.report.clone(), l.children.clone())), s.focus, s.focus_seq)
            };
            let Some((report, children)) = loaded else {
                ui.label(lang.t(Msg::AppNoScanYet));
                return;
            };
            if focus_seq != self.seen_focus {
                self.seen_focus = focus_seq;
                self.open_path.clear();
                self.focus_node = focus;
                let mut n = focus;
                while let Some(i) = n {
                    self.open_path.insert(i);
                    let p = report.dirs[i as usize].parent;
                    n = if p == wdc_core::scan::NO_PARENT { None } else { Some(p) };
                }
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for r in &report.roots {
                    match r.node {
                        Some(n) => self.node(ui, lang, &report, &children, n, report.dirs[n as usize].bytes),
                        None => {
                            ui.label(format!("{}: {}", r.path, r.error.clone().unwrap_or_default()));
                        }
                    }
                }
            });
            if let Some(n) = self.pending_explain.take() {
                core.start_explain(report.dir_path(n));
            }
        });
    }
}

impl WdcApp {
    fn node(&mut self, ui: &mut egui::Ui, lang: Lang, report: &ScanReport, children: &[Vec<u32>], n: u32, parent_bytes: u64) {
        let d = &report.dirs[n as usize];
        let share = if parent_bytes > 0 { d.bytes as f32 / parent_bytes as f32 } else { 0.0 };
        let text = format!(
            "{:>10}  {:>4.0}%  {}   ({})",
            fmt::bytes(d.bytes),
            share * 100.0,
            d.name,
            lang.fmt(Msg::AppFiles, &[("n", &fmt::count(d.files))])
        );
        let kids = &children[n as usize];
        let focused = self.focus_node == Some(n);
        // Forced opening happens once per node; then the human is free to collapse it.
        let force = self.open_path.remove(&n);
        let label = if focused { RichText::new(text).monospace().strong() } else { RichText::new(text).monospace() };
        let explain_tip = lang.t(Msg::AppTabExplain);
        if kids.is_empty() {
            let resp = ui
                .horizontal(|ui| {
                    if ui.small_button("?").on_hover_text(explain_tip).clicked() {
                        self.pending_explain = Some(n);
                    }
                    ui.label(label)
                })
                .inner;
            if focused && force {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            return;
        }
        let id = ui.make_persistent_id(("dir", n));
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, d.parent == wdc_core::scan::NO_PARENT);
        if force {
            state.set_open(true);
        }
        let (_, header, _) = state
            .show_header(ui, |ui| {
                if ui.small_button("?").on_hover_text(explain_tip).clicked() {
                    self.pending_explain = Some(n);
                }
                ui.label(label)
            })
            .body(|ui| {
                for &k in kids.iter().take(MAX_CHILDREN) {
                    self.node(ui, lang, report, children, k, d.bytes);
                }
                if kids.len() > MAX_CHILDREN {
                    ui.label(
                        RichText::new(lang.fmt(Msg::AppMoreItems, &[("n", &fmt::count((kids.len() - MAX_CHILDREN) as u64))]))
                            .weak(),
                    );
                }
            });
        if focused && force {
            header.response.scroll_to_me(Some(egui::Align::Center));
        }
    }
}

fn signed(delta: i64) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{}", fmt::bytes(delta.unsigned_abs()))
}

/// "What grew?": the latest comparison, new and grown folders first.
fn growth_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    use wdc_core::diff::Change;
    let diff = core.shared.lock().unwrap().diff.clone();
    let d = match diff {
        Some(Ok(d)) => d,
        _ => {
            ui.label(lang.t(Msg::AppGrowthNone));
            return;
        }
    };
    ui.label(
        RichText::new(lang.fmt(
            Msg::CliDiffHeader,
            &[("from", &d.from.created), ("to", &d.to.created), ("delta", &signed(d.delta_bytes))],
        ))
        .strong(),
    );
    if d.dirs.is_empty() {
        ui.label(lang.fmt(Msg::CliDiffNone, &[("min", &fmt::bytes(d.min_bytes))]));
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("growth").striped(true).num_columns(3).show(ui, |ui| {
            for c in &d.dirs {
                let color = if c.delta_bytes > 0 { Color32::from_rgb(0xeb, 0x57, 0x57) } else { Color32::from_rgb(0x27, 0xae, 0x60) };
                ui.label(RichText::new(signed(c.delta_bytes)).monospace().color(color));
                ui.label(lang.t(match c.change {
                    Change::New => Msg::DiffNew,
                    Change::Grown => Msg::DiffGrown,
                    Change::Shrunk => Msg::DiffShrunk,
                    Change::Removed => Msg::DiffRemoved,
                }));
                ui.label(RichText::new(&c.path).monospace());
                ui.end_row();
            }
        });
        if d.more > 0 {
            ui.label(RichText::new(lang.fmt(Msg::CliDiffMore, &[("n", &d.more.to_string())])).weak());
        }
        if !d.files.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(lang.t(Msg::CliDiffFiles)).strong());
            for f in &d.files {
                ui.label(RichText::new(format!("{:>11}  {}", signed(f.delta_bytes), f.path)).monospace());
            }
        }
    });
}

/// System items: always report-only, each with the Windows tool that manages it.
fn system_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    if ui.button(lang.t(Msg::AppReadSystem)).clicked() {
        core.start_system_items();
    }
    let items = core.shared.lock().unwrap().system.clone();
    let Some(items) = items else {
        ui.label(lang.t(Msg::AppSystemNotRead));
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for it in items.iter().filter(|i| i.status != wdc_core::system::Status::Absent) {
            let size = match (it.bytes, it.status.note()) {
                (Some(b), None) => fmt::bytes(b),
                (_, Some(note)) => lang.t(note).to_string(),
                (None, None) => String::new(),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{size:>10}")).monospace().strong());
                ui.label(RichText::new(lang.t(it.kind.name())).strong());
                if let Some(p) = &it.path {
                    ui.label(RichText::new(p).monospace().weak());
                }
            });
            ui.label(RichText::new(lang.t(it.kind.managed_by())).weak());
            ui.add_space(4.0);
        }
    });
}

/// Installed programs: read-only. Each program's own uninstall command is shown for the person
/// to copy; the app never runs it. Then the folders an uninstall left behind, as candidates.
fn programs_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    use wdc_core::programs::Scope;
    if ui.button(lang.t(Msg::AppReadPrograms)).clicked() {
        core.start_programs();
    }
    let report = core.shared.lock().unwrap().programs.clone();
    let Some(report) = report else {
        ui.label(lang.t(Msg::AppProgramsNotRead));
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(
            RichText::new(lang.fmt(Msg::CliProgramsHeader, &[("n", &fmt::count(report.programs.len() as u64))])).weak(),
        );
        if report.programs.is_empty() {
            ui.label(lang.t(Msg::CliProgramsNone));
        }
        ui.add_space(4.0);
        for p in &report.programs {
            let size = p.reported_size_bytes.map(fmt::bytes).unwrap_or_else(|| lang.t(Msg::CliProgramsNoSize).to_string());
            let scope = lang.t(match p.scope {
                Scope::User => Msg::ProgScopeUser,
                Scope::Machine => Msg::ProgScopeMachine,
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{size:>10}")).monospace().strong());
                ui.label(RichText::new(&p.name).strong());
                for extra in [p.version.as_deref(), p.publisher.as_deref(), p.install_date.as_deref()].into_iter().flatten() {
                    ui.label(RichText::new(extra).weak());
                }
                ui.label(RichText::new(format!("[{scope}]")).weak());
            });
            match &p.uninstall_command {
                Some(c) => {
                    ui.add(egui::Label::new(RichText::new(c).monospace()).selectable(true));
                }
                None => {
                    ui.label(RichText::new(lang.t(Msg::CliProgramsNoCommand)).weak());
                }
            }
            if let Some(c) = &p.quiet_uninstall_command {
                ui.label(RichText::new(lang.t(Msg::AppProgramsQuietLabel)).weak());
                ui.add(egui::Label::new(RichText::new(c).monospace()).selectable(true));
            }
            ui.add_space(4.0);
        }
        ui.separator();
        for e in &report.errors {
            ui.label(RichText::new(lang.fmt(Msg::CliProgramsReadError, &[("error", e)])).color(Color32::from_rgb(0xeb, 0x57, 0x57)));
        }
        if report.first_look {
            ui.label(lang.t(Msg::CliProgramsFirstLook));
        } else if report.leftovers_checked {
            if report.leftovers.is_empty() {
                ui.label(lang.t(Msg::CliProgramsLeftoversNone));
            } else {
                ui.label(RichText::new(lang.t(Msg::CliProgramsLeftoversHeader)).strong());
                for l in &report.leftovers {
                    ui.add(egui::Label::new(RichText::new(&l.folder).monospace()).selectable(true));
                    ui.label(
                        RichText::new(l.why(lang)).weak(),
                    );
                    ui.label(RichText::new(lang.fmt(Msg::ProgLeftoverNote, &[("explain", &l.explain)])).weak());
                    ui.add_space(4.0);
                }
            }
        }
    });
}

/// "Explain": the latest explanation, by the human's "?" or by an agent.
fn explain_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    let e = core.shared.lock().unwrap().explanation.clone();
    let Some(e) = e else {
        ui.label(lang.t(Msg::AppExplainNone));
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let kind = lang.t(match e.kind {
            "folder" => Msg::KindFolder,
            "file" => Msg::KindFile,
            "link" => Msg::KindLink,
            _ => Msg::KindMissing,
        });
        ui.label(RichText::new(lang.fmt(Msg::ExplainItem, &[("item", &e.item), ("kind", kind)])).strong().monospace());
        if e.exists {
            ui.label(lang.fmt(Msg::ExplainSize, &[("size", &fmt::bytes(e.bytes_on_disk)), ("files", &fmt::count(e.files))]));
            ui.label(RichText::new(lang.fmt(Msg::ExplainFreed, &[("size", &fmt::bytes(e.expected_freed_bytes))])).strong());
        }
        for h in &e.hard_links {
            ui.label(lang.fmt(Msg::ExplainLinked, &[("size", &fmt::bytes(h.bytes_on_disk)), ("n", &h.links.to_string())]));
            for n in &h.names {
                let out = if h.names_outside.contains(n) { format!("  {}", lang.t(Msg::ExplainOutside)) } else { String::new() };
                ui.label(RichText::new(format!("    {n}{out}")).monospace());
            }
        }
        ui.separator();
        match &e.rule {
            None => {
                ui.label(RichText::new(lang.t(Msg::ExplainUnknown)).color(Color32::from_rgb(0xd9, 0x8c, 0x00)));
            }
            Some(r) => {
                let full = &r.full;
                ui.label(
                    RichText::new(lang.fmt(
                        Msg::ExplainRule,
                        &[("title", full.title(lang)), ("class", r.class.text(lang)), ("action", r.action.text(lang))],
                    ))
                    .strong(),
                );
                if let wdc_core::rules::Origin::Local(file) = &r.origin {
                    ui.label(lang.fmt(Msg::ExplainLocalRuleNote, &[("file", file)]));
                }
                ui.label(lang.fmt(Msg::ExplainEvidence, &[("text", full.evidence(lang))]));
                ui.label(RichText::new(lang.fmt(Msg::ExplainSource, &[("text", &full.source)])).weak());
                ui.label(lang.fmt(Msg::ExplainCost, &[("text", full.cost(lang))]));
                if let Some(how) = full.instructions(lang) {
                    ui.label(lang.fmt(Msg::ExplainHow, &[("text", how)]));
                }
            }
        }
        if let Some(m) = &e.manual {
            ui.label(RichText::new(lang.t(Msg::ExplainManualHeader)).strong());
            for (n, step) in m.steps(lang).iter().enumerate() {
                ui.label(format!("{}. {}", n + 1, step.text));
                if let Some(command) = &step.command {
                    // The command to copy: monospace, selectable, never translated.
                    ui.add(egui::Label::new(RichText::new(command).monospace()).selectable(true));
                }
            }
        }
        if !e.match_conditions.is_empty() || !e.require_conditions.is_empty() {
            ui.label(RichText::new(lang.t(Msg::ExplainConditions)).strong());
            for c in e.match_conditions.iter().chain(&e.require_conditions) {
                let color = if c.passed { Color32::from_rgb(0x27, 0xae, 0x60) } else { Color32::from_rgb(0xeb, 0x57, 0x57) };
                ui.label(RichText::new(format!("{} {}", if c.passed { "✔" } else { "✘" }, c.found.text(lang))).color(color));
            }
        }
        ui.separator();
        if e.actionable {
            ui.label(RichText::new(lang.t(Msg::ExplainActionable)).color(Color32::from_rgb(0x27, 0xae, 0x60)));
        } else {
            ui.label(RichText::new(lang.t(Msg::ExplainNotActionable)).strong());
            for w in &e.why_not {
                ui.label(format!("• {}", w.text(lang)));
            }
        }
    });
}

const GREEN: Color32 = Color32::from_rgb(0x27, 0xae, 0x60);
const ORANGE: Color32 = Color32::from_rgb(0xd9, 0x8c, 0x00);
const RED: Color32 = Color32::from_rgb(0xeb, 0x57, 0x57);

fn rule_of(id: &str) -> Option<wdc_core::rules::Rule> {
    wdc_core::rules::RuleSet::built_in().rules.into_iter().find(|r| r.id == id)
}

/// The items of a plan, each with its rule and evidence: what the person approves.
fn plan_items(ui: &mut egui::Ui, lang: Lang, plan: &wdc_core::plan::Plan) {
    ui.label(
        RichText::new(lang.fmt(
            Msg::CliPlanHeader,
            &[
                ("hash", plan.short_hash()),
                ("n", &plan.items.len().to_string()),
                ("size", &fmt::bytes(plan.bytes_on_disk())),
                ("freed", &fmt::bytes(plan.expected_freed_bytes())),
                ("at", &wdc_core::time::rfc3339(plan.created_at * 1000)),
            ],
        ))
        .strong(),
    );
    for i in &plan.items {
        ui.add_space(4.0);
        ui.label(RichText::new(&i.path).monospace());
        let rule = rule_of(&i.rule_id);
        let title = rule.as_ref().map(|r| r.title(lang).to_string()).unwrap_or_else(|| i.rule_id.clone());
        ui.label(format!(
            "{}  ·  {}  ·  {}  ·  {}",
            fmt::bytes(i.bytes_on_disk),
            lang.fmt(Msg::CliPlanFrees, &[("size", &fmt::bytes(i.expected_freed_bytes))]),
            title,
            i.class.text(lang)
        ));
        if let Some(r) = &rule {
            ui.label(RichText::new(lang.fmt(Msg::ExplainEvidence, &[("text", r.evidence(lang))])).weak());
            ui.label(RichText::new(lang.fmt(Msg::ExplainCost, &[("text", r.cost(lang))])).weak());
        }
    }
}

/// "Plan": the approval card when one waits, else the drafted plan and the last result.
fn plan_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let card = {
            let s = core.shared.lock().unwrap();
            s.approval.as_ref().map(|a| (a.plan.clone(), a.purge_now, a.agent.clone(), a.hello, a.busy, a.error.clone()))
        };
        if let Some((plan, purge_now, agent, hello, busy, error)) = card {
            egui::Frame::group(ui.style()).stroke(egui::Stroke::new(2.0, ORANGE)).show(ui, |ui| {
                let title = match &agent {
                    Some(a) => lang.fmt(Msg::AppApprovalTitle, &[("client", a)]),
                    None => lang.t(Msg::AppApprovalTitleYou).to_string(),
                };
                ui.heading(RichText::new(title).color(ORANGE));
                ui.label(lang.t(if purge_now { Msg::CliApplyAskPurge } else { Msg::CliApplyAsk }));
                ui.separator();
                plan_items(ui, lang, &plan);
                ui.separator();
                ui.horizontal(|ui| {
                    let label = lang.t(if hello { Msg::AppApproveHello } else { Msg::AppApproveClick });
                    if ui.add_enabled(!busy, egui::Button::new(RichText::new(label).strong())).clicked() {
                        core.approve();
                    }
                    if ui.add_enabled(!busy, egui::Button::new(lang.t(Msg::AppReject))).clicked() {
                        core.reject();
                    }
                    if busy {
                        ui.spinner();
                        ui.label(lang.t(Msg::PresenceHelloPrompt));
                    }
                });
                if let Some(e) = error {
                    ui.label(RichText::new(e).color(RED));
                }
            });
            ui.add_space(8.0);
        }

        let (draft, last, busy) = {
            let s = core.shared.lock().unwrap();
            (s.draft.clone(), s.last_apply.clone(), s.approval.is_some())
        };
        if let Some(r) = last {
            ui.label(RichText::new(lang.fmt(Msg::CliApplyMoved, &[("n", &r.moved.to_string()), ("size", &fmt::bytes(r.moved_bytes))])).strong().color(GREEN));
            for it in &r.items {
                if let Some(sk) = &it.skipped {
                    ui.label(RichText::new(lang.fmt(Msg::CliSkipped, &[("path", &it.path), ("why", &sk.text(lang))])).color(ORANGE));
                }
            }
            for v in r.volumes.iter().chain(r.purge.iter().flat_map(|p| p.volumes.iter())) {
                if let (Some(b), Some(a)) = (v.free_before, v.free_after) {
                    ui.label(lang.fmt(
                        Msg::AppFreeSpace,
                        &[("volume", &v.volume), ("before", &fmt::bytes(b)), ("after", &fmt::bytes(a))],
                    ));
                }
            }
            ui.separator();
        }
        if !busy && ui.button(lang.t(Msg::AppDraftPlan)).clicked() {
            core.start_draft();
        }
        match draft {
            None => {
                ui.label(lang.t(Msg::AppPlanNone));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(RED));
            }
            Some(Ok(d)) => {
                if d.plan.items.is_empty() {
                    ui.label(lang.t(Msg::CliPlanEmpty));
                } else {
                    plan_items(ui, lang, &d.plan);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new(lang.t(Msg::AppApplyPlan))).clicked() {
                            core.start_human_apply(false);
                        }
                        if ui.add_enabled(!busy, egui::Button::new(lang.t(Msg::AppApplyPurgeNow))).clicked() {
                            core.start_human_apply(true);
                        }
                    });
                }
                if !d.not_included.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(lang.t(Msg::CliPlanNotIncluded)).strong());
                    for n in d.not_included.iter().take(50) {
                        let why: Vec<String> = n.why_not.iter().map(|w| w.text(lang)).collect();
                        ui.label(RichText::new(format!("{}  —  {}", n.path, why.join("; "))).weak());
                    }
                }
            }
        }
    });
}

/// "Quarantine": what is held, with restore and purge.
fn quarantine_view(ui: &mut egui::Ui, lang: Lang, core: &Arc<AppCore>) {
    let (items, last, busy) = {
        let s = core.shared.lock().unwrap();
        (s.quarantine.clone().unwrap_or_default(), s.last_change.clone(), s.busy)
    };
    let held: u64 = items.iter().map(|e| e.bytes_on_disk).sum();
    ui.label(
        RichText::new(if items.is_empty() {
            lang.t(Msg::CliQuarantineEmpty).to_string()
        } else {
            lang.fmt(Msg::CliQuarantineHeld, &[("size", &fmt::bytes(held)), ("n", &items.len().to_string())])
        })
        .strong(),
    );
    if let Some(t) = last {
        ui.label(t);
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && !items.is_empty(), egui::Button::new(lang.t(Msg::AppPurgeOld))).clicked() {
            core.start_quarantine_change(None, false);
        }
        if ui.add_enabled(!busy && !items.is_empty(), egui::Button::new(lang.t(Msg::AppPurgeNow))).clicked() {
            core.start_quarantine_change(None, true);
        }
        if busy {
            ui.spinner();
        }
    });
    ui.separator();
    let now = wdc_core::time::now_unix_ms() / 1000;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for e in items.iter() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&e.id).monospace().weak());
                ui.label(format!("{:>10}", fmt::bytes(e.bytes_on_disk)));
                ui.label(RichText::new(&e.original).monospace());
                if ui.add_enabled(!busy, egui::Button::new(lang.t(Msg::AppRestore))).clicked() {
                    core.start_quarantine_change(Some(e.id.clone()), false);
                }
            });
            let title = rule_of(&e.rule_id).map(|r| r.title(lang).to_string()).unwrap_or_else(|| e.rule_id.clone());
            let days = now.saturating_sub(e.moved_at) / 86_400;
            ui.label(RichText::new(lang.fmt(Msg::CliQuarantineLine, &[("rule", &title), ("days", &days.to_string()), ("stored", &e.stored)])).weak());
        }
    });
}
