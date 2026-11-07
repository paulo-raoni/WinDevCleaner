//! The budget fixture of plan section 6.4, with its expected scan totals. Expected sizes are
//! read from Windows through a handle on each file (or `GetCompressedFileSizeW` for sparse
//! and compressed files), never through the scanner's directory enumeration.

use std::path::{Path, PathBuf};

use serde::Serialize;
use wdc_testkit::*;

const MIB: u64 = 1024 * 1024;
pub const PACKAGES: usize = 2500;
const DIRS_PER_PACKAGE: [&str; 10] =
    ["", "lib", "lib\\internal", "lib\\internal\\utils", "dist", "dist\\esm", "dist\\cjs", "types", "test", "docs"];
const FILES_PER_PACKAGE: usize = 80;
const SIZES: [u64; 7] = [0, 120, 600, 1500, 3000, 5000, 9000];

/// What a correct scan of `<root>\scan` must report.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct Expected {
    pub bytes_on_disk: u64,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub hardlink_duplicates: u64,
    pub placeholders: u64,
    pub errors: u64,
}

/// Facts about the generated fixture, for the report.
#[derive(Clone, Debug, Serialize)]
pub struct Fixture {
    pub root: String,
    pub expected: Expected,
    pub generation_seconds: f64,
    /// Cloud placeholders: created and connected, or why not (then they are untested).
    pub cloud: Result<CloudFacts, String>,
    pub stale_entry_bytes: u64,
    pub stale_true_bytes: u64,
    pub long_path_chars: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CloudFacts {
    pub dehydrated_placeholders: u64,
    pub partially_hydrated: bool,
}

/// Counts one file with its allocation read through a handle on it.
fn count_file(e: &mut Expected, p: &Path) {
    e.files += 1;
    e.bytes_on_disk += facts_by_handle(p).allocation;
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn data(len: u64) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

/// Generates the fixture under `base` (the VHDX root). Returns the facts and the paths that
/// need live handles during the scans (the locked file and the cloud root).
pub fn generate(base: &Path) -> (Fixture, PathBuf, PathBuf) {
    let started = std::time::Instant::now();
    let scan = base.join("scan");
    let outside = base.join("outside");
    std::fs::create_dir_all(&scan).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let mut e = Expected { dirs: 1, ..Default::default() };
    let mut facts = Fixture {
        root: scan.display().to_string(),
        expected: Expected::default(),
        generation_seconds: 0.0,
        cloud: Err("not attempted".into()),
        stale_entry_bytes: 0,
        stale_true_bytes: 0,
        long_path_chars: 0,
    };


    // 200,000 files in 25,000 folders shaped like node_modules and VS Code extensions.
    let packages = scan.join("node_modules");
    std::fs::create_dir(&packages).unwrap();
    e.dirs += 1;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let blobs: Vec<Vec<u8>> = SIZES.iter().map(|&s| data(s)).collect();
    for p in 0..PACKAGES {
        let pkg = packages.join(format!("package-{p:04}"));
        for d in DIRS_PER_PACKAGE {
            let dir = if d.is_empty() { pkg.clone() } else { pkg.join(d) };
            std::fs::create_dir_all(&dir).unwrap();
            e.dirs += 1;
        }
        for f in 0..FILES_PER_PACKAGE {
            let d = DIRS_PER_PACKAGE[f % DIRS_PER_PACKAGE.len()];
            let dir = if d.is_empty() { pkg.clone() } else { pkg.join(d) };
            let path = dir.join(format!("module-{f:02}.js"));
            let blob = &blobs[(rng.next() % SIZES.len() as u64) as usize];
            std::fs::write(&path, blob).unwrap();
            count_file(&mut e, &path);
        }
    }

    // 1,000 hard-link pairs (1 MiB each, so they are tracked by file ID): counted once.
    let pairs = scan.join("hardlinks");
    std::fs::create_dir(&pairs).unwrap();
    e.dirs += 1;
    let mib = data(MIB);
    for i in 0..1000 {
        let a = pairs.join(format!("pair-{i:04}-a.bin"));
        std::fs::write(&a, &mib).unwrap();
        std::fs::hard_link(&a, pairs.join(format!("pair-{i:04}-b.bin"))).unwrap();
        count_file(&mut e, &a);
        e.files += 1;
        e.hardlink_duplicates += 1;
    }

    let special = scan.join("special");
    std::fs::create_dir(&special).unwrap();
    e.dirs += 1;

    // One file with a name outside the scanned root (64 MiB): counted once, in full.
    let vm = special.join("bundle.vhdx");
    write_file(&vm, 64 * MIB);
    std::fs::hard_link(&vm, outside.join("bundle.vhdx")).unwrap();
    count_file(&mut e, &vm);

    // A stale-size link: grown through the outside name after linking.
    let grow_out = outside.join("grow.vhdx");
    write_file(&grow_out, 64 * MIB);
    let grow_in = special.join("grow.vhdx");
    std::fs::hard_link(&grow_out, &grow_in).unwrap();
    append(&grow_out, 16 * MIB);
    facts.stale_entry_bytes = directory_entry_length(&grow_in);
    facts.stale_true_bytes = facts_by_handle(&grow_out).length;
    count_file(&mut e, &grow_out);

    // Links: never followed.
    junction(&special.join("loop"), &scan);
    junction(&special.join("to-windows"), Path::new(r"C:\Windows"));
    symlink_dir(&special.join("dir-symlink"), &outside);
    symlink_file(&special.join("file-symlink.vhdx"), &outside.join("bundle.vhdx"));
    e.links += 4;

    // Sparse and compressed files: their allocation.
    let sparse = special.join("disk.img");
    sparse_file(&sparse, 1024 * MIB, 256 * 1024);
    e.files += 1;
    e.bytes_on_disk += compressed_size(&sparse);
    let compressed = special.join("server.log");
    compressed_file(&compressed, 32 * MIB);
    e.files += 1;
    e.bytes_on_disk += compressed_size(&compressed);

    // An offline file: counts 0.
    let offline = special.join("archived.bin");
    write_file(&offline, 2 * MIB);
    add_attributes(&offline, 0x1000);
    e.files += 1;
    e.placeholders += 1;

    // A path over 260 characters, with Unicode names.
    let mut long = special.clone();
    for i in 0..10 {
        long = long.join(format!("pasta-número-{i:02}-ação-日本語-ελληνικά"));
        e.dirs += 1;
    }
    std::fs::create_dir_all(&long).unwrap();
    let deep = long.join("arquivo-ü.bin");
    write_file(&deep, 100_000);
    facts.long_path_chars = deep.as_os_str().len();
    count_file(&mut e, &deep);

    // A folder the user cannot list: reported, not fatal.
    let denied = special.join("denied");
    std::fs::create_dir(&denied).unwrap();
    std::fs::write(denied.join("hidden.bin"), data(5000)).unwrap();
    deny_listing(&denied);
    e.dirs += 1;
    e.errors += 1;

    // A file held open exclusively during the scans (the caller holds it).
    let locked = special.join("locked.db");
    write_file(&locked, 3 * MIB);
    count_file(&mut e, &locked);

    // Real cloud placeholders from a test sync root, when the runner allows registering one.
    let cloud_root = scan.join("cloud");
    std::fs::create_dir(&cloud_root).unwrap();
    e.dirs += 1;
    facts.cloud = (|| {
        crate::cloud::register(&cloud_root)?;
        let _conn = crate::cloud::connect(&cloud_root)?;
        crate::cloud::create_placeholders(&cloud_root, 100, 10 * MIB)?;
        let partial = crate::cloud::partially_hydrated(&cloud_root.join("partial.bin"), 4 * MIB).is_ok();
        Ok(CloudFacts { dehydrated_placeholders: 100, partially_hydrated: partial })
    })();
    match &facts.cloud {
        Ok(c) => {
            e.files += c.dehydrated_placeholders;
            e.placeholders += c.dehydrated_placeholders;
            if c.partially_hydrated {
                e.files += 1;
                e.placeholders += 1;
            } else {
                // Conversion failed after the file was written: it is a plain file.
                let p = cloud_root.join("partial.bin");
                if p.exists() {
                    count_file(&mut e, &p);
                }
            }
        }
        Err(_) => {
            let p = cloud_root.join("partial.bin");
            if p.exists() {
                count_file(&mut e, &p);
            }
        }
    }

    facts.expected = e;
    facts.generation_seconds = started.elapsed().as_secs_f64();
    (facts, locked, cloud_root)
}
