//! `WinDevCleaner.exe`: the desktop app (plan, sections 4.5 and 4.6).
//!
//! Slice 1 shows where the space is (a tree of the read scope with sizes on disk) and, when
//! an AI agent connects through `wdc mcp`, a banner with its name, every call live in the
//! activity panel, and "Pause agent" / "Disconnect agent" buttons. It redraws only when
//! something changes, so it uses no CPU while idle, and runs only while open.

#![windows_subsystem = "windows"]

mod core;
mod ui;

use std::sync::Arc;

use wdc_core::i18n::Lang;

fn main() -> eframe::Result {
    let mut lang = Lang::detect();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--lang" {
            if let Some(l) = args.get(i + 1).and_then(|v| Lang::parse(v)) {
                lang = l;
            }
            i += 1;
        }
        i += 1;
    }

    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("WinDevCleaner")
            .with_app_id("WinDevCleaner")
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    // The integrated GPU (or Windows' software renderer) is plenty for this window; never
    // wake a discrete GPU for it.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = wgpu::PowerPreference::LowPower;
        #[cfg(windows)]
        {
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
        }
    }

    eframe::run_native(
        "WinDevCleaner",
        options,
        Box::new(move |cc| {
            let data_dir = wdc_core::scope::data_dir();
            let app = Arc::new(core::AppCore::new(cc.egui_ctx.clone(), data_dir, lang));
            core::start_agent_link(&app);
            Ok(Box::new(ui::WdcApp::new(app)))
        }),
    )
}
