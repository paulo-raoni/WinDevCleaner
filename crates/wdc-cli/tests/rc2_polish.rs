//! Release candidate 2: `--top N` limits both human lists, and the version is the release
//! candidate. Portable: runs the real binary on a small tree.

use std::path::Path;
use std::process::{Command, Output};

use wdc_testkit::{fresh_dir, guard_assert_eq};

fn wdc(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wdc")).args(args).env("WDC_DATA_DIR", data).output().unwrap()
}

/// The lines of the section that starts at `header`, up to the next blank line.
fn section(text: &str, header: &str) -> Vec<String> {
    let mut lines = text.lines().skip_while(|l| l.trim() != header);
    assert!(lines.next().is_some(), "no `{header}` in:\n{text}");
    lines.take_while(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

fn tree(name: &str) -> std::path::PathBuf {
    let root = fresh_dir(name);
    for i in 1..=8u64 {
        let d = root.join(format!("folder{i}"));
        std::fs::create_dir_all(&d).unwrap();
        // Distinct sizes, so the order is fixed.
        std::fs::write(d.join(format!("file{i}.bin")), vec![1u8; (i as usize) * 64 * 1024]).unwrap();
    }
    root
}

#[test]
fn top_n_limits_the_folders_and_the_files_of_the_human_output() {
    let data = fresh_dir("polish-top-data");
    let root = tree("polish-top");
    let out = wdc(&data, &["--lang", "en", "scan", root.to_str().unwrap(), "--top", "5"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let folders = section(&text, "Largest folders:");
    let files = section(&text, "Largest files:");
    guard_assert_eq!("top-limits-lists", folders.len(), 5, "folders listed with --top 5:\n{text}");
    guard_assert_eq!("top-limits-lists", files.len(), 5, "files listed with --top 5:\n{text}");

    // Without --top the defaults apply (20 and 10): all 8 and 8 here.
    let out = wdc(&data, &["--lang", "en", "scan", root.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(section(&text, "Largest folders:").len(), 8);
    assert_eq!(section(&text, "Largest files:").len(), 8);

    // The JSON list of files follows --top as before.
    let out = wdc(&data, &["scan", root.to_str().unwrap(), "--top", "3", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["top_files"].as_array().unwrap().len(), 3);
    assert_eq!(v["errors_truncated"], false);
    assert!(v["errors_by_kind"].is_object());
}

#[test]
fn the_version_is_the_release_candidate() {
    let data = fresh_dir("polish-version");
    let out = wdc(&data, &["--version"]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    assert!(text.contains("1.0.0-rc.2"), "{text}");
}
