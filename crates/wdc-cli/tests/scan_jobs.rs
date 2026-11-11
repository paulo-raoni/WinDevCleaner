//! Slice 6: `wdc scan --jobs N`. Platform-independent (it also runs in the Linux cloud session).

use std::path::Path;
use std::process::{Command, Output};

use wdc_testkit::{fresh_dir, remove_tree};

fn wdc(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wdc")).args(args).env("WDC_DATA_DIR", data).output().unwrap()
}

#[test]
fn jobs_outside_one_to_four_or_not_a_number_is_refused_with_exit_code_2_in_both_languages() {
    let data = fresh_dir("scan-jobs-bad-data");
    let tree = fresh_dir("scan-jobs-bad-tree");
    let tree_arg = tree.to_string_lossy().into_owned();
    for bad in ["0", "5", "x", "-1", "2.5", ""] {
        for (lang, words) in [("en", "from 1 to 4"), ("pt-BR", "de 1 a 4")] {
            let out = wdc(&data, &["--lang", lang, "scan", &tree_arg, "--jobs", bad]);
            assert_eq!(out.status.code(), Some(2), "--jobs {bad:?} ({lang})");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains("--jobs") && err.contains(words), "--jobs {bad:?} ({lang}): {err}");
            assert!(out.stdout.is_empty(), "nothing is scanned: {}", String::from_utf8_lossy(&out.stdout));
        }
    }
    // `--jobs` with no value at all.
    let out = wdc(&data, &["scan", &tree_arg, "--jobs"]);
    assert_eq!(out.status.code(), Some(2));
    // Nothing was scanned, so nothing was saved.
    assert!(!data.join("snapshots").exists() || std::fs::read_dir(data.join("snapshots")).unwrap().next().is_none());
    remove_tree(&data);
    remove_tree(&tree);
}

fn small_tree(name: &str) -> std::path::PathBuf {
    let tree = fresh_dir(name);
    for d in 0..30 {
        let dir = tree.join(format!("d{d:02}")).join("inner");
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..8 {
            std::fs::write(dir.join(format!("f{f}.bin")), vec![1u8; 1000 + d * 100 + f]).unwrap();
        }
    }
    std::fs::write(tree.join("d03").join("big.bin"), vec![2u8; 2 * 1024 * 1024]).unwrap();
    #[cfg(unix)]
    {
        std::fs::hard_link(tree.join("d03").join("big.bin"), tree.join("d17").join("big-again.bin")).unwrap();
        std::os::unix::fs::symlink(&tree, tree.join("d05").join("loop")).unwrap();
    }
    tree
}

/// What a scan says, without the fields that are about when and how long.
fn without_timing(mut v: serde_json::Value) -> serde_json::Value {
    let o = v.as_object_mut().unwrap();
    for k in ["started", "duration_ms", "snapshot", "process"] {
        o.remove(k).unwrap_or_else(|| panic!("`{k}` is in the output"));
    }
    v
}

#[test]
fn jobs_4_json_equals_jobs_1_json_except_timing() {
    let data = fresh_dir("scan-jobs-same-data");
    let tree = small_tree("scan-jobs-same-tree");
    let tree_arg = tree.to_string_lossy().into_owned();
    let run = |jobs: &str| {
        let out = wdc(&data, &["scan", &tree_arg, "--json", "--top", "20", "--depth", "3", "--jobs", jobs]);
        assert!(out.status.success(), "--jobs {jobs}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("JSON output")
    };
    let one = run("1");
    assert!(one["totals"]["files"].as_u64().unwrap() >= 240, "{one}");
    let default = run("1");
    for jobs in ["2", "4"] {
        let many = run(jobs);
        assert_eq!(without_timing(many.clone()), without_timing(one.clone()), "--jobs {jobs}");
    }
    // Leaving `--jobs` out is the same as `--jobs 1`.
    let out = wdc(&data, &["scan", &tree_arg, "--json", "--top", "20", "--depth", "3"]);
    let omitted: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(without_timing(omitted), without_timing(default));
    remove_tree(&data);
    remove_tree(&tree);
}
