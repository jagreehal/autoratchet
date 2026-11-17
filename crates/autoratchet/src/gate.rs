//! The ratchet rule, and the regressions it finds between two sets of snapshots.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::config::{Lang, Limits};
use crate::snapshots::Snapshots;

pub const DEFAULT_CYCLOMATIC_THRESHOLD: f64 = 10.0;
pub const DEFAULT_COGNITIVE_THRESHOLD: f64 = 15.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    Cyclomatic,
    Cognitive,
}

pub const METRICS: [Metric; 2] = [Metric::Cyclomatic, Metric::Cognitive];

impl Metric {
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Cyclomatic => "cyclomatic",
            Metric::Cognitive => "cognitive",
        }
    }
}

/// A function passes when its score does not exceed the higher of its previous score and the
/// threshold. A new function has no previous score, so it must stay within the threshold.
pub fn passes_complexity_gate(previous: Option<u32>, current: u32, threshold: f64) -> bool {
    f64::from(current) <= f64::from(previous.unwrap_or(0)).max(threshold)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSnapshot {
    pub file_path: String,
    pub function_name: String,
    pub cyclomatic: u32,
    /// Line Oxlint reports the function on. Matches override comments to functions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Absent in JSON snapshots that predate cognitive scores: not gated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cognitive: Option<u32>,
}

impl FunctionSnapshot {
    pub fn score(&self, metric: Metric) -> Option<u32> {
        match metric {
            Metric::Cyclomatic => Some(self.cyclomatic),
            Metric::Cognitive => self.cognitive,
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
        };
        match (flag, self.limits.get(Lang::of(file), metric)) {
            (Some(n), _) => (n, None),
            (None, Some(limit)) => (limit.value, Some(limit.source.as_str())),
            (None, None) => (default, None),
        }
    }
}

/// `approved` holds `approval_key` entries a new override approved on this change. Those skip
/// the gate once; measurement continues, so a later change that makes the function worse fails.
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
            if approved.contains(&crate::overrides::approval_key(
                &head.file_path,
                &head.function_name,
                metric,
            )) {
                continue;
            }
            let Some(now) = head.score(metric) else { continue };
            // A base snapshot without this metric (older JSON) gives nothing to compare against.
            if base.is_some_and(|b| b.score(metric).is_none()) {
                continue;
            }
            let (threshold, source) = options.threshold(&head.file_path, metric);
            let before = base.and_then(|b| b.score(metric));
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
