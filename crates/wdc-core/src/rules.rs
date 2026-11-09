//! The rule base (plan, section 4.2): data files in `rules/*.toml`, reviewed like code and
//! compiled into the executable. Only built-in rules can make an item actionable. Rules from
//! the local folder (`<data>\rules\*.toml`) help discovery and explanation but are always
//! report-only, so an agent that writes a rule file cannot make anything deletable.
//!
//! A rule applies to an item when one of its path patterns matches and every `match`
//! condition passes; the item is eligible when every `require` condition passes too.
//! Conditions come from a fixed set implemented here: rules contain no scripts.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

/// The built-in rule files, in priority order (the first rule that applies wins).
const BUILT_IN: &[(&str, &str)] = &[
    ("windows.toml", include_str!("../../../rules/windows.toml")),
    ("vscode.toml", include_str!("../../../rules/vscode.toml")),
    ("nvidia.toml", include_str!("../../../rules/nvidia.toml")),
    ("apps.toml", include_str!("../../../rules/apps.toml")),
];

/// What kind of item a rule describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Its owner rebuilds it automatically; deleting costs only time or a download.
    RegenerableCache,
    /// Left behind by an install or update that has finished.
    InstallLeftover,
    /// The app manages it; the right action is the app's own setting or tool.
    AppManaged,
    /// User data, system files, or anything whose loss is not recoverable.
    NeverTouch,
}

impl Class {
    pub fn text(self, lang: Lang) -> &'static str {
        use crate::i18n::Msg as M;
        lang.t(match self {
            Class::RegenerableCache => M::ClassRegenerableCache,
            Class::InstallLeftover => M::ClassInstallLeftover,
            Class::AppManaged => M::ClassAppManaged,
            Class::NeverTouch => M::ClassNeverTouch,
        })
    }
}

/// What the tool may do with an eligible item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Quarantine,
    ReportOnly,
    OpenAppSetting,
}

impl Action {
    pub fn text(self, lang: Lang) -> &'static str {
        use crate::i18n::Msg as M;
        lang.t(match self {
            Action::Quarantine => M::ActionQuarantine,
            Action::ReportOnly => M::ActionReportOnly,
            Action::OpenAppSetting => M::ActionOpenAppSetting,
        })
    }
}

/// Where a rule comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "file")]
pub enum Origin {
    #[default]
    BuiltIn,
    /// A file in the local rules folder: always report-only.
    Local(String),
}

/// Formats of files read by conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileFormat {
    /// A JSON object whose keys are names with a true value (VS Code's `.obsolete`).
    JsonObjectKeys,
    /// VS Code's `extensions.json`: entries with `relativeLocation` or `location.path`.
    VscodeExtensionsJson,
}

/// The fixed set of conditions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Condition {
    /// None of these programs is running.
    ProcessNotRunning { names: Vec<String> },
    /// The item was last changed at least this many days ago.
    OlderThan { days: u32 },
    /// The item's name is listed in a file.
    ListedInFile { file: String, format: FileFormat },
    /// The item's name is not referenced by a file.
    NotReferencedBy { file: String, format: FileFormat },
    /// The item's name matches a regular expression.
    NameMatchesRegex { pattern: String },
    /// The driver version in the item's name is not newer than the installed driver.
    DriverVersionNotNewer { vendor: String },
}

/// A rule.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub title: String,
    pub title_pt_br: String,
    pub class: Class,
    pub action: Action,
    pub paths: Vec<String>,
    pub evidence: String,
    pub evidence_pt_br: String,
    /// Where the evidence comes from: documentation, an observed case with its date, or a check.
    pub source: String,
    pub cost: String,
    pub cost_pt_br: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub instructions_pt_br: Option<String>,
    pub last_verified: String,
    #[serde(default, rename = "match")]
    pub matches: Vec<Condition>,
    #[serde(default)]
    pub require: Vec<Condition>,
    #[serde(skip_deserializing, default)]
    pub origin: Origin,
}

