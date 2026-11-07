//! `wdc mcp` with the app closed: the engine runs headless and answers over MCP stdio.
//! Platform-independent (it also runs in the Linux cloud session).

use wdc_testkit::{fresh_dir, McpClient};

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
    assert_eq!(names, vec!["scan", "tree"]);

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
