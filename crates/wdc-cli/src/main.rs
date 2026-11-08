//! `wdc.exe`: WinDevCleaner's headless command line (plan, section 4.4) and MCP server
//! (`wdc mcp`, section 4.6). Read-only in slice 1.

mod mcp;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use wdc_core::fmt;
use wdc_core::i18n::{Lang, Msg};
use wdc_core::tools::{run_scan, ScanArgs};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut lang = Lang::detect();
    // `--lang` may appear anywhere.
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--lang" {
            match args.get(i + 1).and_then(|v| Lang::parse(v)) {
                Some(l) => lang = l,
                None => {
                    let v = args.get(i + 1).cloned().unwrap_or_default();
                    eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", "--lang"), ("value", &v)]));
                    return ExitCode::from(1);
                }
            }
            i += 2;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }

    match rest.first().map(String::as_str) {
        None | Some("--help") | Some("-h") | Some("help") => {
            print!("{}", lang.fmt(Msg::CliUsage, &[("version", wdc_core::VERSION)]));
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") => {
            println!("wdc {}", wdc_core::VERSION);
            ExitCode::SUCCESS
        }
        Some("scan") => scan_command(&rest[1..], lang),
        Some("diff") => diff_command(&rest[1..], lang),
        Some("mcp") => {
            let show = match rest[1..].iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                [] => false,
                ["--show"] => true,
                [other, ..] => {
                    eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", other)]));
                    return ExitCode::from(1);
                }
            };
            mcp::run(show)
        }
        Some(other) => {
            eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", other)]));
            ExitCode::from(1)
        }
    }
}

fn parse_number(lang: Lang, option: &str, value: Option<&String>) -> Result<u64, ExitCode> {
    match value.and_then(|v| v.parse::<u64>().ok()) {
        Some(n) => Ok(n),
        None => {
            let v = value.cloned().unwrap_or_default();
            eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", option), ("value", &v)]));
            Err(ExitCode::from(1))
        }
    }
}

