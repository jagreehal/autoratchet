//! Override comments: which are new on a change, and which function each one approves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::functions::analyse;
use crate::gate::Metric;
use crate::snapshots::{Snapshots, function_key, normalize_path};

/// A disable directive and the rules it names, up to the `--` that starts its reason:
/// `eslint-disable-next-line complexity, sonarjs/cognitive-complexity -- options bag`.
static DISABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:oxlint|eslint)-disable[A-Za-z0-9-]*[ \t]+([^\n]*)").unwrap());

/// The metrics a comment switches off: `complexity` covers cyclomatic, `sonarjs/cognitive-complexity`
/// and Biome's `noExcessiveCognitiveComplexity` cover cognitive.
fn override_metrics(comment: &str) -> Vec<Metric> {
    let mut metrics = Vec::new();
    if let Some(caps) = DISABLE_RE.captures(comment) {
        let rules = caps[1].split("--").next().unwrap_or("").trim().trim_end_matches("*/");
        let names: Vec<&str> = rules
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .collect();
        if names.iter().any(|r| matches!(*r, "complexity" | "eslint/complexity")) {
            metrics.push(Metric::Cyclomatic);
        }
        if names.iter().any(|r| r.ends_with("/cognitive-complexity")) {
            metrics.push(Metric::Cognitive);
        }
    } else if comment.contains("biome-ignore") && comment.contains("noExcessiveCognitiveComplexity") {
        metrics.push(Metric::Cognitive);
    }
    metrics
}

/// Overrides that target one line. A block `oxlint-disable`, `biome-ignore-all` and `-start` cover more.
static FUNCTION_LEVEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-disable-next-line|-disable-line|biome-ignore\s").unwrap());

static SOURCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.(?:[cm]?[jt]sx?|rs|py|go)$").unwrap());
static DECLARATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.d\.[cm]?ts$").unwrap());

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Override {
    pub file_path: String,
    pub line: u32,
    pub text: String,
    /// `complexity` overrides cover cyclomatic; `sonarjs/cognitive-complexity` and Biome's cover cognitive.
    pub metric: Metric,
    /// Rust, Python and Go: the function the override is attached to; none for one on a file or module.
    #[serde(skip)]
    pub function: Option<String>,
}

/// A new override on a function: it approves that function's current score on one metric.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    #[serde(rename = "override")]
    pub override_: Override,
    pub function_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
}

/// JavaScript, TypeScript, Rust, Python and Go sources the ratchet measures. Declaration files hold no functions.
/// Rust, Python and Go: measured in-process, with overrides that know their function.
pub fn is_native(file: &str) -> bool {
    file.ends_with(".rs") || file.ends_with(".py") || file.ends_with(".go")
}

/// Names, scores and overrides for a Rust, Python or Go file.
pub fn analyse_native(file: &str, source: &str) -> crate::lang::NativeAnalysis {
    if file.ends_with(".py") {
        crate::python::analyse_python(source)
    } else if file.ends_with(".go") {
        crate::go::analyse_go(source)
    } else {
        crate::rust::analyse_rust(source)
    }
}

pub fn is_source_file(file: &str) -> bool {
    // Go's vendored dependencies: someone else's code, whichever way the file was found.
    let vendored_go = file.ends_with(".go") && file.split(['/', '\\']).any(|part| part == "vendor");
    !vendored_go && SOURCE_RE.is_match(file) && !DECLARATION_RE.is_match(file)
}

/// Source files under `dir`, skipping `node_modules` and dot-directories.
pub fn source_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    walk(dir, false)
}

fn walk(dir: &Path, in_vendor: bool) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !skipped_dir(&name, &path) {
                out.extend(walk(&path, in_vendor || name == "vendor")?);
            }
        } else if is_source_file(&name) && !(in_vendor && name.ends_with(".go")) {
            out.push(path);
        }
    }
    Ok(out)
}

/// `node_modules`, dot-directories, and build and environment directories: Cargo's target
/// (CACHEDIR.TAG) and Python virtual environments (pyvenv.cfg).
fn skipped_dir(name: &str, path: &Path) -> bool {
    name == "node_modules"
        || name.starts_with('.')
        || path.join("CACHEDIR.TAG").exists()
        || path.join("pyvenv.cfg").exists()
}

