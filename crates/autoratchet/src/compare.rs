//! Compare two sides: directories of sources or JSON snapshot files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::thread;

use crate::collect::collect_files;
use crate::config::{Limits, read_limits};
use crate::gate::{FunctionSnapshot, GateOptions, METRICS, Regression, find_regressions, find_untracked};
use crate::overrides::{
    Approval, Override, approval_key, find_overrides_in, match_overrides, new_overrides, source_files,
};
use crate::snapshots::Snapshots;
use crate::tools::Tools;

pub enum Source {
    Files { root: PathBuf, files: Vec<PathBuf> },
    Json(PathBuf),
}

pub struct CompareResult {
    pub previous: Snapshots,
    pub current: Snapshots,
    pub regressions: Vec<Regression>,
    pub untracked: Vec<FunctionSnapshot>,
    /// New overrides on a function: each approves one metric's current score, once.
    pub approvals: Vec<Approval>,
    /// New overrides on no single function (file-wide or misplaced). The gate ignores them.
    pub unmatched_overrides: Vec<Override>,
    /// With `require_override_approval`: every new override, each one a gate failure.
    pub blocked_overrides: Vec<Override>,
    /// Anything either side could not measure. Non-empty means the gate fails.
    pub incomplete: Vec<String>,
    /// The commit `check` compared with.
    pub base_commit: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Incomplete,
}

impl CompareResult {
    pub fn verdict(&self) -> Verdict {
        if !self.incomplete.is_empty() {
            Verdict::Incomplete
        } else if !self.regressions.is_empty() || !self.blocked_overrides.is_empty() {
            Verdict::Fail
        } else {
            Verdict::Pass
        }
    }
}

pub fn source_for(target: &Path) -> Result<Source, String> {
    let abs = std::path::absolute(target).map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
    if meta.is_dir() {
        let files = source_files(&abs).map_err(|e| e.to_string())?;
        return Ok(Source::Files { root: abs, files });
    }
    if abs.extension().is_some_and(|e| e == "json") {
        return Ok(Source::Json(abs));
    }
    Err(format!(
        "Expected a directory of sources or a .json snapshot file, got: {}",
        target.display()
    ))
}

fn snapshots_of(source: &Source, tools: &Tools) -> Result<Snapshots, String> {
    match source {
        Source::Files { root, files } => collect_files(root, files, tools),
        Source::Json(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let list: Vec<FunctionSnapshot> =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut snapshots = Snapshots::default();
            list.into_iter().for_each(|s| snapshots.insert(s));
            Ok(snapshots)
        }
    }
}

pub fn compare_sources(
    base: &Source,
    head: &Source,
    options: &GateOptions,
    tools: &Tools,
) -> Result<CompareResult, String> {
    let (previous, current) = thread::scope(|s| {
        let previous = s.spawn(|| snapshots_of(base, tools));
        let current = s.spawn(|| snapshots_of(head, tools));
        (
            previous.join().expect("base thread"),
            current.join().expect("head thread"),
        )
    });
    let (previous, current) = (previous?, current?);
    let added = match (base, head) {
        (Source::Files { root: br, files: bf }, Source::Files { root: hr, files: hf }) => new_overrides(
            &find_overrides_in(br, bf).map_err(|e| e.to_string())?,
            &find_overrides_in(hr, hf).map_err(|e| e.to_string())?,
        ),
        _ => Vec::new(),
    };
    let strict = options.require_override_approval;
    let (matched, unmatched) = match_overrides(&added, &current);
    let approvals = if strict { Vec::new() } else { matched };
    let approved: HashSet<String> = approvals
        .iter()
        .map(|a| approval_key(&a.override_.file_path, &a.function_name, a.override_.metric))
        .collect();
    let incomplete = previous
        .problems
        .iter()
        .map(|p| format!("base: {p}"))
        .chain(current.problems.iter().map(|p| format!("head: {p}")))
        .collect();
    let mut regressions = find_regressions(&previous, &current, options, &approved);
    sort_regressions(&mut regressions);
    Ok(CompareResult {
        untracked: find_untracked(&previous, &current, options),
        regressions,
        approvals,
        unmatched_overrides: if strict { Vec::new() } else { unmatched },
        blocked_overrides: if strict { added } else { Vec::new() },
        incomplete,
        previous,
        current,
        base_commit: None,
    })
}

pub fn compare_paths(base: &Path, head: &Path, options: &GateOptions, tools: &Tools) -> Result<CompareResult, String> {
    let mut options = options.clone();
    options.limits = options.limits.stricter(limits_in(base)?).stricter(limits_in(head)?);
    compare_sources(&source_for(base)?, &source_for(head)?, &options, tools)
}

/// Limits from the linter configs at the root of a directory. A JSON snapshot has none.
fn limits_in(dir: &Path) -> Result<Limits, String> {
    if !dir.is_dir() {
        return Ok(Limits::default());
    }
    read_limits(|name| std::fs::read_to_string(dir.join(name)).ok())
}

/// File, then function, then metric, so every report comes out the same on every run.
pub fn sort_regressions(regressions: &mut [Regression]) {
    let metric_order = |r: &Regression| METRICS.iter().position(|m| *m == r.metric);
    regressions.sort_by(|a, b| {
        locale_order(&a.file_path, &b.file_path)
            .then_with(|| locale_order(&a.function_name, &b.function_name))
            .then_with(|| metric_order(a).cmp(&metric_order(b)))
    });
}

/// Case-insensitive first, then exact: close to JavaScript's `localeCompare` for code identifiers.
pub fn locale_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}
