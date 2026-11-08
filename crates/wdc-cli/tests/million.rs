//! A 1,000,000-file tree under the memory and time limits, with background mode checked
//! from outside the process (plan, section 8, slice 1 row). Windows only; run with
//! `--ignored --release`. Writes its measurements to `$WDC_REPORT_DIR/million.json`.
//!
//! Limits: peak private memory at most 40 MB (the 200k-file budget of section 6.4; memory
//! must not grow with the number of files) and a warm scan at most 10 s (five times the
//! 200k-file warm budget of 2 s).
#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;

use wdc_testkit::{guard_assert, measure, Measurement};

const FILES_PER_DIR: usize = 1000;
const DIRS: usize = 1000;
const MAX_PRIVATE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_WARM_MS: f64 = 10_000.0;

fn generate(root: &PathBuf) {
    let marker = root.with_extension("complete");
    if marker.exists() {
        return;
    }
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).unwrap();
    for d in 0..DIRS {
        let dir = root.join(format!("pkg{d:04}"));
        std::fs::create_dir(&dir).unwrap();
        for f in 0..FILES_PER_DIR {
            std::fs::File::create(dir.join(format!("file{f:04}.js"))).unwrap();
        }
    }
    std::fs::write(marker, b"ok").unwrap();
}

fn run_scan(root: &PathBuf, data: &PathBuf) -> (Measurement, serde_json::Value) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wdc"));
    cmd.arg("scan").arg(root).args(["--json", "--top", "10"]).env("WDC_DATA_DIR", data);
    let m = measure(&mut cmd);
    assert_eq!(m.exit_code, Some(0), "wdc failed: {}", m.stderr);
    let v: serde_json::Value = serde_json::from_str(&m.stdout).expect("JSON output");
    (m, v)
}

#[test]
#[ignore]
fn a_million_files_stay_under_the_memory_and_time_limits_in_background_mode() {
    let root = std::env::var_os("WDC_MILLION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| wdc_testkit::fresh_dir("million").join("tree"));
    let started = std::time::Instant::now();
    generate(&root);
    let generation_s = started.elapsed().as_secs_f64();
    let data = wdc_testkit::fresh_dir("million-data");

    // The first scan warms the cache; the second is the warm measurement.
    let (first, _) = run_scan(&root, &data);
    let (m, v) = run_scan(&root, &data);

    let totals = &v["totals"];
    let report = serde_json::json!({
        "generation_seconds": generation_s,
        "first_scan": first,
        "warm_scan": m,
        "totals": totals,
        "self_reported": v["process"],
        "limits": { "max_private_bytes": MAX_PRIVATE_BYTES, "max_warm_ms": MAX_WARM_MS },
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Some(dir) = std::env::var_os("WDC_REPORT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(PathBuf::from(dir).join("million.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }

    assert_eq!(totals["files"], (DIRS * FILES_PER_DIR) as u64);
    assert_eq!(totals["dirs"], (DIRS + 1) as u64);
    assert_eq!(totals["errors"], 0);

    // Background mode, observed from outside: every sample of the process' I/O priority is
    // "very low" (0). Normal priority is 2.
    guard_assert!("background-mode", !m.io_priority_samples.is_empty(), "no samples taken");
    guard_assert!(
        "background-mode",
        m.io_priority_samples.iter().all(|&p| p == 0),
        "I/O priority samples while scanning: {:?}",
        m.io_priority_samples.iter().copied().collect::<std::collections::BTreeSet<_>>()
    );
    guard_assert!(
        "background-mode",
        v["process"]["background_mode"]["io_priority"] == 0,
        "self-reported: {}",
        v["process"]["background_mode"]
    );

    assert!(
        m.peak_private_bytes <= MAX_PRIVATE_BYTES,
        "peak private bytes {} over {MAX_PRIVATE_BYTES}",
        m.peak_private_bytes
    );
    assert!(m.wall_ms <= MAX_WARM_MS, "warm scan took {} ms", m.wall_ms);
}