impl Rule {
    pub fn title(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.title,
            Lang::PtBr => &self.title_pt_br,
        }
    }
    pub fn evidence(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.evidence,
            Lang::PtBr => &self.evidence_pt_br,
        }
    }
    pub fn cost(&self, lang: Lang) -> &str {
        match lang {
            Lang::En => &self.cost,
            Lang::PtBr => &self.cost_pt_br,
        }
    }
    pub fn instructions(&self, lang: Lang) -> Option<&str> {
        match lang {
            Lang::En => self.instructions.as_deref(),
            Lang::PtBr => self.instructions_pt_br.as_deref().or(self.instructions.as_deref()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

/// The rules in use: built-in first, then local ones.
#[derive(Clone, Debug)]
pub struct RuleSet {
    pub rules: Vec<Rule>,
    /// Local rule files that could not be read, with why.
    pub local_errors: Vec<String>,
}

fn parse(text: &str) -> Result<Vec<Rule>, String> {
    let f: RuleFile = toml::from_str(text).map_err(|e| e.to_string())?;
    for r in &f.rule {
        if r.paths.is_empty() {
            return Err(format!("rule `{}` has no paths", r.id));
        }
        for c in r.matches.iter().chain(&r.require) {
            if let Condition::NameMatchesRegex { pattern } = c {
                regex_lite::Regex::new(pattern).map_err(|e| format!("rule `{}`: {e}", r.id))?;
            }
        }
    }
    Ok(f.rule)
}

impl RuleSet {
    /// The rules compiled into the executable.
    pub fn built_in() -> RuleSet {
        let mut rules = Vec::new();
        for (name, text) in BUILT_IN {
            match parse(text) {
                Ok(r) => rules.extend(r),
                Err(e) => panic!("built-in rule file {name} is invalid: {e}"),
            }
        }
        RuleSet { rules, local_errors: Vec::new() }
    }

    /// Built-in rules plus the local rules folder `<data_dir>\rules`.
    pub fn with_local(data_dir: &Path) -> RuleSet {
        let mut set = RuleSet::built_in();
        let dir = data_dir.join("rules");
        let Ok(rd) = std::fs::read_dir(&dir) else { return set };
        let mut files: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("toml")).unwrap_or(false))
            .collect();
        files.sort();
        for f in files {
            let text = match std::fs::read_to_string(&f) {
                Ok(t) => t,
                Err(e) => {
                    set.local_errors.push(format!("{}: {e}", f.display()));
                    continue;
                }
            };
            match parse(&text) {
                Ok(rules) => {
                    for mut r in rules {
                        r.origin = Origin::Local(f.display().to_string());
                        // A local rule can explain, never act: whatever it says.
                        // GUARD-BEGIN(local-report-only)
                        r.action = Action::ReportOnly;
                        // GUARD-END(local-report-only)
                        set.rules.push(r);
                    }
                }
                Err(e) => set.local_errors.push(format!("{}: {e}", f.display())),
            }
        }
        set
    }
}

/// What the conditions read from the machine. Built once per command; tests build their own.
pub struct Context {
    /// Environment variables, upper-case names.
    pub env: HashMap<String, String>,
    /// Names of running programs, lower-case (`code.exe`); None when they cannot be listed.
    pub running: Option<HashSet<String>>,
    /// The installed NVIDIA display driver version in Windows' form (`32.0.15.6094`).
    pub nvidia_driver: Option<String>,
    pub now: SystemTime,
    files: RefCell<HashMap<String, Option<String>>>,
    long: RefCell<HashMap<String, String>>,
}

impl Context {
    pub fn new(env: HashMap<String, String>, running: Option<HashSet<String>>, nvidia_driver: Option<String>) -> Context {
        let env = env.into_iter().map(|(k, v)| (k.to_uppercase(), v)).collect();
        let running = running.map(|r| r.into_iter().map(|n| n.to_lowercase()).collect());
        Context {
            env,
            running,
            nvidia_driver,
            now: SystemTime::now(),
            files: RefCell::new(HashMap::new()),
            long: RefCell::new(HashMap::new()),
        }
    }

    /// The real machine: environment, running programs, installed driver.
    pub fn system() -> Context {
        let env: HashMap<String, String> = std::env::vars().collect();
        Context::new(env, crate::facts::running_programs(), crate::facts::nvidia_driver_version())
    }

    /// Expands `%NAME%` variables; None if one is not set.
    pub fn expand(&self, pattern: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = pattern;
        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('%')?;
            let name = after[..end].to_uppercase();
            out.push_str(self.env.get(&name)?);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out)
    }

    /// An expanded pattern with its fixed folders in the long, real form Windows gives (short
    /// names such as `RUNNER~1` and links above the item resolved), so it matches the final
    /// paths `apply` checks. The last name and everything from the first wildcard on are kept
    /// as written: the item itself is never resolved.
    pub fn long_form(&self, pattern: &str) -> String {
        let p = canon(pattern);
        if let Some(l) = self.long.borrow().get(&p) {
            return l.clone();
        }
        let segs: Vec<&str> = p.split('\\').collect();
        let wild = segs.iter().position(|s| s.contains('*') || s.contains('?')).unwrap_or(segs.len());
        let fixed = wild.min(segs.len().saturating_sub(1));
        let mut out = p.clone();
        for k in (2..=fixed).rev() {
            let prefix = segs[..k].join("\\");
            if let Ok(r) = std::fs::canonicalize(native(&prefix)) {
                let r = r.to_string_lossy().into_owned();
                let r = canon(r.strip_prefix(r"\\?\").unwrap_or(&r));
                let rest = &segs[k..];
                out = if rest.is_empty() { r } else { format!("{r}\\{}", rest.join("\\")) };
                break;
            }
        }
        self.long.borrow_mut().insert(p, out.clone());
        out
    }

    fn read(&self, path: &str) -> Option<String> {
        if let Some(c) = self.files.borrow().get(path) {
            return c.clone();
        }
        let text = std::fs::read_to_string(native(path)).ok();
        self.files.borrow_mut().insert(path.to_string(), text.clone());
        text
    }
}

/// A path in the form used for matching: `\` separators, no trailing separator.
pub fn canon(p: &str) -> String {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let s = p.replace('/', "\\");
    let t = s.trim_end_matches('\\');
    if t.is_empty() {
        s
    } else {
        t.to_string()
    }
}

/// A canonical path back in the platform's form, for file access.
pub fn native(p: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(p)
    } else {
        PathBuf::from(p.replace('\\', "/"))
    }
}

fn segments(p: &str) -> Vec<&str> {
    p.split('\\').filter(|s| !s.is_empty()).collect()
}

/// `*` and `?` inside one name, case-insensitive.
pub(crate) fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// If `path` is an item of `pattern` or inside one, the item's path.
fn item_of(pattern: &str, path: &str) -> Option<String> {
    let pat = segments(pattern);
    let segs = segments(path);
    let (pat, whole_tree) = match pat.last() {
        Some(&"**") => (&pat[..pat.len() - 1], true),
        _ => (&pat[..], false),
    };
    if segs.len() < pat.len() || pat.is_empty() {
        return None;
    }
    for (p, s) in pat.iter().zip(&segs) {
        if !glob(p, s) {
            return None;
        }
    }
    let _ = whole_tree;
    // Keep the original spelling of the path for the item.
    let item: Vec<&str> = segs[..pat.len()].to_vec();
    let prefix = if path.starts_with("\\\\") { "\\\\" } else if path.starts_with('\\') { "\\" } else { "" };
    Some(format!("{prefix}{}", item.join("\\")))
}

fn last_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

/// What a condition found, as data, so every face can say it in the user's language.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "found", rename_all = "kebab-case")]
pub enum Found {
    Running { names: Vec<String> },
    NotRunning { names: Vec<String> },
    NoProcessList,
    Age { days: u64, needs: u32 },
    NoDate,
    Listed { file: String },
    NotListed { file: String },
    Referenced { file: String },
    NotReferenced { file: String },
    FileMissing { file: String },
    BadFile { file: String },
    VariableMissing { file: String },
    NameMatches { pattern: String },
    NameDiffers { pattern: String },
    DriverOlder { artifact: String, installed: String },
    DriverEqual { artifact: String, installed: String },
    DriverNewer { artifact: String, installed: String },
    NoDriver,
    NoArtifactVersion,
    Unsupported { what: String },
}

impl Found {
    /// The text in a language.
    pub fn text(&self, lang: Lang) -> String {
        use crate::i18n::Msg as M;
        match self {
            Found::Running { names } => lang.fmt(M::FoundRunning, &[("names", &names.join(", "))]),
            Found::NotRunning { names } => lang.fmt(M::FoundNotRunning, &[("names", &names.join(", "))]),
            Found::NoProcessList => lang.t(M::FoundNoProcessList).into(),
            Found::Age { days, needs } => {
                lang.fmt(M::FoundAge, &[("days", &days.to_string()), ("need", &needs.to_string())])
            }
            Found::NoDate => lang.t(M::FoundNoDate).into(),
            Found::Listed { file } => lang.fmt(M::FoundListed, &[("file", file)]),
            Found::NotListed { file } => lang.fmt(M::FoundNotListed, &[("file", file)]),
            Found::Referenced { file } => lang.fmt(M::FoundReferenced, &[("file", file)]),
            Found::NotReferenced { file } => lang.fmt(M::FoundNotReferenced, &[("file", file)]),
            Found::FileMissing { file } => lang.fmt(M::FoundFileMissing, &[("file", file)]),
            Found::BadFile { file } => lang.fmt(M::FoundBadFile, &[("file", file)]),
            Found::VariableMissing { file } => lang.fmt(M::FoundVarMissing, &[("file", file)]),
            Found::NameMatches { pattern } => lang.fmt(M::FoundNameMatches, &[("pattern", pattern)]),
            Found::NameDiffers { pattern } => lang.fmt(M::FoundNameDiffers, &[("pattern", pattern)]),
            Found::DriverOlder { artifact, installed } => {
                lang.fmt(M::FoundDriverOlder, &[("a", artifact), ("i", installed)])
            }
            Found::DriverEqual { artifact, installed } => {
                lang.fmt(M::FoundDriverEqual, &[("a", artifact), ("i", installed)])
            }
            Found::DriverNewer { artifact, installed } => {
                lang.fmt(M::FoundDriverNewer, &[("a", artifact), ("i", installed)])
            }
            Found::NoDriver => lang.t(M::FoundNoDriver).into(),
            Found::NoArtifactVersion => lang.t(M::FoundNoArtifactVersion).into(),
            Found::Unsupported { what } => lang.fmt(M::FoundUnsupported, &[("what", what)]),
        }
    }
}

/// The result of one condition on one item.
#[derive(Clone, Debug, Serialize)]
pub struct CondResult {
    pub condition: Condition,
    pub passed: bool,
    pub found: Found,
    /// What was found, in English (for JSON and logs).
    pub detail: String,
}

/// Version of an NVIDIA driver in NVIDIA's form, from Windows' form: `32.0.15.6094` is
/// `560.94` (the last digit of the third part and the four digits of the fourth).
pub fn nvidia_version_from_windows(v: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = v.trim().split('.').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let digit = parts[2].chars().last()?;
    let fourth: u32 = parts[3].parse().ok()?;
    if fourth > 9999 {
        return None;
    }
    let five = format!("{digit}{fourth:04}");
    Some((five[..3].parse().ok()?, five[3..].parse().ok()?))
}

/// The first `NNN.NN` driver version in a name (`566.14`).
pub fn nvidia_version_in_name(name: &str) -> Option<(u32, u32)> {
    let re = regex_lite::Regex::new(r"(?:^|[^0-9])([0-9]{3})\.([0-9]{2})(?:[^0-9]|$)").ok()?;
    let c = re.captures(name)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?))
}

