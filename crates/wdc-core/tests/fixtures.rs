//! Slice-1 scanner fixtures (plan, section 8, slice 1 row). Windows only: they run on the
//! `windows-latest` runner. Expected values come from Windows itself through other APIs
//! (a handle on the file, `GetCompressedFileSizeW`), never from the scanner.
//!
//! Guarded assertions use `guard_assert!`: ci/guard-check.sh removes a guarding line from the
//! scanner and checks that the test then fails at such an assertion (`GUARD-CHECK failed`),
//! not at an unrelated one.
#![cfg(windows)]

use std::path::{Path, PathBuf};

use wdc_core::scan::{scan, ScanOptions, ScanReport};
use wdc_testkit::{guard_assert, guard_assert_eq};
use wdc_testkit::*;

const MIB: u64 = 1024 * 1024;

fn scan_roots(roots: &[&Path]) -> ScanReport {
    let opts = ScanOptions { roots: roots.iter().map(|p| p.to_path_buf()).collect(), ..Default::default() };
    scan(&opts, None, None)
}

fn scan_roots_with_jobs(roots: &[&Path], jobs: usize) -> ScanReport {
    let opts = ScanOptions { roots: roots.iter().map(|p| p.to_path_buf()).collect(), jobs, ..Default::default() };
    scan(&opts, None, None)
}

fn file_bytes(r: &ScanReport, name: &str) -> Option<(u64, u32)> {
    r.top_files.iter().find(|f| f.path.ends_with(name)).map(|f| (f.bytes_on_disk, f.links))
}

#[test]
fn hard_link_with_both_names_inside_the_root_counts_once() {
    let base = fresh_dir("hl-inside");
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let a = root.join("bundle.vhdx");
    write_file(&a, 64 * MIB);
    std::fs::hard_link(&a, root.join("bundle-copy.vhdx")).unwrap();
    let truth = facts_by_handle(&a);
    assert_eq!(truth.links, 2);

    let r = scan_roots(&[&root]);
    assert_eq!(r.totals.files, 2, "both names are listed");
    guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, truth.allocation, "a 64 MB file with two names counts once");
    guard_assert_eq!("hardlink-dedup", r.totals.hardlink_duplicates, 1, "the second name is a duplicate");
    remove_tree(&base);
}

#[test]
fn hard_link_with_a_name_outside_the_root_counts_once_and_reports_both_names() {
    let base = fresh_dir("hl-outside");
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let inside_name = root.join("bundle.vhdx");
    write_file(&inside_name, 64 * MIB);
    std::fs::hard_link(&inside_name, outside.join("bundle.vhdx")).unwrap();
    let truth = facts_by_handle(&inside_name);

    // Only the inside name is in scope: it counts its full size once, and the scanner knows
    // the file has another name (the Cowork case of 2026-11-07).
    let r = scan_roots(&[&root]);
    assert_eq!(r.totals.bytes_on_disk, truth.allocation);
    let (bytes, links) = file_bytes(&r, "bundle.vhdx").expect("the 64 MB file is in the top files");
    assert_eq!(bytes, truth.allocation);
    guard_assert_eq!("large-file-recheck", links, 2, "the link count is read from the file itself");

    // Scanning both folders in one scan still counts the file once.
    let r = scan_roots(&[&root, &outside]);
    guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, truth.allocation, "two roots, one file");
    remove_tree(&base);
}

