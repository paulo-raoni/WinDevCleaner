//! `wdc.exe`: WinDevCleaner's headless command line (plan, section 4.4) and MCP server
//! (`wdc mcp`, section 4.6).

mod change;
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
        Some("explain") => explain_command(&rest[1..], lang),
        Some("programs") => programs_command(&rest[1..], lang),
        Some("plan") => change::plan_command(&rest[1..], lang),
        Some("apply") => change::apply_command(&rest[1..], lang),
        Some("quarantine") => change::quarantine_command(&rest[1..], lang),
        Some("restore") => change::restore_command(&rest[1..], lang),
        Some("purge") => change::purge_command(&rest[1..], lang),
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
    if let Some(list) = summary.get("unknown_large").and_then(|v| v.as_array()).filter(|l| !l.is_empty()) {
        println!("\n{}", lang.t(Msg::CliUnknownLarge));
        for d in list {
            println!("  {:>10}  {}", fmt::bytes(d["bytes_on_disk"].as_u64().unwrap_or(0)), d["path"].as_str().unwrap_or_default());
        }
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
    held_if_any(lang);
}

/// Every command shows what the quarantine holds, when it holds something.
fn held_if_any(lang: Lang) {
    let data_dir = wdc_core::scope::data_dir();
    if !wdc_core::quarantine::list(&data_dir).is_empty() {
        println!("{}", change::held_line(lang, &data_dir));
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

fn programs_command(args: &[String], lang: Lang) -> ExitCode {
    let mut json = false;
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            other => {
                eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", other)]));
                return ExitCode::from(1);
            }
        }
    }
    wdc_core::background::enter_process_background_mode();
    let (report, value) = wdc_core::tools::run_programs(&wdc_core::scope::data_dir());
    if json {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        print_programs(lang, &report);
    }
    ExitCode::SUCCESS
}

fn print_programs(lang: Lang, report: &wdc_core::programs::Report) {
    use wdc_core::programs::Scope;
    println!("{}", lang.fmt(Msg::CliProgramsHeader, &[("n", &fmt::count(report.programs.len() as u64))]));
    if report.programs.is_empty() {
        println!("{}", lang.t(Msg::CliProgramsNone));
    }
    for p in &report.programs {
        let size = p.reported_size_bytes.map(fmt::bytes).unwrap_or_else(|| lang.t(Msg::CliProgramsNoSize).to_string());
        let scope = lang.t(match p.scope {
            Scope::User => Msg::ProgScopeUser,
            Scope::Machine => Msg::ProgScopeMachine,
        });
        let mut line = format!("  {:>10}  {}", size, p.name);
        if let Some(v) = &p.version {
            line.push_str(&format!("  {v}"));
        }
        if let Some(publisher) = &p.publisher {
            line.push_str(&format!("  ({publisher})"));
        }
        if let Some(d) = &p.install_date {
            line.push_str(&format!("  {d}"));
        }
        line.push_str(&format!("  [{scope}]"));
        println!("{line}");
        match &p.uninstall_command {
            Some(c) => println!("              {}", lang.fmt(Msg::CliProgramsUninstall, &[("command", c)])),
            None => println!("              {}", lang.t(Msg::CliProgramsNoCommand)),
        }
        if let Some(c) = &p.quiet_uninstall_command {
            println!("              {}", lang.fmt(Msg::CliProgramsQuiet, &[("command", c)]));
        }
    }
    println!();
    for e in &report.errors {
        eprintln!("{}", lang.fmt(Msg::CliProgramsReadError, &[("error", e)]));
    }
    if report.first_look {
        println!("{}", lang.t(Msg::CliProgramsFirstLook));
    } else if report.leftovers_checked {
        if report.leftovers.is_empty() {
            println!("{}", lang.t(Msg::CliProgramsLeftoversNone));
        } else {
            println!("{}", lang.t(Msg::CliProgramsLeftoversHeader));
            for l in &report.leftovers {
                println!("  {}", l.folder);
                println!("      {}", l.why(lang));
                println!("      {}", lang.fmt(Msg::ProgLeftoverNote, &[("explain", &l.explain)]));
            }
        }
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
    held_if_any(lang);
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
        let tag = match (&c.rule, c.unknown) {
            (Some(r), _) => format!("  [{r}]"),
            (None, true) => format!("  [{}]", lang.t(Msg::DiffUnknownTag)),
            _ => String::new(),
        };
        println!("  {:>11}  {:<9} {}{tag}", signed(c.delta_bytes), change_text(lang, c.change), c.path);
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

fn explain_command(args: &[String], lang: Lang) -> ExitCode {
    let mut json = false;
    let mut path: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            other if other.starts_with("--") => {
                eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", other)]));
                return ExitCode::from(1);
            }
            other => path = Some(other),
        }
    }
    let Some(path) = path else {
        eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", "PATH"), ("value", "")]));
        return ExitCode::from(1);
    };
    let e = wdc_core::tools::run_explain(path, &wdc_core::scope::data_dir());
    if json {
        println!("{}", serde_json::to_string_pretty(&e).unwrap());
    } else {
        print_explanation(lang, &e);
        held_if_any(lang);
    }
    ExitCode::SUCCESS
}

