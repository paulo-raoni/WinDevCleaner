//! The agent tools of slice 1, shared by `wdc mcp` (headless) and the app (when an agent
//! drives it). Each face only routes calls here; the logic lives in the engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

use crate::scan::{scan, Progress, ScanOptions, ScanReport};
use crate::scope;
use crate::snapshot;

/// A tool definition: name, description and JSON Schema of its arguments.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
}

pub const SCAN: &str = "scan";
pub const TREE: &str = "tree";
pub const DIFF: &str = "diff";
pub const SYSTEM_ITEMS: &str = "system_items";
pub const EXPLAIN: &str = "explain";

/// Folders at least this large that no rule knows are flagged after a scan.
pub const UNKNOWN_LARGE_BYTES: u64 = 1024 * 1024 * 1024;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: SCAN,
            description: "Read-only scan of where disk space is. With no `roots`, scans the default read scope \
                (the user profile without personal folders, plus ProgramData). Sizes are sizes on disk: each file \
                counted once even with several hard-link names, links (junctions, symbolic links) recorded and \
                never followed, cloud files not stored locally counted 0 and never downloaded. Runs in Windows \
                background mode. Saves a snapshot in the tool's own data folder; changes nothing else. When the \
                WinDevCleaner app is open, the scan runs in the app and the user watches it there.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "roots": { "type": "array", "items": { "type": "string" },
                                   "description": "Folders to scan instead of the default read scope." },
                        "top": { "type": "integer", "minimum": 0, "maximum": 1000,
                                 "description": "How many of the largest files to return (default 50)." },
                        "depth": { "type": "integer", "minimum": 1, "maximum": 10,
                                   "description": "Depth below each root for the largest folders list (default 3)." }
                    },
                    "additionalProperties": false
                })
            },
        },
        ToolSpec {
            name: TREE,
            description: "Children of a folder from the latest scan, largest first, with sizes on disk. Use it to \
                drill down after `scan`. When the app is open, the app shows the same folder.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "A folder inside the latest scan." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000,
                                   "description": "Maximum children to return (default 50)." }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                })
            },
        },
        ToolSpec {
            name: DIFF,
            description: "What grew since last time: compares the latest scan with the previous scan of the same \
                folders (from the saved snapshots). New and grown folders come first, each shown at the place that \
                changed (not every parent), even when no rule knows them; then removed and shrunk ones. Needs two \
                scans of the same folders. When the app is open, the app shows the same comparison.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "min_bytes": { "type": "integer", "minimum": 0,
                                       "description": "Smallest change shown (default 1 MiB)." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000,
                                   "description": "Most folders shown (default 50)." }
                    },
                    "additionalProperties": false
                })
            },
        },
        ToolSpec {
            name: EXPLAIN,
            description: "Why a folder or file is here and what deleting it would free. Reports the rule that knows \
                it (class, evidence, source, cost, how to manage it), every condition with its result, whether it \
                is inside the allowed roots or forbidden (checked on its real final path), its size on disk, and \
                every name of each hard-linked file: a file with a name outside the item frees 0 bytes. \
                Read-only. When the app is open, the app shows the same explanation.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "A folder or file." } },
                    "required": ["path"],
                    "additionalProperties": false
                })
            },
        },
        ToolSpec {
            name: SYSTEM_ITEMS,
            description: "Space Windows itself manages, read through Windows' own APIs: Recycle Bin, paging, swap and \
                hibernation files, error reports, crash and memory dumps, System Restore. Always report-only; each \
                item names the Windows setting or tool that manages it. When the app is open, the app shows them.",
            schema: || json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        },
    ]
}

/// Parsed arguments of `scan`.
#[derive(Clone, Debug)]
pub struct ScanArgs {
    pub options: ScanOptions,
    pub depth: u16,
    /// What the scan is about, for display ("C:\Users\me, C:\ProgramData").
    pub target: String,
    /// The default read scope: system items are read too.
    pub default_scope: bool,
}

pub fn parse_scan_args(args: &Value) -> Result<ScanArgs, String> {
    let obj = match args {
        Value::Null => serde_json::Map::new(),
        Value::Object(m) => m.clone(),
        _ => return Err("arguments must be an object".into()),
    };
    for k in obj.keys() {
        if !["roots", "top", "depth"].contains(&k.as_str()) {
            return Err(format!("unknown argument `{k}`"));
        }
    }
    let mut options = ScanOptions::default();
    let default_scope = matches!(obj.get("roots"), None | Some(Value::Null));
    match obj.get("roots") {
        None | Some(Value::Null) => {
            let s = scope::default_read_scope();
            options.roots = s.roots;
            options.excluded = s.excluded;
        }
        Some(Value::Array(list)) => {
            for v in list {
                let s = v.as_str().ok_or("`roots` must be a list of strings")?;
                if s.trim().is_empty() {
                    return Err("empty root".into());
                }
                options.roots.push(PathBuf::from(s));
            }
            if options.roots.is_empty() {
                return Err("`roots` is empty".into());
            }
        }
        Some(_) => return Err("`roots` must be a list of strings".into()),
    }
    if let Some(v) = obj.get("top") {
        let n = v.as_u64().ok_or("`top` must be a non-negative integer")?;
        options.top_files = n.min(1000) as usize;
    }
    let depth = match obj.get("depth") {
        Some(v) => v.as_u64().ok_or("`depth` must be a positive integer")?.clamp(1, 10) as u16,
        None => 3,
    };
    let target = options.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ");
    Ok(ScanArgs { options, depth, target, default_scope })
}

