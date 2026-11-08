//! The window. It reads the shared state each frame; egui redraws only on input or when a
//! worker asks for it, so an idle window uses no CPU.

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};

use wdc_core::activity::{Entry, Status, What};
use wdc_core::fmt;
use wdc_core::i18n::{Lang, Msg};
use wdc_core::scan::ScanReport;

use crate::core::{AppCore, ScanBy};

/// Children shown per folder before "... and N more".
const MAX_CHILDREN: usize = 300;

pub struct WdcApp {
    core: Arc<AppCore>,
    seen_focus: u64,
    /// Folders to force open once, so the view follows the agent.
    open_path: HashSet<u32>,
    focus_node: Option<u32>,
}

impl WdcApp {
    pub fn new(core: Arc<AppCore>) -> WdcApp {
        WdcApp { core, seen_focus: 0, open_path: HashSet::new(), focus_node: None }
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
            ui.heading(lang.t(Msg::AppWhereIsMySpace));
            ui.separator();
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
        if kids.is_empty() {
            let resp = ui.label(label);
            if focused && force {
                resp.scroll_to_me(Some(egui::Align::Center));
            }
            return;
        }
        let mut header = egui::CollapsingHeader::new(label).id_salt(("dir", n));
        if force {
            header = header.open(Some(true));
        }
        if d.parent == wdc_core::scan::NO_PARENT {
            header = header.default_open(true);
        }
        let resp = header.show(ui, |ui| {
            for &k in kids.iter().take(MAX_CHILDREN) {
                self.node(ui, lang, report, children, k, d.bytes);
            }
            if kids.len() > MAX_CHILDREN {
                ui.label(RichText::new(lang.fmt(Msg::AppMoreItems, &[("n", &fmt::count((kids.len() - MAX_CHILDREN) as u64))])).weak());
            }
        });
        if focused && force {
            resp.header_response.scroll_to_me(Some(egui::Align::Center));
        }
    }
}
