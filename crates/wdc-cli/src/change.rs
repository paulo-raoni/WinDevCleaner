//! The commands that draft and apply changes (plan, sections 4.4, 5 and 7): `plan`, `plan show`,
//! `apply`, `quarantine`, `restore` and `purge`. Only `apply`, `restore` and `purge` change the
//! disk, and each needs proof that a person is present first.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wdc_core::fmt;
use wdc_core::i18n::{Lang, Msg};
use wdc_core::plan::{Draft, Plan};
use wdc_core::presence::console_proof;
use wdc_core::quarantine::{self, VolumeMeasure};
use wdc_core::rules::{Context, RuleSet};

/// Exit codes: 1 bad usage, 2 failed, 4 refused (no proof of presence, or plan refused).
const REFUSED: u8 = 4;

fn now_secs() -> u64 {
    wdc_core::time::now_unix_ms() / 1000
}

fn bad(lang: Lang, arg: &str) -> ExitCode {
    eprintln!("{}", lang.fmt(Msg::CliUnknownCommand, &[("arg", arg)]));
    ExitCode::from(1)
}

/// The line every command ends with: what the quarantine holds now.
pub fn held_line(lang: Lang, data_dir: &Path) -> String {
    let items = quarantine::list(data_dir);
    if items.is_empty() {
        return lang.t(Msg::CliQuarantineEmpty).into();
    }
    let held: u64 = items.iter().map(|e| e.bytes_on_disk).sum();
    lang.fmt(Msg::CliQuarantineHeld, &[("size", &fmt::bytes(held)), ("n", &items.len().to_string())])
}

fn rule_title(lang: Lang, id: &str) -> String {
    RuleSet::built_in().rules.iter().find(|r| r.id == id).map(|r| r.title(lang).to_string()).unwrap_or_else(|| id.into())
}

fn rule_evidence(lang: Lang, id: &str) -> String {
    RuleSet::built_in().rules.iter().find(|r| r.id == id).map(|r| r.evidence(lang).to_string()).unwrap_or_default()
}

/// The plan as a person reads it before approving: each item with its rule and evidence.
fn print_plan(lang: Lang, plan: &Plan) {
    println!(
        "{}",
        lang.fmt(
            Msg::CliPlanHeader,
            &[
                ("hash", plan.short_hash()),
                ("n", &plan.items.len().to_string()),
                ("size", &fmt::bytes(plan.bytes_on_disk())),
                ("freed", &fmt::bytes(plan.expected_freed_bytes())),
                ("at", &wdc_core::time::rfc3339(plan.created_at * 1000)),
            ]
        )
    );
    for i in &plan.items {
        println!(
            "  {:>10}  {}  {}",
            fmt::bytes(i.bytes_on_disk),
            lang.fmt(Msg::CliPlanFrees, &[("size", &fmt::bytes(i.expected_freed_bytes))]),
            i.path
        );
        println!("              {} ({})", rule_title(lang, &i.rule_id), i.class.text(lang));
        let ev = rule_evidence(lang, &i.rule_id);
        if !ev.is_empty() {
            println!("              {}", lang.fmt(Msg::ExplainEvidence, &[("text", &ev)]));
        }
    }
}

fn print_draft(lang: Lang, d: &Draft, file: &Path) {
    if d.plan.items.is_empty() {
        println!("{}", lang.t(Msg::CliPlanEmpty));
    } else {
        print_plan(lang, &d.plan);
    }
    if !d.not_included.is_empty() {
        println!("\n{}", lang.t(Msg::CliPlanNotIncluded));
        for n in d.not_included.iter().take(30) {
            let why: Vec<String> = n.why_not.iter().map(|w| w.text(lang)).collect();
            println!("  {}  [{}] {}", n.path, n.rule_id.as_deref().unwrap_or("-"), why.join("; "));
        }
        if d.not_included.len() > 30 {
            println!("{}", lang.fmt(Msg::CliDiffMore, &[("n", &(d.not_included.len() - 30).to_string())]));
        }
    }
    println!("\n{}", lang.fmt(Msg::CliPlanSaved, &[("path", &file.to_string_lossy())]));
    if !d.plan.items.is_empty() {
        println!("{}", lang.fmt(Msg::CliPlanNext, &[("path", &file.to_string_lossy())]));
    }
}