/// Slice 6: the same two cases with 4 workers, the names in sibling folders so that different
/// workers meet them. A file counts once however the workers share the folders.
#[test]
fn hard_link_names_in_sibling_folders_count_once_with_4_jobs() {
    let base = fresh_dir("hl-inside-jobs");
    let root = base.join("root");
    let first = root.join("s0");
    std::fs::create_dir_all(&first).unwrap();
    let a = first.join("bundle.vhdx");
    write_file(&a, 64 * MIB);
    let names = 8u64;
    for i in 1..names {
        let dir = root.join(format!("s{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::hard_link(&a, dir.join("bundle.vhdx")).unwrap();
    }
    let truth = facts_by_handle(&a);
    assert_eq!(truth.links, names as u32);

    for round in 0..3 {
        let r = scan_roots_with_jobs(&[&root], 4);
        assert_eq!(r.totals.files, names, "every name is listed");
        guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, truth.allocation, "round {round}: a 64 MB file with {names} names counts once");
        guard_assert_eq!("hardlink-dedup", r.totals.hardlink_duplicates, names - 1, "round {round}: every other name is a duplicate");
    }
    remove_tree(&base);
}

#[test]
fn hard_link_with_a_name_outside_the_root_counts_once_with_4_jobs() {
    let base = fresh_dir("hl-outside-jobs");
    let root = base.join("root");
    let outside = base.join("outside");
    for d in ["a", "b", "c"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
        std::fs::create_dir_all(outside.join(d)).unwrap();
    }
    let inside_name = root.join("a").join("bundle.vhdx");
    write_file(&inside_name, 64 * MIB);
    std::fs::hard_link(&inside_name, root.join("b").join("bundle.vhdx")).unwrap();
    std::fs::hard_link(&inside_name, root.join("c").join("bundle.vhdx")).unwrap();
    std::fs::hard_link(&inside_name, outside.join("a").join("bundle.vhdx")).unwrap();
    std::fs::hard_link(&inside_name, outside.join("b").join("bundle.vhdx")).unwrap();
    let truth = facts_by_handle(&inside_name);
    assert_eq!(truth.links, 5);

    for round in 0..3 {
        // Three names in the root: one count, and the scanner knows the file has five names.
        let r = scan_roots_with_jobs(&[&root], 4);
        guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, truth.allocation, "round {round}: three names in the root count once");
        let (_, links) = file_bytes(&r, "bundle.vhdx").expect("the 64 MB file is in the top files");
        assert_eq!(links, 5, "the link count is read from the file itself");

        // Both folders in one scan still count the file once.
        let r = scan_roots_with_jobs(&[&root, &outside], 4);
        guard_assert_eq!("hardlink-dedup", r.totals.bytes_on_disk, truth.allocation, "round {round}: two roots, one file");
        guard_assert_eq!("hardlink-dedup", r.totals.hardlink_duplicates, 4, "round {round}: four names are duplicates");
    }
    remove_tree(&base);
}

#[test]
fn stale_size_in_another_names_directory_entry_is_corrected() {
    let base = fresh_dir("hl-stale");
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let writer_name = outside.join("disk.vhdx");
    let other_name = root.join("disk.vhdx");
    write_file(&writer_name, 64 * MIB);
    std::fs::hard_link(&writer_name, &other_name).unwrap();
    // Grow the file through the outside name: the inside name's directory entry keeps the
    // old size (The Old New Thing, 2011-12-26).
    append(&writer_name, 32 * MIB);
    let truth = facts_by_handle(&writer_name);
    assert_eq!(truth.length, 96 * MIB);
    let entry = directory_entry_length(&other_name);
    println!("directory entry of the other name says {entry} bytes; the file has {}", truth.length);

    let r = scan_roots(&[&root]);
    let (bytes, _) = file_bytes(&r, "disk.vhdx").unwrap();
    guard_assert_eq!("large-file-recheck", bytes, truth.allocation, "size re-read from the file (entry said {entry})");
    remove_tree(&base);
}

#[test]
fn junctions_and_symbolic_links_are_recorded_and_never_followed() {
    let base = fresh_dir("links");
    let root = base.join("root");
    let target = base.join("target");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    write_file(&target.join("big.bin"), 10 * MIB);
    write_file(&root.join("own.bin"), 64 * 1024);
    junction(&root.join("to-target"), &target);
    junction(&root.join("loop"), &root);
    junction(&root.join("windows"), Path::new(r"C:\Windows"));
    symlink_dir(&root.join("dir-symlink"), &target);
    symlink_file(&root.join("file-symlink.bin"), &target.join("big.bin"));
    let own = facts_by_handle(&root.join("own.bin")).allocation;

    let r = scan_roots(&[&root]);
    guard_assert_eq!("no-follow-links", r.totals.links, 5, "every link is recorded");
    guard_assert_eq!("no-follow-links", r.totals.dirs, 1, "no link is walked into as a folder");
    guard_assert_eq!("no-follow-links", r.totals.files, 1, "a file symbolic link is not a file");
    assert_eq!(r.totals.bytes_on_disk, own, "nothing behind a link is counted");
    assert_eq!(r.totals.errors, 0, "{:?}", r.errors);
    let kinds: Vec<_> = r.links.iter().map(|l| (l.path.rsplit('\\').next().unwrap().to_string(), l.kind)).collect();
    println!("{kinds:?}");
    remove_tree(&base);
}

#[test]
fn a_root_that_is_a_junction_is_recorded_and_not_followed() {
    let base = fresh_dir("root-link");
    let target = base.join("target");
    std::fs::create_dir_all(&target).unwrap();
    write_file(&target.join("big.bin"), 10 * MIB);
    let link = base.join("link");
    junction(&link, &target);

    let r = scan_roots(&[&link]);
    guard_assert!("root-link", r.roots[0].is_link, "the root is reported as a link: {:?}", r.roots[0]);
    assert_eq!(r.totals.bytes_on_disk, 0);
    assert_eq!(r.totals.links, 1);
    remove_tree(&base);
}

