//! Slice 6 (plan, sections 6.3 and 8): a scan with 2 or 4 jobs returns the same report as a
//! scan with 1, timing aside. Unix only: it runs in the Linux cloud session on the portable
//! walker and proves the merge there (the Windows walker shares the merge; its hard-link cases
//! are in `fixtures.rs`).
#![cfg(unix)]

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wdc_core::scan::{scan, Progress, ScanOptions, ScanReport};
use wdc_testkit::{fresh_dir, guard_assert, guard_assert_eq, remove_tree};

/// A small deterministic generator, so the tree is the same on every run.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

const LARGE: usize = 1_200_000;

/// About 3,000 files in about 330 folders, several levels deep, with many files of equal size
/// (so the top files have ties), hard-linked large files whose names sit in different folders,
/// and (on Unix) symbolic links.
fn generate(base: &Path) -> PathBuf {
    let root = base.join("root");
    let mut rng = Lcg(7);
    let mut dirs = vec![root.clone()];
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..330 {
        let parent = dirs[(rng.next() as usize) % dirs.len()].clone();
        let d = parent.join(format!("d{i:03}"));
        std::fs::create_dir_all(&d).unwrap();
        dirs.push(d);
    }
    for i in 0..3000 {
        let d = &dirs[(rng.next() as usize) % dirs.len()];
        // Few distinct sizes: many ties at the boundary of the top files.
        let len = match rng.next() % 5 {
            0 => 0,
            1 => 100,
            2 => 5_000,
            3 => 9_000,
            _ => 70_000,
        };
        std::fs::write(d.join(format!("f{i:04}.bin")), vec![1u8; len]).unwrap();
    }
    // Large files, each with three more names in other folders (never the same folder).
    for i in 0..12 {
        let home = &dirs[(rng.next() as usize) % dirs.len()];
        let original = home.join(format!("big{i:02}.dat"));
        std::fs::write(&original, vec![2u8; LARGE + i * 1000]).unwrap();
        for k in 0..3 {
            let other = &dirs[(rng.next() as usize) % dirs.len()];
            std::fs::hard_link(&original, other.join(format!("big{i:02}-name{k}.dat"))).unwrap();
        }
    }
    #[cfg(unix)]
    for i in 0..6 {
        let d = &dirs[(rng.next() as usize) % dirs.len()];
        std::os::unix::fs::symlink(&dirs[0], d.join(format!("link{i}"))).unwrap();
    }
    root
}

fn run(roots: &[&Path], excluded: &[PathBuf], jobs: usize, collect_large: bool) -> ScanReport {
    let opts = ScanOptions {
        roots: roots.iter().map(|p| p.to_path_buf()).collect(),
        excluded: excluded.to_vec(),
        jobs,
        collect_large,
        ..Default::default()
    };
    scan(&opts, None, None)
}

/// Every field but the timing ones, compared one by one so a failure names the field.
fn same_report(what: &str, one: &ScanReport, many: &ScanReport) -> Result<(), String> {
    macro_rules! field {
        ($f:ident) => {
            if one.$f != many.$f {
                return Err(format!("{what}: `{}` differs", stringify!($f)));
            }
        };
    }
    field!(roots);
    field!(totals);
    field!(dirs);
    field!(top_files);
    field!(links);
    field!(errors);
    field!(excluded);
    field!(cancelled);
    field!(large_files);
    Ok(())
}

#[test]
fn two_and_four_jobs_return_the_same_report_as_one() {
    let base = fresh_dir("parallel-same");
    let root = generate(&base);
    let one = run(&[&root], &[], 1, true);
    assert!(one.totals.files >= 3000 && one.totals.dirs >= 330, "{:?}", one.totals);
    assert!(one.totals.hardlink_duplicates >= 36, "the large files' other names are duplicates: {:?}", one.totals);
    assert_eq!(one.top_files.len(), 50);
    let mut failures = Vec::new();
    for jobs in [2usize, 4] {
        // Several runs: how the folders fall to the workers differs from run to run.
        for round in 0..3 {
            let many = run(&[&root], &[], jobs, true);
            if let Err(e) = same_report(&format!("{jobs} jobs, run {round}"), &one, &many) {
                failures.push(e);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    remove_tree(&base);
}

#[test]
fn the_tree_keeps_children_after_parents_and_the_subtree_totals_add_up() {
    let base = fresh_dir("parallel-tree");
    let root = generate(&base);
    let r = run(&[&root], &[], 4, false);
    for (i, d) in r.dirs.iter().enumerate() {
        assert!(d.parent == u32::MAX || (d.parent as usize) < i, "folder {i} comes before its parent");
        let kids: u64 = r.dirs.iter().filter(|k| k.parent == i as u32).map(|k| k.bytes).sum();
        assert_eq!(d.bytes, d.own_bytes + kids, "folder {i} ({})", d.name);
    }
    assert_eq!(r.roots[0].bytes_on_disk, r.totals.bytes_on_disk);
    assert_eq!(r.dirs[0].files, r.totals.files);
    remove_tree(&base);
}

#[test]
fn excluded_folders_and_several_roots_give_the_same_report_too() {
    let base = fresh_dir("parallel-excluded");
    let root = generate(&base);
    let second = base.join("second");
    std::fs::create_dir_all(second.join("x").join("y")).unwrap();
    std::fs::write(second.join("x").join("y").join("a.bin"), vec![3u8; 3000]).unwrap();
    let excluded: Vec<PathBuf> = ["d001", "d002", "d003"].iter().map(|n| root.join(n)).filter(|p| p.is_dir()).collect();
    assert!(!excluded.is_empty());
    let one = run(&[&root, &second], &excluded, 1, true);
    assert!(one.totals.excluded >= 1, "{:?}", one.totals);
    for jobs in [2usize, 4] {
        let many = run(&[&root, &second], &excluded, jobs, true);
        if let Err(e) = same_report(&format!("{jobs} jobs"), &one, &many) {
            panic!("{e}");
        }
    }
    remove_tree(&base);
}

#[test]
fn a_cancel_flag_set_from_the_start_gives_an_empty_cancelled_report_for_any_jobs() {
    let base = fresh_dir("parallel-cancel");
    let root = generate(&base);
    let cancel = AtomicBool::new(true);
    for jobs in [1usize, 2, 4] {
        let opts = ScanOptions { roots: vec![root.clone()], jobs, ..Default::default() };
        let r = scan(&opts, None, Some(&cancel));
        assert!(r.cancelled, "{jobs} jobs");
        assert!(r.dirs.is_empty() && r.roots.is_empty(), "{jobs} jobs: {} folders", r.dirs.len());
        assert_eq!(r.totals.files, 0);
        assert_eq!(r.totals.bytes_on_disk, 0);
    }
    remove_tree(&base);
}

#[test]
fn a_scan_cancelled_part_way_stops_and_leaves_a_consistent_tree() {
    let base = fresh_dir("parallel-cancel-midway");
    let root = generate(&base);
    let cancel = AtomicBool::new(false);
    let opts = ScanOptions { roots: vec![root.clone()], jobs: 4, throttle: Some(Duration::from_millis(20)), ..Default::default() };
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            cancel.store(true, Ordering::Relaxed);
        });
        scan(&opts, None, Some(&cancel))
    });
    assert!(r.cancelled);
    let full = run(&[&root], &[], 4, false);
    assert!(r.totals.files < full.totals.files, "it stopped early: {} of {}", r.totals.files, full.totals.files);
    for (i, d) in r.dirs.iter().enumerate() {
        assert!(d.parent == u32::MAX || (d.parent as usize) < i);
    }
    remove_tree(&base);
}

