//! The ratchet rule, and the regressions it finds between two sets of snapshots.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::align::Identity;

use crate::config::{Lang, Limits};
use crate::lang::Origin;
use crate::snapshots::Snapshots;

pub const DEFAULT_CYCLOMATIC_THRESHOLD: f64 = 10.0;
pub const DEFAULT_COGNITIVE_THRESHOLD: f64 = 15.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    Cyclomatic,
    Cognitive,
    /// Findings from the anti-slop and Oxlint rules.
    Lint,
}

pub const METRICS: [Metric; 3] = [Metric::Cyclomatic, Metric::Cognitive, Metric::Lint];

impl Metric {
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Cyclomatic => "cyclomatic",
            Metric::Cognitive => "cognitive",
            Metric::Lint => "lint",
        }
    }
}

/// A function passes when its score does not exceed the higher of its previous score and the
/// threshold. A new function has no previous score, so it must stay within the threshold.
pub fn passes_complexity_gate(previous: Option<u32>, current: u32, threshold: f64) -> bool {
    f64::from(current) <= f64::from(previous.unwrap_or(0)).max(threshold)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSnapshot {
    pub file_path: String,
    pub function_name: String,
    pub cyclomatic: u32,
    /// Line Oxlint reports the function on. Matches override comments to functions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Byte Oxlint reports it at: which side of a comment on its line it is on.
    #[serde(skip)]
    pub offset: Option<usize>,
    /// What its name comes from.
    #[serde(skip)]
    pub origin: Option<Origin>,
    /// Where its code is, by byte.
    #[serde(skip)]
    pub span: Option<std::ops::Range<usize>>,
    /// Absent in JSON snapshots that predate cognitive scores: not gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cognitive: Option<u32>,
    /// Lint findings inside the function: Oxlint's for JavaScript and TypeScript, AutoRatchet's
    /// own for Rust, Python and Go. Absent: not gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lint: Option<Vec<LintFinding>>,
}

/// How many of base's findings the approved ones account for, so they leave base's count too.
/// Findings pair by rule and, level by level, what tells them apart, approved ones first: of two
/// alike either may be the old one, and an approval must not leave its slot for a new one. Then
/// each approved one left unpaired takes any finding of its rule base has left over, since it
/// may be one that changed. Without identities (JSON input) they pair by rule, open ones first.
fn excused(olds: &[LintFinding], open: &[LintFinding], approvals: &[LintFinding]) -> u32 {
    let told = olds.iter().chain(open).chain(approvals).all(|f| f.identity.is_some());
    let mut left: Vec<&LintFinding> = olds.iter().collect();
    let (mut open, mut approvals): (Vec<_>, Vec<_>) = (open.iter().collect(), approvals.iter().collect());
    if !told {
        pair(&mut left, open, |_| Some([0; 2]));
        return pair(&mut left, approvals, |_| Some([0; 2])).0;
    }
    let mut excused = 0;
    for level in 0..Identity::LEVELS {
        if left.is_empty() || approvals.is_empty() {
            break;
        }
        let key = |f| LintFinding::key(f, level);
        let paired;
        (paired, approvals) = pair(&mut left, approvals, key);
        (_, open) = pair(&mut left, open, key);
        excused += paired;
    }
    excused + pair(&mut left, approvals, |_| Some([0; 2])).0
}

/// Pair findings with base's that are left, of the same rule and `key`: how many paired, and
/// the findings that did not. Those that paired are no longer left.
fn pair<'f>(
    left: &mut Vec<&'f LintFinding>,
    findings: Vec<&'f LintFinding>,
    key: impl Fn(&'f LintFinding) -> Option<[u64; 2]>,
) -> (u32, Vec<&'f LintFinding>) {
    let key = |f: &'f LintFinding| Some((f.rule.as_str(), key(f)?));
    let mut free: HashMap<(&str, [u64; 2]), u32> = HashMap::new();
    for key in left.iter().filter_map(|old| key(old)) {
        *free.entry(key).or_default() += 1;
    }
    let mut taken: HashMap<(&str, [u64; 2]), u32> = HashMap::new();
    let (paired, unpaired): (Vec<_>, Vec<_>) = findings.into_iter().partition(|f| {
        let key = key(f).filter(|key| take(free.get_mut(key)));
        key.map(|key| *taken.entry(key).or_default() += 1).is_some()
    });
    left.retain(|old| !key(old).is_some_and(|key| take(taken.get_mut(&key))));
    (paired.len() as u32, unpaired)
}

fn take(left: Option<&mut u32>) -> bool {
    let Some(left) = left.filter(|n| **n > 0) else {
        return false;
    };
    *left -= 1;
    true
}

/// Name for a file's findings outside every function. No function can have it.
pub const TOP_LEVEL: &str = "(top level)";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LintFinding {
    /// As Oxlint reports it, e.g. `anti-slop(no-reflect-get)`, or `anti-slop-go(no-any-returns)`.
    pub rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
    /// What tells it from others across revisions. Set only where a lint override approves something.
    #[serde(skip)]
    pub identity: Option<Identity>,
    /// Where in its file the linter reported it, by byte.
    #[serde(skip)]
    pub span: Option<std::ops::Range<usize>>,
}

impl LintFinding {
    /// What to pair it by across revisions, at a level of `Identity`.
    fn key(&self, level: usize) -> Option<[u64; 2]> {
        self.identity.as_ref()?.key(level)
    }
}

impl FunctionSnapshot {
    /// The scores the gate compares, before and now, and the findings it reports: none when an
    /// override approved the function's score on this change. For lint, the findings overrides
    /// approved one by one leave the count now; per rule, approvals cover the rule's new
    /// findings first, and only those left over were already in `base`, so leave its count too.
    /// Approving an old finding frees no slot for a new one, and approving a new one takes none
    /// from an old one.
    fn weighed(
        &self,
        base: Option<&FunctionSnapshot>,
        metric: Metric,
        approved: &HashSet<String>,
    ) -> Option<(Option<u32>, u32, Vec<LintFinding>)> {
        use crate::overrides::{approval_key, finding_key};
        if approved.contains(&approval_key(&self.file_path, &self.function_name, metric)) {
            return None;
        }
        let score = self.score(metric)?;
        let before = base.and_then(|b| b.score(metric));
        if metric != Metric::Lint {
            return Some((before, score, self.findings(metric)));
        }
        let (approvals, open): (Vec<_>, Vec<_>) = self
            .findings(metric)
            .into_iter()
            .partition(|f| approved.contains(&finding_key(&self.file_path, &self.function_name, f)));
        let olds = base.map(|b| b.findings(metric)).unwrap_or_default();
        let excused = excused(&olds, &open, &approvals);
        let before = before.map(|b| b.saturating_sub(excused));
        Some((before, open.len() as u32, open))
    }

    pub fn score(&self, metric: Metric) -> Option<u32> {
        match metric {
            Metric::Cyclomatic => Some(self.cyclomatic),
            Metric::Cognitive => self.cognitive,
            Metric::Lint => self.lint.as_ref().map(|l| l.len() as u32),
        }
    }

    /// The findings behind a lint score; none for the complexity metrics.
    pub fn findings(&self, metric: Metric) -> Vec<LintFinding> {
        match metric {
            Metric::Lint => self.lint.clone().unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Worsened,
    New,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Regression {
    pub file_path: String,
    pub function_name: String,
    pub metric: Metric,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<u32>,
    pub current: u32,
    #[serde(serialize_with = "crate::report::js_number")]
    pub threshold: f64,
    /// The linter config file the threshold came from, when it came from one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold_source: Option<String>,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// For a lint regression: every finding in the function now.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<LintFinding>,
}

#[derive(Clone, Debug, Default)]
pub struct GateOptions {
    /// From the command line: wins over `limits` for every language.
    pub cyclomatic_threshold: Option<f64>,
    pub cognitive_threshold: Option<f64>,
    /// From the linter configs the repository already has.
    pub limits: Limits,
    /// A new override approves nothing and fails the gate instead.
    pub require_override_approval: bool,
}

impl GateOptions {
    /// The threshold for a file's language, and the config file it came from, if any.
    pub fn threshold(&self, file: &str, metric: Metric) -> (f64, Option<&str>) {
        let (flag, default) = match metric {
            Metric::Cyclomatic => (self.cyclomatic_threshold, DEFAULT_CYCLOMATIC_THRESHOLD),
            Metric::Cognitive => (self.cognitive_threshold, DEFAULT_COGNITIVE_THRESHOLD),
            // New code gets no findings; existing findings may not grow.
            Metric::Lint => return (0.0, None),
        };
        match (flag, self.limits.get(Lang::of(file), metric)) {
            (Some(n), _) => (n, None),
            (None, Some(limit)) => (limit.value, Some(limit.source.as_str())),
            (None, None) => (default, None),
        }
    }
}

/// `approved` holds what new overrides approved on this change: `approval_key` entries skip a
/// function's score, `finding_key` entries drop one lint finding from its count. Either skips
/// the gate once; measurement continues, so a later change that makes it worse fails.
pub fn find_regressions(
    previous: &Snapshots,
    current: &Snapshots,
    options: &GateOptions,
    approved: &HashSet<String>,
) -> Vec<Regression> {
    let mut out = Vec::new();
    for (key, head) in current.iter() {
        let base = previous.get(key);
        for metric in METRICS {
            let Some((before, now, findings)) = head.weighed(base, metric, approved) else {
                continue;
            };
            // A base snapshot without this metric (older JSON) gives nothing to compare against.
            if base.is_some_and(|b| b.score(metric).is_none()) {
                continue;
            }
            let (threshold, source) = options.threshold(&head.file_path, metric);
            if passes_complexity_gate(before, now, threshold) {
                continue;
            }
            out.push(Regression {
                file_path: head.file_path.clone(),
                function_name: head.function_name.clone(),
                metric,
                previous: before,
                current: now,
                threshold,
                threshold_source: source.map(String::from),
                kind: if base.is_some() { Kind::Worsened } else { Kind::New },
                line: head.line,
                findings,
            });
        }
    }
    out
}

/// Functions over either threshold on base that head no longer reports.
pub fn find_untracked(previous: &Snapshots, current: &Snapshots, options: &GateOptions) -> Vec<FunctionSnapshot> {
    let hot = |s: &FunctionSnapshot| {
        f64::from(s.cyclomatic) > options.threshold(&s.file_path, Metric::Cyclomatic).0
            || f64::from(s.cognitive.unwrap_or(0)) > options.threshold(&s.file_path, Metric::Cognitive).0
    };
    previous
        .iter()
        .filter(|(key, snap)| hot(snap) && current.get(key).is_none())
        .map(|(_, snap)| snap.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratchet_rule() {
        for (previous, current, pass) in [
            (None, 10, true),
            (None, 11, false),
            (Some(7), 10, true),
            (Some(7), 11, false),
            (Some(25), 25, true),
            (Some(25), 26, false),
            (Some(25), 22, true),
            (Some(22), 23, false),
        ] {
            assert_eq!(
                passes_complexity_gate(previous, current, 10.0),
                pass,
                "{previous:?} -> {current}"
            );
        }
    }
}