/// `wdc plan [PATH ...] [--class C] [--rule R] [-o FILE] [--json]` and `wdc plan show FILE`.
pub fn plan_command(args: &[String], lang: Lang) -> ExitCode {
    let data_dir = wdc_core::scope::data_dir();
    if args.first().map(String::as_str) == Some("show") {
        return show_command(&args[1..], lang, &data_dir);
    }
    let mut json = false;
    let mut out: Option<PathBuf> = None;
    let mut opts = wdc_core::plan::DraftOptions::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json" => json = true,
            "-o" | "--out" | "--class" | "--rule" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", a), ("value", "")]));
                    return ExitCode::from(1);
                };
                match a {
                    "--class" => match serde_json::from_value(serde_json::Value::String(v.clone())) {
                        Ok(c) => opts.classes.push(c),
                        Err(_) => {
                            eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", a), ("value", v)]));
                            return ExitCode::from(1);
                        }
                    },
                    "--rule" => opts.rules.push(v.clone()),
                    _ => out = Some(PathBuf::from(v)),
                }
                i += 1;
            }
            _ if a.starts_with("--") => return bad(lang, a),
            _ => opts.paths.push(a.to_string()),
        }
        i += 1;
    }
    let entered = wdc_core::background::enter_process_background_mode();
    let _ = entered;
    let (d, saved) = match wdc_core::tools::run_plan(&opts, &data_dir) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{}", lang.fmt(Msg::CliScanFailed, &[("error", &e)]));
            return ExitCode::from(2);
        }
    };
    let file = match &out {
        Some(o) => match d.plan.save(o) {
            Ok(()) => o.clone(),
            Err(e) => {
                eprintln!("{}", lang.fmt(Msg::CliScanFailed, &[("error", &e.to_string())]));
                return ExitCode::from(2);
            }
        },
        None => saved,
    };
    if json {
        let mut v = wdc_core::tools::plan_json(&d, &file);
        v["quarantine_held_bytes"] = quarantine::held_bytes(&data_dir).into();
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        print_draft(lang, &d, &file);
        println!("{}", held_line(lang, &data_dir));
    }
    ExitCode::SUCCESS
}

fn load_plan(lang: Lang, path: Option<&String>) -> Result<Plan, ExitCode> {
    let Some(p) = path else {
        eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", "--plan"), ("value", "")]));
        return Err(ExitCode::from(1));
    };
    Plan::load(Path::new(p)).map_err(|e| {
        eprintln!("{}", lang.fmt(Msg::CliScanFailed, &[("error", &e)]));
        ExitCode::from(2)
    })
}

fn show_command(args: &[String], lang: Lang, data_dir: &Path) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let file = args.iter().find(|a| !a.starts_with("--"));
    let plan = match load_plan(lang, file) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let refusal = plan.refusal(now_secs());
    if json {
        let v = serde_json::json!({ "plan": plan, "refused": refusal, "quarantine_held_bytes": quarantine::held_bytes(data_dir) });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        print_plan(lang, &plan);
        if let Some(r) = &refusal {
            println!("{}", lang.fmt(Msg::CliRefused, &[("why", &r.text(lang))]));
        }
        println!("{}", held_line(lang, data_dir));
    }
    ExitCode::SUCCESS
}

fn print_volumes(lang: Lang, volumes: &[VolumeMeasure]) {
    for v in volumes {
        let (Some(b), Some(a)) = (v.free_before, v.free_after) else { continue };
        let delta = a as i64 - b as i64;
        let measured = if delta < 0 { format!("-{}", fmt::bytes(delta.unsigned_abs())) } else { format!("+{}", fmt::bytes(delta as u64)) };
        println!(
            "{}",
            lang.fmt(
                Msg::CliFreeSpace,
                &[
                    ("volume", &v.volume),
                    ("before", &fmt::bytes(b)),
                    ("after", &fmt::bytes(a)),
                    ("measured", &measured),
                    ("expected", &fmt::bytes(v.expected_freed_bytes)),
                ]
            )
        );
    }
}