#[test]
fn a_plain_file_counts_its_allocation_not_its_length() {
    let base = fresh_dir("plain");
    let f = base.join("small.txt");
    write_file(&f, 5000);
    let truth = facts_by_handle(&f);
    let cluster = cluster_size(&base);
    assert_eq!(truth.allocation, 5000u64.div_ceil(cluster) * cluster);

    let r = scan_roots(&[&base]);
    guard_assert_eq!("size-on-disk", r.totals.bytes_on_disk, truth.allocation, "allocation, not length 5000");
    assert_eq!(r.totals.logical_bytes, 5000);
    remove_tree(&base);
}

#[test]
fn sparse_files_count_their_allocation() {
    let base = fresh_dir("sparse");
    // One below the 64 MB re-read threshold, one above it.
    let small = base.join("sparse-32m.img");
    let large = base.join("sparse-1g.img");
    sparse_file(&small, 32 * MIB, 64 * 1024);
    sparse_file(&large, 1024 * MIB, 64 * 1024);
    let expected = compressed_size(&small) + compressed_size(&large);
    assert!(expected < 4 * MIB, "Windows reports {expected} bytes allocated");

    let r = scan_roots(&[&base]);
    println!("sparse: scanner {} bytes, Windows {expected} bytes", r.totals.bytes_on_disk);
    guard_assert_eq!("size-on-disk", r.totals.bytes_on_disk, expected, "sparse files count what is allocated");
    assert_eq!(r.totals.logical_bytes, 1056 * MIB);
    remove_tree(&base);
}

#[test]
fn compressed_files_count_their_compressed_allocation() {
    let base = fresh_dir("compressed");
    // One below the 64 MB re-read threshold, one above it.
    let small = base.join("log.txt");
    let large = base.join("big.log");
    compressed_file(&small, 16 * MIB);
    compressed_file(&large, 80 * MIB);
    let expected = compressed_size(&small) + compressed_size(&large);
    assert!(expected < 48 * MIB, "NTFS compressed them to {expected} bytes");

    let r = scan_roots(&[&base]);
    println!("compressed: scanner {} bytes, Windows {expected} bytes", r.totals.bytes_on_disk);
    guard_assert_eq!("size-on-disk", r.totals.bytes_on_disk, expected, "compressed files count what is allocated");
    assert_eq!(r.totals.logical_bytes, 96 * MIB);
    remove_tree(&base);
}

#[test]
fn offline_files_count_zero_and_are_never_opened() {
    let base = fresh_dir("offline");
    let f = base.join("archived.bin");
    write_file(&f, 2 * MIB);
    // FILE_ATTRIBUTE_OFFLINE: the data is said to live elsewhere.
    add_attributes(&f, 0x1000);
    assert!(facts_by_handle(&f).allocation >= 2 * MIB);

    let r = scan_roots(&[&base]);
    assert_eq!(r.totals.placeholders, 1);
    guard_assert_eq!("placeholder-zero", r.totals.bytes_on_disk, 0, "offline files count 0");
    remove_tree(&base);
}

#[test]
fn unreadable_folders_and_locked_files_are_reported_and_the_scan_goes_on() {
    let base = fresh_dir("denied");
    let denied = base.join("denied");
    std::fs::create_dir_all(&denied).unwrap();
    write_file(&denied.join("secret.bin"), MIB);
    let locked_path = base.join("locked.bin");
    write_file(&locked_path, 70 * MIB);
    let truth = facts_by_handle(&locked_path);
    let _held = hold_exclusive(&locked_path);
    deny_listing(&denied);

    let r = scan_roots(&[&base]);
    allow_listing(&denied);
    assert_eq!(r.totals.errors, 1, "{:?}", r.errors);
    assert_eq!(r.errors[0].kind, "access-denied");
    assert!(r.errors[0].path.ends_with("denied"));
    assert_eq!(r.totals.bytes_on_disk, truth.allocation, "the locked file is measured without opening its data");
    remove_tree(&base);
}

