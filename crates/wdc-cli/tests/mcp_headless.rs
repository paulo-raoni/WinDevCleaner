//! `wdc mcp` with the app closed: the engine runs headless and answers over MCP stdio.
//! Platform-independent (it also runs in the Linux cloud session).

use wdc_testkit::{fresh_dir, guard_assert, McpClient};

#[test]
fn mcp_client_scans_and_drills_down_headless() {
    let tree = fresh_dir("mcp-headless-tree");
    std::fs::create_dir_all(tree.join("cache").join("deep")).unwrap();
    std::fs::write(tree.join("cache").join("deep").join("blob.bin"), vec![1u8; 2 * 1024 * 1024]).unwrap();
    let data = fresh_dir("mcp-headless-data");

    let mut c = McpClient::start(
        std::path::Path::new(env!("CARGO_BIN_EXE_wdc")),
        &[],
        &[
            ("WDC_DATA_DIR", data.as_os_str()),
            // No app instance can answer on this pipe name: headless for sure.
            ("WDC_PIPE_SUFFIX", std::ffi::OsStr::new("-headless-test")),
        ],
    );
    let init = c.initialize("headless-test-client");
    assert_eq!(init["serverInfo"]["name"], "wdc");
    let tools = c.request("tools/list", serde_json::json!({}));
    let names: Vec<_> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, vec!["scan", "tree", "diff", "explain", "system_items", "programs", "plan", "quarantine", "request_apply"]);

    let r = c.call("scan", serde_json::json!({ "roots": [tree.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    let s = &r["structuredContent"];
    assert_eq!(s["totals"]["files"], 1);
    assert!(s["totals"]["bytes_on_disk"].as_u64().unwrap() >= 2 * 1024 * 1024);
    assert!(s["snapshot"]["path"].as_str().unwrap().contains("snapshots"));

    let r = c.call("tree", serde_json::json!({ "path": tree.to_string_lossy() }));
    assert_eq!(r["structuredContent"]["children"][0]["name"], "cache");

    let r = c.call("scan", serde_json::json!({ "delete": true }));
    assert_eq!(r["isError"], true, "unknown arguments are refused: {r}");
    assert_eq!(c.close(), Some(0));
    wdc_testkit::remove_tree(&tree);
    wdc_testkit::remove_tree(&data);
}

/// Slice 6: `scan` takes `jobs` (1 to 4); the report is the same, and a value out of range is
/// an error that names the range.
#[test]
fn scan_takes_jobs_from_one_to_four() {
    let tree = fresh_dir("mcp-headless-jobs-tree");
    for d in 0..12 {
        let dir = tree.join(format!("d{d}")).join("deep");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![1u8; 20_000 + d]).unwrap();
    }
    let data = fresh_dir("mcp-headless-jobs-data");
    let mut c = McpClient::start(
        std::path::Path::new(env!("CARGO_BIN_EXE_wdc")),
        &[],
        &[("WDC_DATA_DIR", data.as_os_str()), ("WDC_PIPE_SUFFIX", std::ffi::OsStr::new("-headless-jobs-test"))],
    );
    c.initialize("headless-jobs-test-client");
    let tools = c.request("tools/list", serde_json::json!({}));
    let scan = tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == "scan").unwrap().clone();
    assert_eq!(scan["inputSchema"]["properties"]["jobs"]["minimum"], 1);
    assert_eq!(scan["inputSchema"]["properties"]["jobs"]["maximum"], 4);
    assert!(scan["description"].as_str().unwrap().contains("only when the person is waiting"));

    let roots = serde_json::json!([tree.to_string_lossy()]);
    let one = c.call("scan", serde_json::json!({ "roots": roots, "jobs": 1 }));
    let four = c.call("scan", serde_json::json!({ "roots": roots, "jobs": 4 }));
    assert_ne!(one["isError"], true, "{one}");
    assert_ne!(four["isError"], true, "{four}");
    let strip = |r: &serde_json::Value| {
        let mut s = r["structuredContent"].clone();
        for k in ["started", "duration_ms", "snapshot"] {
            s.as_object_mut().unwrap().remove(k);
        }
        s
    };
    assert_eq!(strip(&four), strip(&one));
    assert_eq!(four["structuredContent"]["totals"]["files"], 12);

    for bad in [serde_json::json!(9), serde_json::json!(0), serde_json::json!("4")] {
        let r = c.call("scan", serde_json::json!({ "roots": roots, "jobs": bad }));
        assert_eq!(r["isError"], true, "jobs {bad} is refused: {r}");
        assert!(r.to_string().contains("1 to 4"), "the error names the range: {r}");
    }
    assert_eq!(c.close(), Some(0));
    wdc_testkit::remove_tree(&tree);
    wdc_testkit::remove_tree(&data);
}

/// Without the app there is no person to ask: `request_apply` answers `needs-human` and the
/// planned item stays where it is (plan, section 4.6).
#[test]
fn request_apply_without_the_app_changes_nothing() {
    let base = fresh_dir("mcp-headless-apply");
    let profile = base.join("u");
    let roaming = profile.join("AppData").join("Roaming");
    let local = profile.join("AppData").join("Local");
    let cache = roaming.join("Code").join("CachedData");
    std::fs::create_dir_all(cache.join("x")).unwrap();
    std::fs::create_dir_all(local.join("Temp")).unwrap();
    std::fs::write(cache.join("x").join("blob.bin"), vec![1u8; 2 * 1024 * 1024]).unwrap();
    let data = base.join("data");
    let mut c = McpClient::start(
        std::path::Path::new(env!("CARGO_BIN_EXE_wdc")),
        &[],
        &[
            ("WDC_DATA_DIR", data.as_os_str()),
            ("WDC_PIPE_SUFFIX", std::ffi::OsStr::new("-headless-apply-test")),
            ("USERPROFILE", profile.as_os_str()),
            ("APPDATA", roaming.as_os_str()),
            ("LOCALAPPDATA", local.as_os_str()),
            ("TEMP", local.join("Temp").as_os_str()),
        ],
    );
    c.initialize("headless-apply-client");
    let r = c.call("plan", serde_json::json!({ "paths": [cache.to_string_lossy()] }));
    assert_ne!(r["isError"], true, "{r}");
    let plan = &r["structuredContent"];
    assert_eq!(plan["items"].as_array().unwrap().len(), 1, "{plan}");
    assert_eq!(plan["items"][0]["rule_id"], "vscode-caches");
    let hash = plan["hash"].as_str().unwrap().to_string();

    let r = c.call("request_apply", serde_json::json!({ "plan": hash, "purge_now": true }));
    assert_ne!(r["isError"], true, "{r}");
    assert_eq!(r["structuredContent"]["status"], "needs-human", "{r}");
    assert_eq!(r["structuredContent"]["changed"], false);
    assert!(cache.join("x").join("blob.bin").exists(), "nothing moved");
    let r = c.call("quarantine", serde_json::json!({}));
    assert_eq!(r["structuredContent"]["held_bytes"], 0, "{r}");
    assert_eq!(c.close(), Some(0));
    wdc_testkit::remove_tree(&base);
}

/// Every result says whether the app is attached, and the instructions never claim the person
/// is watching when nobody is.
#[test]
fn headless_results_and_instructions_say_nobody_is_watching() {
    let data = fresh_dir("mcp-headless-mode-data");
    let mut c = McpClient::start(
        std::path::Path::new(env!("CARGO_BIN_EXE_wdc")),
        &[],
        &[("WDC_DATA_DIR", data.as_os_str()), ("WDC_PIPE_SUFFIX", std::ffi::OsStr::new("-headless-mode-test"))],
    );
    let init = c.initialize("headless-mode-client");
    let instructions = init["instructions"].as_str().unwrap();
    assert!(instructions.contains("not watching"), "{instructions}");
    assert!(instructions.contains("open the WinDevCleaner app"), "{instructions}");
    assert!(!instructions.contains("watches every call"), "{instructions}");

    let r = c.call("quarantine", serde_json::json!({}));
    assert_ne!(r["isError"], true, "{r}");
    guard_assert!("mcp-app-field", r["structuredContent"]["app"] == "headless", "a result carries the mode: {r}");
    assert!(r["structuredContent"].get("app_reason").is_none(), "{r}");

    let r = c.call("tree", serde_json::json!({ "path": "C:\\nowhere" }));
    assert_eq!(r["isError"], true, "{r}");
    guard_assert!("mcp-app-field", r["structuredContent"]["app"] == "headless", "an error carries the mode too: {r}");
    assert_eq!(c.close(), Some(0));
    wdc_testkit::remove_tree(&data);
}
