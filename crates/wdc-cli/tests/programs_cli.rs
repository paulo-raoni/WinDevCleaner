//! Slice 7 (plan, section 8): `wdc programs`. The registry test reads the runner's real
//! registry (Windows only); the rest runs everywhere and checks the command's shape.

use std::path::Path;
use std::process::{Command, Output};

use wdc_testkit::fresh_dir;

fn wdc(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wdc")).args(args).env("WDC_DATA_DIR", data).output().unwrap()
}

fn json(data: &Path) -> serde_json::Value {
    let out = wdc(data, &["programs", "--json"]);
    assert!(out.status.success(), "wdc programs --json: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("JSON output")
}

#[cfg(windows)]
#[test]
fn programs_json_on_the_real_registry_parses_and_every_entry_has_a_name_and_a_scope() {
    let data = fresh_dir("programs-cli-real");
    let v = json(&data);
    let list = v["programs"].as_array().expect("programs is a list");
    println!("{} programs", list.len());
    assert!(!list.is_empty(), "the runner has installed programs");
    for p in list {
        assert!(p["name"].as_str().is_some_and(|n| !n.trim().is_empty()), "{p}");
        assert!(matches!(p["scope"].as_str(), Some("user") | Some("machine")), "{p}");
        // A missing value is absent, never an empty string.
        for k in ["publisher", "version", "install_date", "install_location", "uninstall_command", "quiet_uninstall_command"] {
            assert!(p.get(k).is_none_or(|x| x.is_null() || x.as_str().is_some_and(|s| !s.is_empty())), "{k} of {p}");
        }
    }
    assert!(v["leftovers"].is_array(), "the leftovers field is present");
    assert_eq!(v["errors"], serde_json::json!([]));
    assert_eq!(v["first_look"], true);
    // The second look compares with the first one: the same machine leaves nothing behind.
    let again = json(&data);
    assert_eq!(again["first_look"], false);
    assert_eq!(again["leftovers_checked"], true);
    assert!(again["leftovers"].as_array().unwrap().is_empty(), "{again}");
}

#[test]
fn programs_has_a_stable_english_json_shape_and_a_human_form_in_both_languages() {
    let data = fresh_dir("programs-cli-shape");
    let v = json(&data);
    for k in ["count", "reported_bytes_total", "size_note", "note", "programs", "first_look", "leftovers_checked", "leftovers", "errors"] {
        assert!(v.get(k).is_some(), "`{k}` is missing from {v}");
    }
    assert!(v["size_note"].as_str().unwrap().contains("installer"));
    // The list of the look was saved for the next one.
    assert!(data.join("programs.json").is_file());

    let en = wdc(&data, &["--lang", "en", "programs"]);
    assert!(en.status.success());
    let en = String::from_utf8_lossy(&en.stdout).to_string();
    assert!(en.contains("Installed programs"), "{en}");
    assert!(en.contains("never runs an uninstall command"), "{en}");
    let pt = wdc(&data, &["--lang", "pt-BR", "programs"]);
    assert!(pt.status.success());
    let pt = String::from_utf8_lossy(&pt.stdout).to_string();
    assert!(pt.contains("Programas instalados"), "{pt}");
    assert!(pt.contains("nunca executa"), "{pt}");

    let bad = wdc(&data, &["programs", "--remove"]);
    assert_eq!(bad.status.code(), Some(1));
}

#[test]
fn the_programs_tool_answers_headless_with_the_same_json() {
    let data = fresh_dir("programs-mcp");
    let mut c = wdc_testkit::McpClient::start(
        Path::new(env!("CARGO_BIN_EXE_wdc")),
        &[],
        &[
            ("WDC_DATA_DIR", data.as_os_str()),
            // No app instance can answer on this pipe name: headless for sure.
            ("WDC_PIPE_SUFFIX", std::ffi::OsStr::new("-programs-test")),
        ],
    );
    c.initialize("programs-test-client");
    let tools = c.request("tools/list", serde_json::json!({}));
    let spec = tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == "programs").expect("the programs tool is listed").clone();
    assert!(spec["description"].as_str().unwrap().contains("Read-only"));
    let r = c.call("programs", serde_json::json!({}));
    assert_ne!(r["isError"], true, "{r}");
    let s = &r["structuredContent"];
    assert!(s["programs"].is_array() && s["leftovers"].is_array(), "{s}");
    assert_eq!(s["first_look"], true);
    assert_eq!(c.close(), Some(0));
}