fn scan_command(args: &[String], lang: Lang) -> ExitCode {
    let mut json = false;
    let mut throttle = None;
    let mut top = None;
    let mut depth = 2u16;
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json" => json = true,
            "--throttle" | "--top" | "--depth" => {
                let n = match parse_number(lang, a, args.get(i + 1)) {
                    Ok(n) => n,
                    Err(code) => return code,
                };
                match a {
                    "--throttle" => throttle = Some(Duration::from_millis(n.min(10_000))),
                    "--top" => top = Some(n.min(1000) as usize),
                    _ => depth = n.clamp(1, 10) as u16,
                }
                i += 1;
            }
            _ if a.starts_with("--") => {
                eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", a)]));
                return ExitCode::from(1);
            }
            _ => roots.push(PathBuf::from(a)),
        }
        i += 1;
    }

    // wdc.exe is a background tool: lower CPU, I/O and memory priority for the whole run.
    let entered = wdc_core::background::enter_process_background_mode();

    let mut scan_args = match wdc_core::tools::parse_scan_args(&if roots.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!({ "roots": roots.iter().map(|r| r.to_string_lossy()).collect::<Vec<_>>() })
    }) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", lang.fmt(Msg::CliScanFailed, &[("error", &e)]));
            return ExitCode::from(1);
        }
    };
    scan_args.options.throttle = throttle;
    if let Some(t) = top {
        scan_args.options.top_files = t;
    }
    scan_args.depth = depth;

    if !json {
        eprintln!("{}", lang.fmt(Msg::CliScanStarting, &[("roots", &scan_args.target)]));
    }
    let data_dir = wdc_core::scope::data_dir();
    let (report, mut summary) = run_scan(&scan_args, &data_dir, None, None);
    let background = wdc_core::background::process_state(entered);

    if json {
        summary["tool"] = "wdc".into();
        summary["version"] = wdc_core::VERSION.into();
        summary["process"] = process_json(background);
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    } else {
        print_human(lang, &scan_args, &report, &summary);
    }
    if report.roots.iter().all(|r| r.node.is_none() && !r.is_link) {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

fn process_json(background: wdc_core::background::BackgroundState) -> serde_json::Value {
    #[cfg(windows)]
    let mem = wdc_core::win::own_memory().unwrap_or_default();
    #[cfg(windows)]
    let mem = serde_json::json!({
        "peak_working_set_bytes": mem.peak_working_set,
        "peak_private_bytes": mem.peak_private_bytes,
    });
    #[cfg(not(windows))]
    let mem = serde_json::Value::Null;
    serde_json::json!({ "background_mode": background, "memory": mem })
}

fn print_human(lang: Lang, args: &ScanArgs, report: &wdc_core::scan::ScanReport, summary: &serde_json::Value) {
    let t = &report.totals;
    println!(
        "{}",
        lang.fmt(
            Msg::CliScanDone,
            &[
                ("files", &fmt::count(t.files)),
                ("dirs", &fmt::count(t.dirs)),
                ("secs", &format!("{:.1}", report.duration_ms as f64 / 1000.0)),
            ]
        )
    );
    println!("{}", lang.fmt(Msg::CliSizeOnDisk, &[("size", &fmt::bytes(t.bytes_on_disk))]));
    for r in &report.roots {
        if let Some(e) = &r.error {
            println!("  {}: {e}", r.path);
        } else {
            println!("  {:>10}  {}", fmt::bytes(r.bytes_on_disk), r.path);
        }
    }

    let dirs = report.largest_dirs(args.depth, 20);
    if !dirs.is_empty() {
        println!("\n{}", lang.t(Msg::CliLargestFolders));
        for (path, bytes, _) in dirs {
            println!("  {:>10}  {path}", fmt::bytes(bytes));
        }
    }
    if !report.top_files.is_empty() {
        println!("\n{}", lang.t(Msg::CliLargestFiles));
        for f in report.top_files.iter().take(10) {
            let note = if f.links > 1 {
                format!("  {}", lang.fmt(Msg::CliHardLinkNote, &[("links", &f.links.to_string())]))
            } else {
                String::new()
            };
            println!("  {:>10}  {}{note}", fmt::bytes(f.bytes_on_disk), f.path);
        }
    }
    println!();
    let n = |v: u64| fmt::count(v);
    if t.links > 0 {
        println!("{}", lang.fmt(Msg::CliLinks, &[("n", &n(t.links))]));
    }
    if t.hardlink_duplicates > 0 {
        println!("{}", lang.fmt(Msg::CliHardLinkDupes, &[("n", &n(t.hardlink_duplicates))]));
    }
    if t.placeholders > 0 {
        println!("{}", lang.fmt(Msg::CliPlaceholders, &[("n", &n(t.placeholders))]));
    }
    if t.excluded > 0 {
        println!("{}", lang.fmt(Msg::CliExcluded, &[("n", &n(t.excluded))]));
    }
    if t.errors > 0 {
        println!("{}", lang.fmt(Msg::CliErrors, &[("n", &n(t.errors))]));
    }
    if let Some(items) = summary.get("system_items").and_then(|v| v.as_array()) {
        println!("\n{}", lang.t(Msg::CliSystemItems));
        print_system_items(lang, items);
        println!();
    }
    match summary.get("snapshot") {
        Some(s) if s.get("path").is_some() => println!(
            "{}",
            lang.fmt(
                Msg::CliSnapshotSaved,
                &[
                    ("path", s["path"].as_str().unwrap_or_default()),
                    ("kept", &s["kept"].to_string()),
                    ("size", &fmt::bytes(s["total_bytes"].as_u64().unwrap_or(0))),
                ]
            )
        ),
        Some(s) => println!(
            "{}",
            lang.fmt(Msg::CliSnapshotFailed, &[("error", s["error"].as_str().unwrap_or_default())])
        ),
        None => println!("{}", lang.t(Msg::CliCancelled)),
    }
}

fn print_system_items(lang: Lang, items: &[serde_json::Value]) {
    use wdc_core::system::{Kind, Status};
    for it in items {
        let (Ok(kind), Ok(status)) = (
            serde_json::from_value::<Kind>(it["kind"].clone()),
            serde_json::from_value::<Status>(it["status"].clone()),
        ) else {
            continue;
        };
        if status == Status::Absent {
            continue;
        }
        let size = match (it["bytes"].as_u64(), status.note()) {
            (Some(b), None) => fmt::bytes(b),
            (_, Some(note)) => lang.t(note).to_string(),
            (None, None) => String::new(),
        };
        let path = it["path"].as_str().map(|p| format!("  {p}")).unwrap_or_default();
        println!("  {:>10}  {}{path}", size, lang.t(kind.name()));
        println!("              {}", lang.t(kind.managed_by()));
    }
}

fn diff_command(args: &[String], lang: Lang) -> ExitCode {
    let mut json = false;
    let mut from: Option<PathBuf> = None;
    let mut to: Option<PathBuf> = None;
    let mut opts = wdc_core::diff::DiffOptions::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json" => json = true,
            "--from" | "--to" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", a), ("value", "")]));
                    return ExitCode::from(1);
                };
                if a == "--from" {
                    from = Some(PathBuf::from(v));
                } else {
                    to = Some(PathBuf::from(v));
                }
                i += 1;
            }
            "--min-mb" | "--limit" => {
                let n = match parse_number(lang, a, args.get(i + 1)) {
                    Ok(n) => n,
                    Err(code) => return code,
                };
                if a == "--min-mb" {
                    opts.min_bytes = n.saturating_mul(1024 * 1024);
                } else {
                    opts.limit = n.clamp(1, 1000) as usize;
                }
                i += 1;
            }
            _ => {
                eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", a)]));
                return ExitCode::from(1);
            }
        }
        i += 1;
    }
    let data_dir = wdc_core::scope::data_dir();
    let d = match wdc_core::diff::diff_snapshots(&data_dir, from.as_deref(), to.as_deref(), opts) {
        Ok(d) => d,
        Err(wdc_core::diff::DiffError::NotEnoughSnapshots) => {
            if json {
                println!("{}", serde_json::json!({ "error": "need two scans of the same folders" }));
            } else {
                eprintln!("{}", lang.t(Msg::CliDiffNeedTwo));
            }
            return ExitCode::from(3);
        }
        Err(e) => {
            eprintln!("{}", lang.fmt(Msg::CliScanFailed, &[("error", &e.to_string())]));
            return ExitCode::from(2);
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&d).unwrap());
        return ExitCode::SUCCESS;
    }
    print_diff(lang, &d);
    ExitCode::SUCCESS
}

