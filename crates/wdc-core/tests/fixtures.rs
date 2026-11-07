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
    guard_assert_eq!("size-on-disk, compressed-size", r.totals.bytes_on_disk, expected, "sparse files count what is allocated");
    assert_eq!(r.totals.logical_bytes, 1056 * MIB);
    remove_tree(&base);
}

#[test]
fn compressed_files_count_their_compressed_allocation() {
    let base = fresh_dir("compressed");
    let f = base.join("log.txt");
    compressed_file(&f, 16 * MIB);
    let expected = compressed_size(&f);
    assert!(expected < 8 * MIB, "NTFS compressed it to {expected} bytes");

    let r = scan_roots(&[&base]);
    println!("compressed: scanner {} bytes, Windows {expected} bytes", r.totals.bytes_on_disk);
    guard_assert_eq!("compressed-size, size-on-disk", r.totals.bytes_on_disk, expected, "compressed files count what is allocated");
    assert_eq!(r.totals.logical_bytes, 16 * MIB);
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
        deep = deep.join(format!("pasta-com-nome-longo-número-{i:02}-ação-日本語"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 300);
    let f = deep.join("arquivo-ü.bin");
    write_file(&f, MIB);
    let truth = facts_by_handle(&f);

    let r = scan_roots(&[&base]);
    assert_eq!(r.totals.errors, 0, "{:?}", r.errors);
    assert_eq!(r.totals.bytes_on_disk, truth.allocation);
    assert_eq!(r.totals.dirs, 13);
    assert!(r.top_files[0].path.ends_with("arquivo-ü.bin"));
    remove_tree(&base);
}
