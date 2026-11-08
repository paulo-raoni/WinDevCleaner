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
    ]
}

/// Parsed arguments of `scan`.
#[derive(Clone, Debug)]
pub struct ScanArgs {
    pub options: ScanOptions,
    pub depth: u16,
    /// What the scan is about, for display ("C:\Users\me, C:\ProgramData").
    pub target: String,
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
    Ok(ScanArgs { options, depth, target })
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
    (report, summary)
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