#[test]
fn progress_from_several_workers_never_goes_back() {
    let base = fresh_dir("parallel-progress");
    let root = generate(&base);
    let mut seen: Vec<(u64, u64, u64)> = Vec::new();
    let mut on_progress = |p: &Progress| seen.push((p.dirs, p.files, p.bytes_on_disk));
    let opts = ScanOptions {
        roots: vec![root.clone()],
        jobs: 4,
        // Each worker pauses before each folder: the scan lasts long enough to report.
        throttle: Some(Duration::from_millis(5)),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let r = scan(&opts, Some(&mut on_progress), None);
    let took = started.elapsed();
    assert!(!seen.is_empty(), "no progress was reported");
    for w in seen.windows(2) {
        assert!(w[1].0 >= w[0].0 && w[1].1 >= w[0].1 && w[1].2 >= w[0].2, "went back: {:?} then {:?}", w[0], w[1]);
    }
    // The throttle applies to every folder, in each worker: 4 workers share the pauses.
    let floor = Duration::from_millis(5) * (r.totals.dirs as u32 / 4);
    assert!(took >= floor * 8 / 10, "took {took:?}, the pauses alone need about {floor:?}");
    remove_tree(&base);
}

#[test]
fn a_file_with_several_names_in_folders_of_different_workers_counts_once() {
    let base = fresh_dir("parallel-hardlinks");
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    // Sibling folders, one name of every file in each of them.
    let n = 24;
    for d in 0..n {
        std::fs::create_dir_all(root.join(format!("s{d:02}"))).unwrap();
    }
    for f in 0..6 {
        let first = root.join("s00").join(format!("file{f}.dat"));
        std::fs::write(&first, vec![9u8; LARGE + f * 7000]).unwrap();
        for d in 1..n {
            std::fs::hard_link(&first, root.join(format!("s{d:02}")).join(format!("file{f}.dat"))).unwrap();
        }
        // And a name outside the root.
        std::fs::hard_link(&first, outside.join(format!("file{f}.dat"))).unwrap();
    }
    let one = run(&[&root], &[], 1, false);
    let one_both = run(&[&root, &outside], &[], 1, false);
    let expected_once: u64 = (0..6)
        .map(|f| {
            std::fs::metadata(root.join("s00").join(format!("file{f}.dat"))).unwrap().blocks() * 512
        })
        .sum();
    guard_assert_eq!("hardlink-dedup", one.totals.bytes_on_disk, expected_once, "one job: each file counts once");
    for jobs in [2usize, 4] {
        for _ in 0..3 {
            let r = run(&[&root], &[], jobs, false);
            guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, expected_once, "{jobs} jobs: each file counts once");
            guard_assert_eq!("hardlink-dedup", r.totals.hardlink_duplicates, 6 * (n as u64 - 1), "{jobs} jobs: every other name is a duplicate");
            guard_assert!("hardlink-dedup", same_report("hard links", &one, &r).is_ok(), "{jobs} jobs: the same report, names counted where one job counts them");
            // A name outside the roots does not change what the roots count, and a scan of
            // both counts the file once.
            let both = run(&[&root, &outside], &[], jobs, false);
            guard_assert_eq!("hardlink-dedup", both.totals.bytes_on_disk, expected_once, "{jobs} jobs, two roots: once");
            guard_assert!("hardlink-dedup", same_report("two roots", &one_both, &both).is_ok(), "{jobs} jobs, two roots: the same report");
        }
    }
    remove_tree(&base);
}