fn artifact_version(item: &str) -> Option<(u32, u32)> {
    if let Some(v) = nvidia_version_in_name(last_name(item)) {
        return Some(v);
    }
    // Otherwise look at names inside it, two levels deep.
    let mut stack = vec![(native(item), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(v) = nvidia_version_in_name(&name) {
                return Some(v);
            }
            if depth < 1 && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    None
}

/// Folder names an `extensions.json` references.
fn vscode_referenced(text: &str) -> Option<HashSet<String>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = HashSet::new();
    for e in v.as_array()? {
        if let Some(r) = e.get("relativeLocation").and_then(|x| x.as_str()) {
            out.insert(r.to_lowercase());
        }
        for key in ["path", "fsPath"] {
            if let Some(p) = e.get("location").and_then(|l| l.get(key)).and_then(|x| x.as_str()) {
                out.insert(last_name(&canon(p)).to_lowercase());
            }
        }
    }
    Some(out)
}

fn ver(v: (u32, u32)) -> String {
    format!("{}.{:02}", v.0, v.1)
}

#[allow(unused_mut, unused_assignments)]
fn eval(c: &Condition, item: &str, ctx: &Context) -> CondResult {
    let name = last_name(item);
    let (passed, found) = match c {
        Condition::ProcessNotRunning { names } => match &ctx.running {
            None => (false, Found::NoProcessList),
            Some(running) => {
                let found: Vec<String> = names.iter().filter(|n| running.contains(&n.to_lowercase())).cloned().collect();
                let mut ok = true;
                // GUARD-BEGIN(process-not-running)
                ok = found.is_empty();
                // GUARD-END(process-not-running)
                if ok {
                    (true, Found::NotRunning { names: names.clone() })
                } else {
                    (false, Found::Running { names: found })
                }
            }
        },
        Condition::OlderThan { days } => match std::fs::symlink_metadata(native(item)).and_then(|m| m.modified()) {
            Ok(t) => {
                let age = ctx.now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
                let mut ok = true;
                // GUARD-BEGIN(older-than)
                ok = age >= *days as u64 * 86_400;
                // GUARD-END(older-than)
                (ok, Found::Age { days: age / 86_400, needs: *days })
            }
            Err(_) => (false, Found::NoDate),
        },
        Condition::ListedInFile { file, format } => match ctx.expand(file) {
            None => (false, Found::VariableMissing { file: file.clone() }),
            Some(f) => match (ctx.read(&f), format) {
                (None, _) => (false, Found::FileMissing { file: f }),
                (Some(text), FileFormat::JsonObjectKeys) => {
                    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) {
                        Err(_) => (false, Found::BadFile { file: f }),
                        Ok(m) => {
                            let mut listed = true;
                            // GUARD-BEGIN(listed-in-file)
                            listed = m.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && v.as_bool().unwrap_or(true));
                            // GUARD-END(listed-in-file)
                            if listed {
                                (true, Found::Listed { file: f })
                            } else {
                                (false, Found::NotListed { file: f })
                            }
                        }
                    }
                }
                (Some(_), other) => (false, Found::Unsupported { what: format!("{other:?} for listed-in-file") }),
            },
        },
        Condition::NotReferencedBy { file, format } => match ctx.expand(file) {
            None => (false, Found::VariableMissing { file: file.clone() }),
            Some(f) => match (ctx.read(&f), format) {
                // Without the file, references cannot be checked: not eligible.
                (None, _) => (false, Found::FileMissing { file: f }),
                (Some(text), FileFormat::VscodeExtensionsJson) => match vscode_referenced(&text) {
                    None => (false, Found::BadFile { file: f }),
                    Some(refs) => {
                        let mut referenced = false;
                        // GUARD-BEGIN(not-referenced-by)
                        referenced = refs.contains(&name.to_lowercase());
                        // GUARD-END(not-referenced-by)
                        if referenced {
                            (false, Found::Referenced { file: f })
                        } else {
                            (true, Found::NotReferenced { file: f })
                        }
                    }
                },
                (Some(_), other) => (false, Found::Unsupported { what: format!("{other:?} for not-referenced-by") }),
            },
        },
        Condition::NameMatchesRegex { pattern } => match regex_lite::Regex::new(pattern) {
            Ok(re) => {
                let mut matches = true;
                // GUARD-BEGIN(name-matches-regex)
                matches = re.is_match(name);
                // GUARD-END(name-matches-regex)
                if matches {
                    (true, Found::NameMatches { pattern: pattern.clone() })
                } else {
                    (false, Found::NameDiffers { pattern: pattern.clone() })
                }
            }
            Err(e) => (false, Found::Unsupported { what: format!("pattern {pattern}: {e}") }),
        },
        Condition::DriverVersionNotNewer { vendor } if vendor.eq_ignore_ascii_case("nvidia") => {
            let installed = ctx.nvidia_driver.as_deref().and_then(nvidia_version_from_windows);
            match (installed, artifact_version(item)) {
                (None, _) => (false, Found::NoDriver),
                (_, None) => (false, Found::NoArtifactVersion),
                (Some(i), Some(a)) => {
                    let mut ok = false;
                    // GUARD-BEGIN(driver-version)
                    ok = a <= i;
                    // GUARD-END(driver-version)
                    let (artifact, installed) = (ver(a), ver(i));
                    let found = if a < i {
                        Found::DriverOlder { artifact, installed }
                    } else if a == i {
                        Found::DriverEqual { artifact, installed }
                    } else {
                        Found::DriverNewer { artifact, installed }
                    };
                    (ok, found)
                }
            }
        }
        Condition::DriverVersionNotNewer { vendor } => (false, Found::Unsupported { what: format!("vendor {vendor}") }),
    };
    let detail = found.text(Lang::En);
    CondResult { condition: c.clone(), passed, found, detail }
}