/// One override per metric a real comment switches off. Text that only looks like a directive
/// (inside a string) does not count.
fn overrides_in_file(root: &Path, file: &Path) -> std::io::Result<Vec<Override>> {
    let source = String::from_utf8_lossy(&std::fs::read(file)?).into_owned();
    let file_path = normalize_path(&crate::collect::relative(root, file));
    if is_native(&file_path) {
        let found = analyse_native(&file_path, &source).overrides;
        return Ok(found
            .into_iter()
            .map(|o| Override {
                file_path: file_path.clone(),
                line: o.line,
                text: o.text,
                metric: o.metric,
                function: o.function,
            })
            .collect());
    }
    let mut out = Vec::new();
    for (start, end) in analyse(file, &source).comments {
        let Some(text) = source.get(start as usize..end as usize) else {
            continue;
        };
        let line = source[..start as usize].matches('\n').count() as u32 + 1;
        for metric in override_metrics(text) {
            out.push(Override {
                file_path: file_path.clone(),
                line,
                text: text.trim().to_string(),
                metric,
                function: None,
            });
        }
    }
    Ok(out)
}

pub fn find_overrides_in(root: &Path, files: &[PathBuf]) -> std::io::Result<Vec<Override>> {
    let mut out = Vec::new();
    for file in files {
        out.extend(overrides_in_file(root, file)?);
    }
    Ok(out)
}

/// Overrides in head that base did not have, matched by file and comment text, so moving an
/// existing comment down a few lines does not count as new.
pub fn new_overrides(base: &[Override], head: &[Override]) -> Vec<Override> {
    let mut remaining: HashMap<(&str, &str), usize> = HashMap::new();
    for o in base {
        *remaining.entry((&o.file_path, &o.text)).or_default() += 1;
    }
    head.iter()
        .filter(|o| match remaining.get_mut(&(o.file_path.as_str(), o.text.as_str())) {
            Some(left) if *left > 0 => {
                *left -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// Match each new override to the function it targets: on its own line (`disable-line`) or on
/// the next line (`disable-next-line` / `biome-ignore`). File-wide or misplaced overrides match
/// nothing: the gate ignores them, and the report says so.
pub fn match_overrides(overrides: &[Override], current: &Snapshots) -> (Vec<Approval>, Vec<Override>) {
    let mut approvals = Vec::new();
    let mut unmatched = Vec::new();
    for o in overrides {
        // Rust, Python and Go overrides know their function from the syntax tree.
        if is_native(&o.file_path) {
            let f = o.function.as_ref().and_then(|name| {
                current
                    .values()
                    .find(|s| s.file_path == o.file_path && &s.function_name == name)
            });
            match f {
                Some(f) => approvals.push(Approval {
                    override_: o.clone(),
                    function_name: f.function_name.clone(),
                    score: f.score(o.metric),
                }),
                None => unmatched.push(o.clone()),
            }
            continue;
        }
        if !FUNCTION_LEVEL_RE.is_match(&o.text) {
            unmatched.push(o.clone());
            continue;
        }
        let line = if o.text.contains("-disable-line") {
            o.line
        } else {
            o.line + 1
        };
        match current
            .values()
            .find(|s| s.file_path == o.file_path && s.line == Some(line))
        {
            Some(f) => approvals.push(Approval {
                override_: o.clone(),
                function_name: f.function_name.clone(),
                score: f.score(o.metric),
            }),
            None => unmatched.push(o.clone()),
        }
    }
    (approvals, unmatched)
}

pub fn approval_key(file_path: &str, function_name: &str, metric: Metric) -> String {
    format!("{}\0{}", function_key(file_path, function_name), metric.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_comments() {
        use Metric::{Cognitive, Cyclomatic};
        assert_eq!(
            override_metrics("// oxlint-disable-next-line complexity -- options bag"),
            [Cyclomatic]
        );
        assert_eq!(override_metrics("/* oxlint-disable complexity */"), [Cyclomatic]);
        assert_eq!(
            override_metrics("// eslint-disable-next-line sonarjs/cognitive-complexity -- spec"),
            [Cognitive]
        );
        assert_eq!(
            override_metrics("// eslint-disable-next-line complexity, sonarjs/cognitive-complexity"),
            [Cyclomatic, Cognitive]
        );
        assert_eq!(
            override_metrics("// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: x"),
            [Cognitive]
        );
        // The reason is not a rule list.
        assert_eq!(
            override_metrics("// eslint-disable-next-line no-console -- complexity is fine"),
            []
        );
        assert_eq!(override_metrics("// oxlint-disable-next-line no-console"), []);
        assert_eq!(override_metrics("// biome-ignore lint/complexity/noForEach: fine"), []);
    }

    #[test]
    fn source_file_filter() {
        assert!(is_source_file("a.ts") && is_source_file("a.mjs") && is_source_file("a.tsx"));
        assert!(!is_source_file("a.d.ts") && !is_source_file("a.json"));
        // Go's vendored dependencies, whichever way the file was found.
        assert!(!is_source_file("vendor/github.com/x/y.go") && !is_source_file("cmd/vendor/x.go"));
        assert!(is_source_file("vendor/lib.js") && is_source_file("vendored/x.go"));
    }
}