#[test]
fn long_and_unicode_paths_are_measured() {
    let base = fresh_dir("long");
    let mut deep: PathBuf = base.clone();
    for i in 0..12 {
        deep = deep.join(format!("folder-with-a-long-name-number-{i:02}-café-日本語"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 300);
    let f = deep.join("file-naïve-ü.bin");
    write_file(&f, MIB);
    let truth = facts_by_handle(&f);

    let r = scan_roots(&[&base]);
    assert_eq!(r.totals.errors, 0, "{:?}", r.errors);
    assert_eq!(r.totals.bytes_on_disk, truth.allocation);
    assert_eq!(r.totals.dirs, 13);
    assert!(r.top_files[0].path.ends_with("file-naïve-ü.bin"));
    remove_tree(&base);
}

// ---- explain and plan: the true size of every item (rc.2, task 1) ----

/// A fake user under `base`, whose Temp folder is `base\Temp`, with the clock far enough ahead
/// for the `temp-old` rule (older than 7 days).
fn temp_ctx(base: &Path) -> wdc_core::rules::Context {
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let env: std::collections::HashMap<String, String> = [
        ("USERPROFILE", s(base.to_path_buf())),
        ("APPDATA", s(base.join("AppData").join("Roaming"))),
        ("LOCALAPPDATA", s(base.join("AppData").join("Local"))),
        ("TEMP", s(base.join("AppData").join("Local").join("Temp"))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let mut c = wdc_core::rules::Context::new(env, Some(Default::default()), None);
    c.now = std::time::SystemTime::now() + std::time::Duration::from_secs(9 * 86_400);
    c
}

fn temp_of(base: &Path) -> PathBuf {
    let t = base.join("AppData").join("Local").join("Temp");
    std::fs::create_dir_all(&t).unwrap();
    t
}

#[test]
fn a_file_item_reports_its_allocation_and_one_file() {
    let base = fresh_dir("file-item");
    let f = temp_of(&base).join("update.exe");
    write_file(&f, 24_576);
    let truth = facts_by_handle(&f);
    let e = wdc_core::explain::explain(&f.to_string_lossy(), &wdc_core::rules::RuleSet::built_in(), &temp_ctx(&base), None);
    assert!(truth.allocation >= 24_576);
    guard_assert_eq!("file-root-size", e.bytes_on_disk, truth.allocation, "a file item reports its allocation, not 0");
    assert_eq!(e.logical_bytes, 24_576);
    assert_eq!(e.files, 1);
    assert!(e.complete, "{e:?}");
    remove_tree(&base);
}

#[test]
fn a_file_item_with_a_second_name_outside_it_frees_nothing() {
    let base = fresh_dir("file-item-linked");
    let f = temp_of(&base).join("shared.bin");
    write_file(&f, 70 * MIB);
    let other = base.join("elsewhere.bin");
    std::fs::hard_link(&f, &other).unwrap();
    let truth = facts_by_handle(&f);
    let e = wdc_core::explain::explain(&f.to_string_lossy(), &wdc_core::rules::RuleSet::built_in(), &temp_ctx(&base), None);
    assert_eq!(e.bytes_on_disk, truth.allocation);
    assert_eq!(e.hard_links.len(), 1, "{:#?}", e.hard_links);
    assert_eq!(e.hard_links[0].names_outside.len(), 1, "{:#?}", e.hard_links);
    guard_assert_eq!("expected-freed", e.expected_freed_bytes, 0, "the file frees nothing while its other name exists");
    remove_tree(&base);
}

#[test]
fn an_item_with_an_unreadable_subfolder_is_partial_and_left_out_of_the_plan() {
    let base = fresh_dir("partial-item");
    let item = temp_of(&base).join("old-build-output");
    let denied = item.join("denied");
    std::fs::create_dir_all(&denied).unwrap();
    write_file(&denied.join("secret.bin"), MIB);
    write_file(&item.join("readable.bin"), 5000);
    deny_listing(&denied);

    let ctx = temp_ctx(&base);
    let rules = wdc_core::rules::RuleSet::built_in();
    let e = wdc_core::explain::explain(&item.to_string_lossy(), &rules, &ctx, None);
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let opts = wdc_core::plan::DraftOptions { paths: vec![item.to_string_lossy().into_owned()], ..Default::default() };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let d = wdc_core::plan::draft(&opts, &rules, &ctx, Some(&data.to_string_lossy()), now);
    allow_listing(&denied);

    assert!(e.bytes_on_disk > 0, "the readable part is still measured");
    assert!(e.errors >= 1, "{e:?}");
    assert_eq!(e.first_error_kind, Some("access-denied"));
    assert!(e.partial_line(wdc_core::i18n::Lang::En).unwrap().starts_with("Partial: "));
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["complete"], false);
    assert_eq!(json["first_error_kind"], "access-denied");
    guard_assert!("explain-complete", !e.complete, "an unreadable subfolder makes the size incomplete");
    let left_out = d.not_included.iter().find(|n| n.path.ends_with("old-build-output"));
    guard_assert!(
        "plan-complete",
        d.plan.items.is_empty() && left_out.is_some(),
        "an incomplete item is not offered: {:#?}",
        d.plan.items
    );
    assert_eq!(left_out.unwrap().why_not, vec![wdc_core::explain::WhyNot::NotFullyReadable]);
    remove_tree(&base);
}