/// A rule that applies to a path.
#[derive(Clone, Debug, Serialize)]
pub struct RuleMatch {
    pub rule: Rule,
    /// The item the rule describes: the path itself, or a folder containing it.
    pub item: String,
    /// The path is inside the item, not the item itself.
    pub inside: bool,
    pub match_results: Vec<CondResult>,
    pub require_results: Vec<CondResult>,
}

impl RuleMatch {
    /// Every `require` condition passed.
    pub fn eligible(&self) -> bool {
        self.require_results.iter().all(|r| r.passed)
    }
}

impl RuleSet {
    /// The first rule that applies to `path`, with its conditions evaluated.
    pub fn find(&self, path: &str, ctx: &Context) -> Option<RuleMatch> {
        let path = canon(path);
        for rule in &self.rules {
            for pattern in &rule.paths {
                let Some(expanded) = ctx.expand(pattern) else { continue };
                let Some(item) = item_of(&canon(&expanded), &path).or_else(|| item_of(&ctx.long_form(&expanded), &path)) else {
                    continue;
                };
                let match_results: Vec<CondResult> = rule.matches.iter().map(|c| eval(c, &item, ctx)).collect();
                if !match_results.iter().all(|r| r.passed) {
                    continue;
                }
                let require_results = rule.require.iter().map(|c| eval(c, &item, ctx)).collect();
                let inside = canon(&item).len() < path.len();
                return Some(RuleMatch { rule: rule.clone(), item, inside, match_results, require_results });
            }
        }
        None
    }

