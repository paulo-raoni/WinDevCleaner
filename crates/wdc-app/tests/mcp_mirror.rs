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
    // A fake user with a VS Code cache, for the plan the agent drafts (slice 4).
    let profile = fresh_dir("mirror-profile");
    let roaming = profile.join("AppData").join("Roaming");
    let local = profile.join("AppData").join("Local");
    let temp = local.join("Temp");
    let cache = roaming.join("Code").join("CachedData");
    std::fs::create_dir_all(cache.join("v1")).unwrap();
    std::fs::create_dir_all(&temp).unwrap();
    write_file(&cache.join("v1").join("data.bin"), 2 * 1024 * 1024);
    let env: Vec<(&str, &OsStr)> = vec![
        ("WDC_DATA_DIR", data.as_os_str()),
        ("WDC_PIPE_SUFFIX", OsStr::new(&suffix)),
        ("WDC_LANG", OsStr::new("en")),
        ("USERPROFILE", profile.as_os_str()),
        ("APPDATA", roaming.as_os_str()),
        ("LOCALAPPDATA", local.as_os_str()),
        ("TEMP", temp.as_os_str()),
        // Nobody clicks in CI: the approval card gives up after 3 seconds.
        ("WDC_APPROVAL_TIMEOUT_SECS", OsStr::new("3")),
    ];

    let mut app_cmd = std::process::Command::new(env!("CARGO_BIN_EXE_WinDevCleaner"));
    for (k, v) in &env {
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
    // Slice 3: the agent asks why a folder is here; the app shows the same explanation.
    let r = mcp.call("explain", serde_json::json!({ "path": fixture.join("node_modules").to_string_lossy() }));
    assert_ne!(r["isError"], true, "{r}");
    let explained = r["structuredContent"].clone();
    assert_eq!(explained["unknown"], true, "no rule knows a test folder: {explained}");
    let r = mcp.call("system_items", serde_json::json!({}));
    assert_ne!(r["isError"], true, "{r}");
    assert!(r["structuredContent"]["items"].as_array().unwrap().iter().any(|i| i["kind"] == "recycle_bin"));

    // Slice 4: the agent drafts a plan and asks the person to approve it. The app shows the
    // approval card; nobody approves in CI, so it times out and nothing changes.
    let r = mcp.call("plan", serde_json::json!({ "paths": [cache.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    assert_eq!(r["structuredContent"]["items"].as_array().unwrap().len(), 1, "{r}");
    let hash = r["structuredContent"]["hash"].as_str().unwrap().to_string();
    let r = mcp.call("request_apply", serde_json::json!({ "plan": hash }));
    assert_ne!(r["isError"], true, "{r}");
    assert_eq!(r["structuredContent"]["status"], "timed-out", "{r}");
    assert_eq!(r["structuredContent"]["changed"], false);
    assert!(cache.join("v1").join("data.bin").exists(), "nothing moved without the person");

    // The app's activity log shows the agent and each of its calls, with the same result.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut log = activity(&data);
    while Instant::now() < deadline && log.iter().filter(|e| e["status"] == "done").count() < 8 {
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
    let explain_done = log.iter().find(|e| e["kind"] == "call" && e["tool"] == "explain" && e["status"] == "done");
    guard_assert!("activity-mirror", explain_done.is_some(), "the agent's explain appears in the app's activity log");
    assert_eq!(explain_done.unwrap()["data"]["expected_freed_bytes"], explained["expected_freed_bytes"], "the app shows the same answer");
    guard_assert!(
        "activity-mirror",
        log.iter().any(|e| e["kind"] == "call" && e["tool"] == "system_items" && e["status"] == "done"),
        "the agent's system items call appears too"
    );
    let asked = log.iter().find(|e| e["kind"] == "call" && e["tool"] == "request_apply" && e["status"] == "done");
    assert!(asked.is_some(), "the request to apply appears in the app's activity log");
    assert_eq!(asked.unwrap()["data"]["status"], "timed-out");
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
    wdc_testkit::remove_tree(&profile);
}

/// The client started while the app was closed; the person opens the app afterwards. The next
/// call goes through the app (mirrored in its activity log) and says so.
#[test]
#[ignore]
fn an_app_opened_after_the_client_started_gets_the_next_call() {
    let fixture = fresh_dir("late-app-fixture");
    write_file(&fixture.join("a.bin"), 1024 * 1024);
    let data = fresh_dir("late-app-data");
    let suffix = format!("-late-app-test-{}", std::process::id());
    let pipe = wdc_core::link::pipe::pipe_name_with_suffix(&suffix).unwrap();
    let env: Vec<(&str, &OsStr)> = vec![
        ("WDC_DATA_DIR", data.as_os_str()),
        ("WDC_PIPE_SUFFIX", OsStr::new(&suffix)),
        ("WDC_LANG", OsStr::new("en")),
    ];

    // The agent's session starts with the app closed.
    let mut mcp = McpClient::start(&target_bin("wdc"), &[], &env);
    let init = mcp.initialize("wdc-late-client");
    assert!(init["instructions"].as_str().unwrap().contains("not watching"), "{init}");
    let r = mcp.call("quarantine", serde_json::json!({}));
    assert_eq!(r["structuredContent"]["app"], "headless", "{r}");

    // The person opens the app.
    let mut app_cmd = std::process::Command::new(env!("CARGO_BIN_EXE_WinDevCleaner"));
    for (k, v) in &env {
        app_cmd.env(k, v);
    }
    let mut app = KillOnDrop(app_cmd.spawn().expect("start the app"));
    assert!(wait_for_pipe(&pipe, Duration::from_secs(60)), "the app did not open its agent pipe");
    assert!(app.0.try_wait().unwrap().is_none(), "the app exited early");

    // The next call reaches it.
    let r = mcp.call("scan", serde_json::json!({ "roots": [fixture.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    guard_assert!("mcp-upgrade", r["structuredContent"]["app"] == "connected", "the call went through the app: {r}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut log = activity(&data);
    while Instant::now() < deadline
        && !log.iter().any(|e| e["kind"] == "call" && e["tool"] == "scan" && e["status"] == "done")
    {
        std::thread::sleep(Duration::from_millis(100));
        log = activity(&data);
    }
    guard_assert!(
        "mcp-upgrade",
        log.iter().any(|e| e["kind"] == "connected" && e["client"] == "wdc-late-client")
            && log.iter().any(|e| e["kind"] == "call" && e["tool"] == "scan" && e["status"] == "done"),
        "the app's activity log shows the agent and its scan: {log:?}"
    );
    // Once attached it stays attached; the headless scan is gone, so `tree` says to scan again.
    let r = mcp.call("tree", serde_json::json!({ "path": fixture.to_string_lossy() }));
    assert_eq!(r["structuredContent"]["app"], "connected", "{r}");
    assert_eq!(mcp.close(), Some(0));
    assert!(app.0.try_wait().unwrap().is_none(), "the app is still running");
    drop(app);
    wdc_testkit::remove_tree(&fixture);
}

/// An agent the person disconnected in the app gets errors from then on, and never a headless
/// result: its results say `blocked` with the reason.
#[test]
fn a_disconnected_agent_gets_errors_and_never_headless_results() {
    use std::sync::atomic::AtomicBool;
    use wdc_core::link::pipe::{accept, create_instance};
    use wdc_core::link::{serve_session, AgentHandler, LinkControl};

    struct Echo;
    impl AgentHandler for Echo {
        fn connected(&self, _: &str) {}
        fn disconnected(&self, _: &str, _: &str) {}
        fn call_started(&self, _: &str, _: &str, _: &serde_json::Value, _: bool) -> u64 {
            1
        }
        fn call_resumed(&self, _: u64) {}
        fn call(&self, tool: &str, _: &serde_json::Value, _: &AtomicBool) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({ "tool": tool }))
        }
        fn call_finished(&self, _: u64, _: &Result<serde_json::Value, String>) {}
    }

    let data = fresh_dir("disconnected-data");
    let suffix = format!("-disconnected-test-{}", std::process::id());
    let pipe = wdc_core::link::pipe::pipe_name_with_suffix(&suffix).unwrap();
    let instance = create_instance(&pipe, true).expect("create pipe");
    let control = LinkControl::new();
    let served = control.clone();
    let server = std::thread::spawn(move || {
        let mut conn = accept(instance).expect("accept");
        serve_session(&mut conn, &served, &Echo);
    });
    let env: Vec<(&str, &OsStr)> =
        vec![("WDC_DATA_DIR", data.as_os_str()), ("WDC_PIPE_SUFFIX", OsStr::new(&suffix))];
    let mut mcp = McpClient::start(&target_bin("wdc"), &[], &env);
    let init = mcp.initialize("wdc-disconnect-client");
    assert!(init["instructions"].as_str().unwrap().contains("watches every call live"), "{init}");
    let r = mcp.call("quarantine", serde_json::json!({}));
    guard_assert!("mcp-app-field", r["structuredContent"]["app"] == "connected", "{r}");

    control.disconnect();
    for _ in 0..3 {
        let r = mcp.call("quarantine", serde_json::json!({}));
        assert_eq!(r["isError"], true, "a disconnected agent gets an error: {r}");
        guard_assert!("mcp-app-field", r["structuredContent"]["app"] == "blocked", "{r}");
        assert!(r["structuredContent"]["app_reason"].as_str().unwrap().contains("disconnected"), "{r}");
        assert!(r["structuredContent"].get("held_bytes").is_none(), "no headless result: {r}");
    }
    assert_eq!(mcp.close(), Some(0));
    let _ = server.join();
    wdc_testkit::remove_tree(&data);
}