/// The human form of an explanation, shared wording with the app.
pub fn print_explanation(lang: Lang, e: &wdc_core::explain::Explanation) {
    let kind = lang.t(match e.kind {
        "folder" => Msg::KindFolder,
        "file" => Msg::KindFile,
        "link" => Msg::KindLink,
        _ => Msg::KindMissing,
    });
    println!("{}", lang.fmt(Msg::ExplainItem, &[("item", &e.item), ("kind", kind)]));
    if wdc_core::rules::canon(&e.item).to_lowercase() != wdc_core::rules::canon(&e.path).to_lowercase() {
        println!("  {}", lang.fmt(Msg::ExplainInside, &[("path", &e.path)]));
    }
    if e.exists {
        println!(
            "  {}",
            lang.fmt(Msg::ExplainSize, &[("size", &fmt::bytes(e.bytes_on_disk)), ("files", &fmt::count(e.files))])
        );
        println!("  {}", lang.fmt(Msg::ExplainFreed, &[("size", &fmt::bytes(e.expected_freed_bytes))]));
    }
    for h in &e.hard_links {
        println!(
            "  {}",
            lang.fmt(Msg::ExplainLinked, &[("size", &fmt::bytes(h.bytes_on_disk)), ("n", &h.links.to_string())])
        );
        for n in &h.names {
            let out = if h.names_outside.contains(n) { format!("  {}", lang.t(Msg::ExplainOutside)) } else { String::new() };
            println!("      {n}{out}");
        }
    }
    match &e.rule {
        None => println!("  {}", lang.t(Msg::ExplainUnknown)),
        Some(r) => {
            let full = &r.full;
            println!(
                "  {}",
                lang.fmt(
                    Msg::ExplainRule,
                    &[("title", full.title(lang)), ("class", r.class.text(lang)), ("action", r.action.text(lang))]
                )
            );
            if let wdc_core::rules::Origin::Local(file) = &r.origin {
                println!("  {}", lang.fmt(Msg::ExplainLocalRuleNote, &[("file", file)]));
            }
            println!("  {}", lang.fmt(Msg::ExplainEvidence, &[("text", full.evidence(lang))]));
            println!("  {}", lang.fmt(Msg::ExplainSource, &[("text", &full.source)]));
            println!("  {}", lang.fmt(Msg::ExplainCost, &[("text", full.cost(lang))]));
            if let Some(how) = full.instructions(lang) {
                println!("  {}", lang.fmt(Msg::ExplainHow, &[("text", how)]));
            }
        }
    }
    if let Some(m) = &e.manual {
        println!("  {}", lang.t(Msg::ExplainManualHeader));
        for (n, step) in m.steps(lang).iter().enumerate() {
            println!("    {}. {}", n + 1, step.text);
            if let Some(command) = &step.command {
                println!("         {command}");
            }
        }
    }
    if !e.match_conditions.is_empty() || !e.require_conditions.is_empty() {
        println!("  {}", lang.t(Msg::ExplainConditions));
        for c in e.match_conditions.iter().chain(&e.require_conditions) {
            println!("    [{}] {}", if c.passed { "x" } else { " " }, c.found.text(lang));
        }
    }
    if let Some(root) = &e.act_scope.allowed_root {
        println!("  {}", lang.fmt(Msg::ExplainScope, &[("root", root)]));
    }
    if e.actionable {
        println!("  {}", lang.t(Msg::ExplainActionable));
    } else {
        println!("  {}", lang.t(Msg::ExplainNotActionable));
        for w in &e.why_not {
            println!("    - {}", w.text(lang));
        }
    }
}
