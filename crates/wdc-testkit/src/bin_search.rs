//! Finds a workspace binary from the location of a test executable. Portable, so the search
//! logic is tested on Linux as well as on the Windows runner.

use std::path::{Path, PathBuf};

/// Walks up the ancestors of `start` (a test executable) and returns the first
/// `<ancestor>/<file_name>` that exists. The profile folder (`target/release`) is an ancestor
/// of both `deps/` and `build/<package>/<hash>/out/`, wherever cargo put the test executable.
/// On a miss, returns `Err` with the first candidate tried.
pub fn find_in_ancestors(start: &Path, file_name: &str) -> Result<PathBuf, PathBuf> {
    let mut first = None;
    for dir in start.ancestors().skip(1) {
        let candidate = dir.join(file_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        first.get_or_insert(candidate);
    }
    Err(first.unwrap_or_else(|| PathBuf::from(file_name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_the_binary_from_deps_and_from_build_out() {
        let root = std::env::temp_dir().join(format!("wdc-bin-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let profile = root.join("target/release");
        let deps = profile.join("deps");
        let out = profile.join("build/x/h/out");
        fs::create_dir_all(&deps).unwrap();
        fs::create_dir_all(&out).unwrap();
        fs::write(profile.join("wdc.exe"), b"").unwrap();
        fs::write(deps.join("t.exe"), b"").unwrap();
        fs::write(out.join("t.exe"), b"").unwrap();

        let from_deps = find_in_ancestors(&deps.join("t.exe"), "wdc.exe");
        let from_out = find_in_ancestors(&out.join("t.exe"), "wdc.exe");
        let missing = find_in_ancestors(&deps.join("t.exe"), "nope.exe");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(from_deps, Ok(profile.join("wdc.exe")));
        assert_eq!(from_out, Ok(profile.join("wdc.exe")));
        assert_eq!(missing, Err(deps.join("nope.exe")));
    }
}
