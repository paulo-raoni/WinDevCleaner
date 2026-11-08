//! An MCP client drives a scan through `wdc mcp` while the app is open, and the scan is
//! mirrored in the app's activity log (plan, section 8, slice 1 row). Windows only, needs a
//! desktop session (the CI runner has one); run with `--ignored`.
#![cfg(windows)]

use std::ffi::OsStr;
use std::time::{Duration, Instant};

use wdc_testkit::{facts_by_handle, fresh_dir, guard_assert, target_bin, wait_for_pipe, write_file, McpClient};

struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn activity(data: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(data.join("activity.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[test]
#[ignore]
fn an_mcp_client_drives_a_scan_and_the_app_mirrors_it_live() {
    let fixture = fresh_dir("mirror-fixture");
    std::fs::create_dir_all(fixture.join("node_modules").join("big-package")).unwrap();
    let big = fixture.join("node_modules").join("big-package").join("bundle.js");
    write_file(&big, 3 * 1024 * 1024);
    let small = fixture.join("readme.txt");
    write_file(&small, 10_000);
    let expected = facts_by_handle(&big).allocation + facts_by_handle(&small).allocation;

    let data = fresh_dir("mirror-data");
    let suffix = format!("-mirror-test-{}", std::process::id());
    let pipe = wdc_core::link::pipe::pipe_name_with_suffix(&suffix).unwrap();
    let env: [(&str, &OsStr); 3] =
        [("WDC_DATA_DIR", data.as_os_str()), ("WDC_PIPE_SUFFIX", OsStr::new(&suffix)), ("WDC_LANG", OsStr::new("en"))];

    let mut app_cmd = std::process::Command::new(env!("CARGO_BIN_EXE_WinDevCleaner"));
    for (k, v) in env {
        app_cmd.env(k, v);
    }
    let mut app = KillOnDrop(app_cmd.spawn().expect("start the app"));
    let started = Instant::now();
    assert!(wait_for_pipe(&pipe, Duration::from_secs(60)), "the app did not open its agent pipe");
    println!("app listening after {:?}", started.elapsed());
    assert!(app.0.try_wait().unwrap().is_none(), "the app exited early");

    let mut mcp = McpClient::start(&target_bin("wdc"), &[], &env);
    mcp.initialize("wdc-ci-client");
    let r = mcp.call("scan", serde_json::json!({ "roots": [fixture.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    let bytes = r["structuredContent"]["totals"]["bytes_on_disk"].as_u64().unwrap();
    assert_eq!(bytes, expected);
    let r = mcp.call("tree", serde_json::json!({ "path": fixture.join("node_modules").to_string_lossy() }));
    assert_eq!(r["structuredContent"]["children"][0]["name"], "big-package", "{r}");

    // The fixture grows; the agent scans again and asks what grew (slice 2).
    let added = fixture.join("node_modules").join("new-package").join("dist").join("index.js");
    std::fs::create_dir_all(added.parent().unwrap()).unwrap();
    write_file(&added, 2 * 1024 * 1024);
    let r = mcp.call("scan", serde_json::json!({ "roots": [fixture.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    let r = mcp.call("diff", serde_json::json!({}));
    assert_ne!(r["isError"], true, "{r}");
    let changes = r["structuredContent"]["dirs"].as_array().unwrap().clone();
    assert!(
        changes.iter().any(|c| c["change"] == "new" && c["path"].as_str().unwrap().ends_with("new-package")),
        "{changes:?}"
    );
    let r = mcp.call("system_items", serde_json::json!({}));
    assert_ne!(r["isError"], true, "{r}");
    assert!(r["structuredContent"]["items"].as_array().unwrap().iter().any(|i| i["kind"] == "recycle_bin"));

    // The app's activity log shows the agent and each of its calls, with the same result.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut log = activity(&data);
    while Instant::now() < deadline && log.iter().filter(|e| e["status"] == "done").count() < 5 {
        std::thread::sleep(Duration::from_millis(100));
        log = activity(&data);
    }
    println!("{}", serde_json::to_string_pretty(&log).unwrap());
    assert!(
        log.iter().any(|e| e["kind"] == "connected" && e["client"] == "wdc-ci-client"),
        "the app shows the agent connected"
    );
    let scan_done = log.iter().find(|e| e["kind"] == "call" && e["tool"] == "scan" && e["status"] == "done");
    guard_assert!("activity-mirror", scan_done.is_some(), "the agent's scan appears in the app's activity log");
    let scan_done = scan_done.unwrap();
    assert_eq!(scan_done["client"], "wdc-ci-client");
    assert_eq!(scan_done["data"]["totals"]["bytes_on_disk"], bytes, "the app shows the same result");
    assert!(scan_done["target"].as_str().unwrap().contains("mirror-fixture"));
    guard_assert!(
        "activity-mirror",
        log.iter().any(|e| e["kind"] == "call" && e["tool"] == "tree" && e["status"] == "done"),
        "the agent's drill-down appears too"
    );
    let diff_done = log.iter().find(|e| e["kind"] == "call" && e["tool"] == "diff" && e["status"] == "done");
    guard_assert!("activity-mirror", diff_done.is_some(), "the agent's diff appears in the app's activity log");
    assert_eq!(diff_done.unwrap()["data"]["changes"].as_u64().unwrap() as usize, changes.len(), "the app shows the same diff");
    guard_assert!(
        "activity-mirror",
        log.iter().any(|e| e["kind"] == "call" && e["tool"] == "system_items" && e["status"] == "done"),
        "the agent's system items call appears too"
    );
    // The scan went through the app, not headless: it ran while the app was open and the
    // running entry came before the finished one.
    let running = log.iter().position(|e| e["kind"] == "call" && e["tool"] == "scan" && e["status"] == "running");
    assert!(running.is_some());

    assert_eq!(mcp.close(), Some(0));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !activity(&data).iter().any(|e| e["kind"] == "disconnected") {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(activity(&data).iter().any(|e| e["kind"] == "disconnected"), "the app shows the agent left");
    assert!(app.0.try_wait().unwrap().is_none(), "the app is still running");
    drop(app);
    wdc_testkit::remove_tree(&fixture);
}
