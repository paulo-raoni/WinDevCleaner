//! Release candidate 2, polish of the contract: what the JSON says about cut lists, partial
//! system measurements, leftovers that were not checked and the `explain` verdict. Portable:
//! no Windows API is needed.

use std::collections::HashMap;

use wdc_core::act::verdict;
use wdc_core::i18n::{Lang, Msg};
use wdc_core::programs::Report;
use wdc_core::rules::Context;
use wdc_core::scan::{scan, ScanError, ScanOptions};
use wdc_core::system::{
    folder_status, hibernation_off, is_listed, size_text, total_is_lower_bound, Kind, Status, SystemItem,
};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq};

fn empty_report() -> wdc_core::scan::ScanReport {
    let dir = fresh_dir("polish-scan");
    scan(&ScanOptions { roots: vec![dir], ..Default::default() }, None, None)
}

#[test]
fn scan_json_says_when_it_cut_the_errors_and_links_lists() {
    let mut r = empty_report();
    for i in 0..250 {
        let kind = if i % 5 == 0 { "sharing-violation" } else { "access-denied" };
        r.errors.push(ScanError { path: format!("C:\\x\\{i}"), code: 5, kind });
    }
    r.totals.errors = 433;
    let v = r.summary_json(2, 40);
    assert_eq!(v["errors"].as_array().unwrap().len(), 200);
    assert_eq!(v["errors_listed"], 200);
    guard_assert_eq!("scan-json-says-cut", v["errors_truncated"], true, "433 errors counted, 200 listed");
    // The counts by kind cover the errors that were recorded.
    assert_eq!(v["errors_by_kind"]["access-denied"], 200);
    assert_eq!(v["errors_by_kind"]["sharing-violation"], 50);

    // Links: counted 300, none recorded beyond what the list holds.
    let mut r = empty_report();
    for i in 0..220 {
        r.links.push(wdc_core::scan::LinkEntry {
            path: format!("C:\\l\\{i}"),
            kind: wdc_core::scan::LinkKind::Junction,
            directory: true,
        });
    }
    r.totals.links = 220;
    let v = r.summary_json(2, 40);
    assert_eq!(v["links_listed"], 200);
    guard_assert_eq!("scan-json-says-cut", v["links_truncated"], true, "220 links counted, 200 listed");
}

#[test]
fn scan_json_does_not_claim_a_cut_when_every_item_is_listed() {
    let mut r = empty_report();
    r.errors.push(ScanError { path: "C:\\x".into(), code: 5, kind: "access-denied" });
    r.totals.errors = 1;
    let v = r.summary_json(2, 40);
    assert_eq!(v["errors_listed"], 1);
    assert_eq!(v["errors_truncated"], false);
    assert_eq!(v["links_listed"], 0);
    assert_eq!(v["links_truncated"], false);
    assert_eq!(v["errors_by_kind"], serde_json::json!({ "access-denied": 1 }));
}

fn item(kind: Kind, bytes: Option<u64>, status: Status) -> SystemItem {
    SystemItem {
        kind,
        name: String::new(),
        path: None,
        bytes,
        items: None,
        status,
        managed_by: String::new(),
        note: None,
    }
}

#[test]
fn a_folder_with_access_errors_below_it_is_a_partial_measurement() {
    // The folder opened, but something below it was denied.
    let status = folder_status(false, &["access-denied", "sharing-violation"]);
    guard_assert_eq!("partial-needs-admin", status, Status::PartialNeedsAdmin, "access errors below an opened folder");
    // Nothing denied: measured. The folder itself unreadable: needs-admin / error, as before.
    assert_eq!(folder_status(false, &["sharing-violation"]), Status::Measured);
    assert_eq!(folder_status(false, &[]), Status::Measured);
    assert_eq!(folder_status(true, &["access-denied"]), Status::NeedsAdmin);
    assert_eq!(folder_status(true, &["not-found"]), Status::Error);
    assert_eq!(serde_json::to_value(Status::PartialNeedsAdmin).unwrap(), "partial-needs-admin");
}

#[test]
fn partial_items_are_labelled_as_lower_bounds_in_both_languages() {
    let partial = item(Kind::CrashDumps, Some(2048), Status::PartialNeedsAdmin);
    assert_eq!(size_text(Lang::En, &partial), "at least 2.00 KiB; the rest needs an administrator");
    assert_eq!(size_text(Lang::PtBr, &partial), "pelo menos 2.00 KiB; o resto precisa de um administrador");
    let whole = item(Kind::CrashDumps, Some(2048), Status::Measured);
    assert_eq!(size_text(Lang::En, &whole), "2.00 KiB");

    assert!(total_is_lower_bound(&[whole.clone(), partial]));
    assert!(!total_is_lower_bound(&[whole]));
}

#[test]
fn the_system_items_result_says_when_the_total_is_a_lower_bound() {
    // The builder the headless server and the app both call.
    let whole = item(Kind::CrashDumps, Some(2048), Status::Measured);
    let partial = item(Kind::CrashDumps, Some(2048), Status::PartialNeedsAdmin);
    let v = wdc_core::tools::system_items_value(&[whole.clone(), partial]);
    assert_eq!(v["total_is_lower_bound"], true);
    assert_eq!(v["total_bytes"], 4096);
    assert_eq!(v["items"].as_array().map(|a| a.len()), Some(2));
    assert_eq!(wdc_core::tools::system_items_value(&[whole])["total_is_lower_bound"], false);
}

