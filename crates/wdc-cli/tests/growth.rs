//! Slice 2 (plan, section 8): `wdc diff` on two snapshots of a fixture that grew, and snapshot
//! retention. Runs `wdc.exe` itself; platform-independent, and run on the Windows runner.

use std::path::Path;
use std::process::Command;

use wdc_testkit::{fresh_dir, guard_assert};

const MIB: usize = 1024 * 1024;

fn wdc(data: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_wdc")).args(args).env("WDC_DATA_DIR", data).output().unwrap();
    assert!(out.status.success(), "wdc {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("JSON output")
}

fn write(path: &Path, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Not compressible and not sparse: the allocation follows the length.
    let data: Vec<u8> = (0..len).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    std::fs::write(path, data).unwrap();
}

#[test]
fn diff_reports_new_grown_and_removed_folders_between_two_scans() {
    let tree = fresh_dir("growth-tree");
    let data = fresh_dir("growth-data");
    write(&tree.join("cache").join("objects").join("a.bin"), 2 * MIB);
    write(&tree.join("old-tool").join("bin").join("tool.exe"), 3 * MIB);
    write(&tree.join("docs").join("readme.txt"), 4 * MIB);
    let root = tree.to_string_lossy().to_string();
    wdc(&data, &["scan", &root, "--json"]);

    // The fixture grows: a cache grows deep inside, a new folder appears, one is removed.
    write(&tree.join("cache").join("objects").join("b.bin"), 6 * MIB);
    write(&tree.join("vm-bundle").join("disks").join("disk.vhdx"), 8 * MIB);
    std::fs::remove_dir_all(tree.join("old-tool")).unwrap();
    wdc(&data, &["scan", &root, "--json"]);

    let d = wdc(&data, &["diff", "--json"]);
    println!("{}", serde_json::to_string_pretty(&d).unwrap());
    let dirs = d["dirs"].as_array().unwrap();
    let find = |suffix: &str| dirs.iter().find(|c| c["path"].as_str().unwrap().ends_with(suffix)).cloned();

    let new = find("vm-bundle");
    guard_assert!("diff-new", new.is_some(), "the new folder is reported");
    let new = new.unwrap();
    assert_eq!(new["change"], "new");
    assert!(new["delta_bytes"].as_i64().unwrap() >= 8 * MIB as i64);

    let grown = find("objects");
    guard_assert!("diff-grown", grown.is_some(), "the grown folder is reported where it grew");
    let grown = grown.unwrap();
    assert_eq!(grown["change"], "grown");
    assert!(grown["delta_bytes"].as_i64().unwrap() >= 6 * MIB as i64);
    // Not repeated at every parent: `cache` grew only because of `objects`.
    assert!(find("cache").is_none(), "{dirs:?}");
    assert!(find("docs").is_none(), "unchanged folders are not listed");

    let removed = find("old-tool").expect("the removed folder is reported");
    assert_eq!(removed["change"], "removed");

    // New and grown first, largest first; removed after.
    let kinds: Vec<&str> = dirs.iter().map(|c| c["change"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["new", "grown", "removed"], "{dirs:?}");
    assert!(d["delta_bytes"].as_i64().unwrap() > 0);
    wdc_testkit::remove_tree(&tree);
    wdc_testkit::remove_tree(&data);
}

#[test]
fn diff_needs_two_scans_of_the_same_folders() {
    let tree = fresh_dir("growth-one");
    let data = fresh_dir("growth-one-data");
    write(&tree.join("f.bin"), MIB);
    let out = Command::new(env!("CARGO_BIN_EXE_wdc")).args(["diff"]).env("WDC_DATA_DIR", &data).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    wdc(&data, &["scan", &tree.to_string_lossy(), "--json"]);
    let other = fresh_dir("growth-other");
    wdc(&data, &["scan", &other.to_string_lossy(), "--json"]);
    // The newest scan is of another folder: nothing to compare it with.
    let out = Command::new(env!("CARGO_BIN_EXE_wdc")).args(["diff", "--json"]).env("WDC_DATA_DIR", &data).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    wdc_testkit::remove_tree(&tree);
    wdc_testkit::remove_tree(&other);
    wdc_testkit::remove_tree(&data);
}

#[test]
fn snapshot_retention_keeps_only_the_last_ten_and_reports_their_size() {
    let tree = fresh_dir("retention-tree");
    let data = fresh_dir("retention-data");
    write(&tree.join("a").join("f.bin"), MIB);
    let root = tree.to_string_lossy().to_string();
    let mut last = serde_json::Value::Null;
    for _ in 0..12 {
        last = wdc(&data, &["scan", &root, "--json"]);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let dir = data.join("snapshots");
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    let on_disk: u64 = files.iter().map(|e| e.metadata().unwrap().len()).sum();
    guard_assert!("snapshot-retention", files.len() == 10, "{} snapshots kept, expected 10", files.len());
    assert_eq!(last["snapshot"]["kept"], 10);
    assert_eq!(last["snapshot"]["total_bytes"].as_u64().unwrap(), on_disk, "reported size equals the files' size");
    // The newest is kept.
    let newest = Path::new(last["snapshot"]["path"].as_str().unwrap());
    assert!(newest.exists());
    wdc_testkit::remove_tree(&tree);
    wdc_testkit::remove_tree(&data);
}