/// Signed size: "+1.50 GiB", "-20.0 MiB".
fn signed(delta: i64) -> String {
    let sign = if delta < 0 { "-" } else { "+" };
    format!("{sign}{}", fmt::bytes(delta.unsigned_abs()))
}

fn change_text(lang: Lang, c: wdc_core::diff::Change) -> &'static str {
    use wdc_core::diff::Change;
    lang.t(match c {
        Change::New => Msg::DiffNew,
        Change::Grown => Msg::DiffGrown,
        Change::Shrunk => Msg::DiffShrunk,
        Change::Removed => Msg::DiffRemoved,
    })
}

fn print_diff(lang: Lang, d: &wdc_core::diff::Diff) {
    println!(
        "{}",
        lang.fmt(Msg::CliDiffHeader, &[("from", &d.from.created), ("to", &d.to.created), ("delta", &signed(d.delta_bytes))])
    );
    if d.dirs.is_empty() {
        println!("{}", lang.fmt(Msg::CliDiffNone, &[("min", &fmt::bytes(d.min_bytes))]));
    }
    for c in &d.dirs {
        println!("  {:>11}  {:<9} {}", signed(c.delta_bytes), change_text(lang, c.change), c.path);
    }
    if d.more > 0 {
        println!("{}", lang.fmt(Msg::CliDiffMore, &[("n", &d.more.to_string())]));
    }
    if !d.files.is_empty() {
        println!("\n{}", lang.t(Msg::CliDiffFiles));
        for f in &d.files {
            println!("  {:>11}  {:<9} {}", signed(f.delta_bytes), change_text(lang, f.change), f.path);
        }
    }
}