fn proof_or_refuse(lang: Lang, json: bool, what: &str) -> Result<wdc_core::presence::Proof, ExitCode> {
    console_proof(what, lang).map_err(|e| {
        if json {
            println!("{}", serde_json::json!({ "error": "presence", "detail": e, "changed": false }));
        } else {
            eprintln!("{}", lang.fmt(Msg::CliRefused, &[("why", &e.text(lang))]));
        }
        ExitCode::from(REFUSED)
    })
}

/// `wdc apply --plan FILE [--purge-now] [--json]`.
pub fn apply_command(args: &[String], lang: Lang) -> ExitCode {
    let data_dir = wdc_core::scope::data_dir();
    let mut json = false;
    let mut purge_now = false;
    let mut file = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--purge-now" => purge_now = true,
            "--plan" => {
                file = args.get(i + 1);
                i += 1;
            }
            other => return bad(lang, other),
        }
        i += 1;
    }
    let plan = match load_plan(lang, file) {
        Ok(p) => p,
        Err(c) => return c,
    };
    // The plan is shown in full, with each item's evidence, before anything is asked.
    if !json {
        print_plan(lang, &plan);
    }
    if let Some(r) = plan.refusal(now_secs()) {
        if json {
            println!("{}", serde_json::json!({ "error": "plan-refused", "detail": r, "changed": false }));
        } else {
            eprintln!("{}", lang.fmt(Msg::CliRefused, &[("why", &r.text(lang))]));
        }
        return ExitCode::from(REFUSED);
    }
    if plan.items.is_empty() {
        println!("{}", lang.t(Msg::CliPlanEmpty));
        return ExitCode::SUCCESS;
    }
    eprintln!("{}", lang.t(if purge_now { Msg::CliApplyAskPurge } else { Msg::CliApplyAsk }));
    let what = lang.fmt(
        Msg::CliApplyWhat,
        &[("n", &plan.items.len().to_string()), ("size", &fmt::bytes(plan.bytes_on_disk())), ("hash", plan.short_hash())],
    );
    let proof = match proof_or_refuse(lang, json, &what) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let opts = wdc_core::apply::ApplyOptions { purge_now, explained: true };
    let report = match wdc_core::apply::apply(&plan, &RuleSet::built_in(), &Context::system(), &data_dir, &opts, &proof, now_secs()) {
        Ok(r) => r,
        Err(r) => {
            eprintln!("{}", lang.fmt(Msg::CliRefused, &[("why", &r.0.text(lang))]));
            return ExitCode::from(REFUSED);
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        return ExitCode::SUCCESS;
    }
    println!("{}", lang.fmt(Msg::CliApplyMoved, &[("n", &report.moved.to_string()), ("size", &fmt::bytes(report.moved_bytes))]));
    for it in &report.items {
        if let Some(s) = &it.skipped {
            println!("  {}", lang.fmt(Msg::CliSkipped, &[("path", &it.path), ("why", &s.text(lang))]));
        }
    }
    print_volumes(lang, &report.volumes);
    if let Some(p) = &report.purge {
        print_purge(lang, p);
    } else if report.moved > 0 {
        println!("{}", lang.fmt(Msg::CliApplyNext, &[("size", &fmt::bytes(report.expected_freed_bytes))]));
    }
    println!("{}", held_line(lang, &data_dir));
    ExitCode::SUCCESS
}

fn print_purge(lang: Lang, r: &quarantine::PurgeReport) {
    let n = r.items.iter().filter(|i| i.purged).count();
    println!("{}", lang.fmt(Msg::CliPurged, &[("n", &n.to_string()), ("size", &fmt::bytes(r.purged_bytes))]));
    for i in r.items.iter().filter(|i| !i.purged) {
        if let Some(why) = &i.refused {
            println!("  {}", lang.fmt(Msg::CliSkipped, &[("path", &format!("{} ({})", i.original, i.id)), ("why", &why.text(lang))]));
        }
    }
    print_volumes(lang, &r.volumes);
}