/// Runs a scan, saves its snapshot, and returns the report with the agent's JSON summary.
pub fn run_scan<'a>(
    args: &ScanArgs,
    data_dir: &Path,
    progress: Option<&'a mut dyn FnMut(&Progress)>,
    cancel: Option<&'a AtomicBool>,
) -> (ScanReport, Value) {
    let report = scan(&args.options, progress, cancel);
    let mut summary = report.summary_json(args.depth, 40);
    if !report.cancelled {
        let saved = snapshot::save(&report, data_dir, snapshot::DEFAULT_DEPTH, snapshot::DEFAULT_KEEP);
        summary["snapshot"] = match saved {
            Ok(s) => serde_json::to_value(s).unwrap_or(Value::Null),
            Err(e) => json!({ "error": e.to_string() }),
        };
    }
    if args.default_scope {
        summary["system_items"] = serde_json::to_value(crate::system::system_items()).unwrap_or(Value::Null);
    }
    // Rules: what each large folder is, and the large folders no rule knows.
    let rules = crate::rules::RuleSet::with_local(data_dir);
    let ctx = crate::rules::Context::system();
    if let Some(list) = summary["largest_dirs"].as_array_mut() {
        for d in list.iter_mut() {
            let path = d["path"].as_str().unwrap_or_default().to_string();
            d["rule"] = rules.covers(&path, &ctx).map(Value::from).unwrap_or(Value::Null);
        }
    }
    summary["unknown_large"] = crate::explain::unknown_large(&report, &rules, &ctx, UNKNOWN_LARGE_BYTES)
        .into_iter()
        .take(20)
        .map(|(path, bytes)| json!({ "path": path, "bytes_on_disk": bytes }))
        .collect();
    (report, summary)
}

/// Parsed arguments of `explain`.
pub fn parse_explain_args(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
    if path.trim().is_empty() {
        return Err("`path` is empty".into());
    }
    Ok(path.to_string())
}

/// `explain`: built-in and local rules, conditions read from this machine.
pub fn run_explain(path: &str, data_dir: &Path) -> crate::explain::Explanation {
    let rules = crate::rules::RuleSet::with_local(data_dir);
    let ctx = crate::rules::Context::system();
    crate::explain::explain(path, &rules, &ctx, Some(&data_dir.to_string_lossy()))
}

/// Parsed arguments of `diff`.
pub fn parse_diff_args(args: &Value) -> Result<crate::diff::DiffOptions, String> {
    let mut o = crate::diff::DiffOptions::default();
    let obj = match args {
        Value::Null => return Ok(o),
        Value::Object(m) => m,
        _ => return Err("arguments must be an object".into()),
    };
    for k in obj.keys() {
        if !["min_bytes", "limit"].contains(&k.as_str()) {
            return Err(format!("unknown argument `{k}`"));
        }
    }
    if let Some(v) = obj.get("min_bytes") {
        o.min_bytes = v.as_u64().ok_or("`min_bytes` must be a non-negative integer")?;
    }
    if let Some(v) = obj.get("limit") {
        o.limit = v.as_u64().ok_or("`limit` must be a positive integer")?.clamp(1, 1000) as usize;
    }
    Ok(o)
}

/// `diff`: what grew between the last two scans of the same folders.
pub fn run_diff(data_dir: &Path, opts: crate::diff::DiffOptions) -> Result<crate::diff::Diff, String> {
    let mut d = crate::diff::diff_snapshots(data_dir, None, None, opts).map_err(|e| e.to_string())?;
    d.annotate(&crate::rules::RuleSet::with_local(data_dir), &crate::rules::Context::system());
    Ok(d)
}

/// `system_items`: space Windows manages, with the tool that manages each.
pub fn run_system_items() -> Value {
    let items = crate::system::system_items();
    json!({ "total_bytes": crate::system::total(&items), "items": items })
}

/// Parsed arguments of `tree`.
pub fn parse_tree_args(args: &Value) -> Result<(String, usize), String> {
    let path = args.get("path").and_then(Value::as_str).ok_or("`path` is required")?;
    let limit = match args.get("limit") {
        Some(v) => v.as_u64().ok_or("`limit` must be a positive integer")?.clamp(1, 1000) as usize,
        None => 50,
    };
    Ok((path.to_string(), limit))
}

pub fn run_tree(report: Option<&ScanReport>, path: &str, limit: usize) -> Result<Value, String> {
    let report = report.ok_or("no scan yet: call `scan` first")?;
    report
        .tree_json(path, limit)
        .ok_or_else(|| format!("`{path}` is not a folder of the latest scan"))
}

/// Short result line for the activity panel and logs: totals of a scan summary.
pub fn scan_totals(summary: &Value) -> Value {
    summary.get("totals").cloned().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scan_args() {
        let a = parse_scan_args(&json!({"roots": ["C:\\x"], "top": 5, "depth": 2})).unwrap();
        assert_eq!(a.options.roots, vec![PathBuf::from("C:\\x")]);
        assert_eq!(a.options.top_files, 5);
        assert_eq!(a.depth, 2);
        assert!(parse_scan_args(&json!({"rm": true})).is_err());
        assert!(parse_scan_args(&json!({"roots": []})).is_err());
        assert!(parse_scan_args(&json!({"roots": [1]})).is_err());
        let d = parse_scan_args(&Value::Null).unwrap();
        assert!(!d.options.roots.is_empty());
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        for s in specs() {
            assert_eq!((s.schema)()["type"], "object", "{}", s.name);
        }
    }
}