    /// Only whether some rule applies (for flagging unknown folders).
    pub fn covers(&self, path: &str, ctx: &Context) -> Option<String> {
        self.find(path, ctx).map(|m| m.rule.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_rules_are_valid_and_bilingual() {
        let set = RuleSet::built_in();
        assert!(set.rules.len() >= 15);
        let mut ids = HashSet::new();
        for r in &set.rules {
            assert!(ids.insert(r.id.clone()), "duplicate id {}", r.id);
            for (en, pt) in [(&r.title, &r.title_pt_br), (&r.evidence, &r.evidence_pt_br), (&r.cost, &r.cost_pt_br)] {
                assert!(!en.trim().is_empty() && !pt.trim().is_empty(), "{} lacks a language", r.id);
            }
            assert_eq!(r.instructions.is_some(), r.instructions_pt_br.is_some(), "{}", r.id);
            assert!(!r.source.is_empty() && !r.last_verified.is_empty(), "{}", r.id);
            if r.class == Class::NeverTouch || r.class == Class::AppManaged {
                assert_ne!(r.action, Action::Quarantine, "{} can never be quarantined", r.id);
            }
        }
    }

    #[test]
    fn converts_windows_driver_versions_to_nvidia_ones() {
        assert_eq!(nvidia_version_from_windows("32.0.15.6094"), Some((560, 94)));
        assert_eq!(nvidia_version_from_windows("31.0.15.5222"), Some((552, 22)));
        assert_eq!(nvidia_version_from_windows("32.0.15.6614"), Some((566, 14)));
        assert_eq!(nvidia_version_from_windows("30.0.14.9649"), Some((496, 49)));
        assert_eq!(nvidia_version_from_windows("27.21.14.5671"), Some((456, 71)));
        assert_eq!(nvidia_version_from_windows("32.0.15"), None);
        assert_eq!(nvidia_version_from_windows("a.b.c.d"), None);
        assert_eq!(nvidia_version_in_name("NVIDIA-566.14-win11-dch"), Some((566, 14)));
        assert_eq!(nvidia_version_in_name("1566.145"), None);
    }

    #[test]
    fn globs_one_name() {
        assert!(glob("*", "anything"));
        assert!(glob("ms-python.python-*", "MS-Python.Python-2024.1.0"));
        assert!(glob("Cach?", "cache"));
        assert!(!glob("cache", "caches"));
    }

    #[test]
    fn items_and_paths_inside_them() {
        assert_eq!(item_of(r"C:\u\.vscode\extensions\*", r"C:\u\.vscode\extensions\a-1.0"), Some(r"C:\u\.vscode\extensions\a-1.0".into()));
        assert_eq!(item_of(r"C:\u\.vscode\extensions\*", r"C:\u\.vscode\extensions\a-1.0\dist\x.js"), Some(r"C:\u\.vscode\extensions\a-1.0".into()));
        assert_eq!(item_of(r"C:\u\.vscode\extensions\*", r"C:\u\.vscode\extensions"), None);
        assert_eq!(item_of(r"C:\l\Claude-Data\**", r"C:\l\Claude-Data\vm\bundle.vhdx"), Some(r"C:\l\Claude-Data".into()));
        assert_eq!(item_of(r"C:\l\Claude-Data\**", r"C:\l\Other"), None);
    }
}