#[test]
fn hibernation_off_is_an_absent_item_with_its_reason() {
    let it = hibernation_off("C:\\hiberfil.sys".into());
    assert_eq!(it.kind, Kind::HibernationFile);
    assert_eq!(it.status, Status::Absent);
    assert_eq!(it.note.as_deref(), Some("hibernation is off"));
    assert!(is_listed(&it), "the absence of the hibernation file is shown");
    assert_eq!(size_text(Lang::En, &it), "hibernation is off");
    assert_eq!(size_text(Lang::PtBr, &it), "a hibernação está desligada");
    // Any other absent item stays out of the human lists.
    assert!(!is_listed(&item(Kind::MemoryDumps, Some(0), Status::Absent)));
}

#[test]
fn programs_say_why_the_leftovers_were_not_checked() {
    // First look, the list was saved.
    let first = Report { first_look: true, ..Default::default() };
    let note = first.to_json()["leftovers_note"].as_str().unwrap().to_string();
    assert!(note.contains("first look") && note.contains("again"), "{note}");

    // Not a first look, but some keys could not be read.
    let blind = Report {
        first_look: false,
        unreadable: 3,
        errors: vec!["a".into(), "b".into(), "c".into()],
        ..Default::default()
    };
    let note = blind.leftovers_note(Lang::En);
    guard_assert!(
        "programs-keys-unreadable",
        note.as_deref().is_some_and(|n| n.contains("3 uninstall key(s) could not be read") && n.contains("not checked")),
        "the unreadable keys must be named: {note:?}"
    );
    let pt = blind.leftovers_note(Lang::PtBr).unwrap();
    assert!(pt.contains("3 chave(s)") && pt.contains("não foram verificadas"), "{pt}");
    assert_eq!(blind.to_json()["leftovers_note"], blind.leftovers_note(Lang::En).unwrap());

    // Checked: no note.
    let checked = Report { leftovers_checked: true, ..Default::default() };
    assert!(checked.to_json()["leftovers_note"].is_null());
}

fn ctx() -> Context {
    let env: HashMap<String, String> = [
        ("USERPROFILE", r"C:\Users\me"),
        ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ("APPDATA", r"C:\Users\me\AppData\Roaming"),
        ("TEMP", r"C:\Users\me\AppData\Local\Temp"),
        ("WINDIR", r"C:\Windows"),
        ("ProgramFiles", r"C:\Program Files"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    Context::new(env, None, None)
}

#[test]
fn the_verdict_tells_a_root_itself_from_a_place_inside_one() {
    let c = ctx();
    let inside = serde_json::to_value(verdict(r"C:\Users\me\AppData\Local\Temp\x", true, &c, None)).unwrap();
    guard_assert!("inside-allowed-root", inside["inside_allowed_root"] == true, "a folder in Temp is inside an allowed root: {inside}");
    assert_eq!(inside["is_root_itself"], false);
    assert_eq!(inside["checked_path"], r"C:\Users\me\AppData\Local\Temp\x");

    let root = serde_json::to_value(verdict(r"C:\Users\me\AppData\Local\Temp", true, &c, None)).unwrap();
    assert_eq!(root["is_root_itself"], true);
    assert_eq!(root["inside_allowed_root"], false, "a root is not inside itself");
    assert_eq!(root["allowed_root"], r"C:\Users\me\AppData\Local\Temp");

    let outside = serde_json::to_value(verdict(r"D:\games\x", true, &c, None)).unwrap();
    assert_eq!(outside["inside_allowed_root"], false);
    assert_eq!(outside["is_root_itself"], false);
    // The old wire name is gone.
    for v in [&inside, &root, &outside] {
        assert!(v.get("is_allowed_root").is_none(), "{v}");
    }
}

#[test]
fn the_explain_tool_description_defines_the_verdict_fields() {
    let spec = wdc_core::tools::specs().into_iter().find(|s| s.name == wdc_core::tools::EXPLAIN).unwrap();
    for field in ["checked_path", "inside_allowed_root", "is_root_itself"] {
        assert!(spec.description.contains(field), "the description must define `{field}`");
    }
    assert!(!spec.description.contains("is_allowed_root"));
}

#[test]
fn the_plan_tail_does_not_speak_of_changes() {
    for lang in [Lang::En, Lang::PtBr] {
        let line = lang.fmt(Msg::CliPlanLeftOutMore, &[("n", "523")]);
        assert!(line.contains("523") && line.contains("--json"), "{line}");
        assert!(!line.contains("--limit"), "{line}");
        assert!(!line.contains("changes") && !line.contains("mudanças"), "{line}");
    }
    assert!(Lang::En.fmt(Msg::CliPlanLeftOutMore, &[("n", "5")]).contains("left out"));
}

#[test]
fn the_help_says_what_top_limits() {
    for lang in [Lang::En, Lang::PtBr] {
        let help = lang.fmt(Msg::CliUsage, &[("version", "x")]);
        let line = help.lines().find(|l| l.trim_start().starts_with("--top N")).expect("a --top line in the help");
        assert!(line.contains("20") && line.contains("10"), "{line}");
    }
}