/// `wdc quarantine [list] [--json]`.
pub fn quarantine_command(args: &[String], lang: Lang) -> ExitCode {
    let data_dir = wdc_core::scope::data_dir();
    let mut json = false;
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "list" => {}
            other => return bad(lang, other),
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&wdc_core::tools::run_quarantine(&data_dir)).unwrap());
        return ExitCode::SUCCESS;
    }
    let now = now_secs();
    for e in quarantine::list(&data_dir) {
        let days = now.saturating_sub(e.moved_at) / 86_400;
        println!("  {}  {:>10}  {}", e.id, fmt::bytes(e.bytes_on_disk), e.original);
        println!(
            "      {}",
            lang.fmt(Msg::CliQuarantineLine, &[("rule", &rule_title(lang, &e.rule_id)), ("days", &days.to_string()), ("stored", &e.stored)])
        );
    }
    println!("{}", held_line(lang, &data_dir));
    ExitCode::SUCCESS
}

/// `wdc restore ID [--json]`.
pub fn restore_command(args: &[String], lang: Lang) -> ExitCode {
    let data_dir = wdc_core::scope::data_dir();
    let json = args.iter().any(|a| a == "--json");
    let ids: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [id] = ids.as_slice() else {
        eprintln!("{}", lang.fmt(Msg::CliBadValue, &[("option", "ID"), ("value", "")]));
        return ExitCode::from(1);
    };
    let Some(entry) = quarantine::list(&data_dir).into_iter().find(|e| &e.id == *id) else {
        eprintln!("{}", wdc_core::quarantine::RestoreRefusal::UnknownId.text(lang));
        return ExitCode::from(2);
    };
    if !json {
        println!("{}", lang.fmt(Msg::CliRestoreWhat, &[("stored", &entry.stored), ("original", &entry.original)]));
    }
    let what = lang.fmt(Msg::CliRestoreWhat, &[("stored", &entry.stored), ("original", &entry.original)]);
    let proof = match proof_or_refuse(lang, json, &what) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let r = quarantine::restore(&data_dir, id, &Context::system(), &proof);
    wdc_core::apply::log(&data_dir, "restore", &serde_json::json!({ "id": id, "result": r.as_ref().map(|e| &e.original) }));
    match r {
        Ok(e) => {
            if json {
                println!("{}", serde_json::json!({ "restored": e, "quarantine_held_bytes": quarantine::held_bytes(&data_dir) }));
            } else {
                println!("{}", lang.fmt(Msg::CliRestored, &[("path", &e.original)]));
                println!("{}", held_line(lang, &data_dir));
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            if json {
                println!("{}", serde_json::json!({ "error": "restore-refused", "detail": why, "changed": false }));
            } else {
                eprintln!("{}", lang.fmt(Msg::CliRefused, &[("why", &why.text(lang))]));
            }
            ExitCode::from(REFUSED)
        }
    }
}

/// `wdc purge [--now] [ID ...] [--json]`.
pub fn purge_command(args: &[String], lang: Lang) -> ExitCode {
    let data_dir = wdc_core::scope::data_dir();
    let mut json = false;
    let mut early = false;
    let mut ids: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "--now" => early = true,
            other if other.starts_with("--") => return bad(lang, other),
            other => ids.push(other.to_string()),
        }
    }
    let held = quarantine::list(&data_dir);
    if held.is_empty() {
        println!("{}", lang.t(Msg::CliQuarantineEmpty));
        return ExitCode::SUCCESS;
    }
    if !json {
        println!("{}", held_line(lang, &data_dir));
    }
    let what = lang.fmt(Msg::CliPurgeWhat, &[("size", &fmt::bytes(held.iter().map(|e| e.bytes_on_disk).sum()))]);
    let proof = match proof_or_refuse(lang, json, &what) {
        Ok(p) => p,
        Err(c) => return c,
    };
    let filter = (!ids.is_empty()).then_some(ids.as_slice());
    let r = quarantine::purge(&data_dir, filter, early, now_secs(), &proof);
    wdc_core::apply::log(&data_dir, "purge", &r);
    if json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    } else {
        print_purge(lang, &r);
        println!("{}", held_line(lang, &data_dir));
    }
    ExitCode::SUCCESS
}
