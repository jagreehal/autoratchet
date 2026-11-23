//! Override comments: which are new on a change, and which function each one approves.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::align::Identity;
use crate::functions::analyse;
use crate::gate::{FunctionSnapshot, LintFinding, Metric};
use crate::snapshots::{Snapshots, function_key, normalize_path};

/// Oxlint's and ESLint's directives: the keyword starts the comment's body, as Oxlint reads it, so
/// prose that mentions one is not one. What it covers, and the rest, rules and reason:
/// `eslint-disable-next-line complexity, sonarjs/cognitive-complexity -- options bag`.
static DISABLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)^(?:oxlint|eslint)-disable(-next-line|-line)?(?:\s+(.*))?$").expect("valid regex")
});

/// Biome's, and the rule it names, up to the `:` that starts its reason.
static BIOME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[*\s]*biome-ignore(-\w+)?\s+([^:]*)").expect("valid regex"));

/// A comment without its delimiters.
fn body(comment: &str) -> &str {
    let comment = comment.trim();
    let block = comment.strip_prefix("/*").map(|c| c.strip_suffix("*/").unwrap_or(c));
    let line = || comment.strip_prefix("//").or_else(|| comment.strip_prefix('#'));
    block.or_else(line).unwrap_or(comment).trim()
}

/// The metrics a comment switches off: `complexity` and `sonarjs/cyclomatic-complexity` cover
/// cyclomatic, `sonarjs/cognitive-complexity` and Biome's `noExcessiveCognitiveComplexity` cover
/// cognitive, and the ratcheted lint rules cover lint.
fn override_metrics(comment: &str) -> Vec<Metric> {
    let mut metrics = Vec::new();
    if DISABLE_RE.is_match(body(comment)) {
        let names = disabled_rules(comment);
        if names
            .iter()
            .any(|r| matches!(*r, "complexity" | "eslint/complexity") || r.ends_with("/cyclomatic-complexity"))
        {
            metrics.push(Metric::Cyclomatic);
        }
        if names.iter().any(|r| r.ends_with("/cognitive-complexity")) {
            metrics.push(Metric::Cognitive);
        }
        if names.iter().any(|r| crate::collect::is_lint_rule(r)) {
            metrics.push(Metric::Lint);
        }
    } else if BIOME_RE
        .captures(body(comment))
        .is_some_and(|c| c[2].contains("noExcessiveCognitiveComplexity"))
    {
        metrics.push(Metric::Cognitive);
    }
    metrics
}

/// The rules a disable comment names, up to the `--` before its reason: Oxlint's and ESLint's
/// spelling, or AutoRatchet's own for Rust, Python and Go.
fn disabled_rules(comment: &str) -> Vec<&str> {
    let Some(caps) = DISABLE_RE.captures(body(comment)) else {
        return crate::lang::directive_rules(comment);
    };
    let rest = caps.get(2).map_or("", |m| m.as_str());
    let rules = rest.split("--").next().unwrap_or("");
    let names = rules.split(|c: char| c == ',' || c.is_whitespace());
    names.filter(|r| !r.is_empty()).collect()
}

/// Whether a comment names the rule of a finding: `eqeqeq` or `eslint/eqeqeq` for `eslint(eqeqeq)`,
/// `anti-slop-go/no-any-returns` for `anti-slop-go(no-any-returns)`.
fn names_rule(comment: &str, code: &str) -> bool {
    let rule = code.split_once('(').map_or(code, |(_, r)| r.trim_end_matches(')'));
    disabled_rules(comment)
        .iter()
        .any(|r| r.rsplit('/').next() == Some(rule))
}

/// What of the code a directive covers, when no more than a line: the rest of a file is not a
/// function's or a finding's override (a block `oxlint-disable`, `biome-ignore-all`, `-start`).
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    /// Code before the comment, on its line.
    Line,
    /// Code from the comment's end to the end of the next line.
    NextLine,
}

fn scope(comment: &str) -> Option<Scope> {
    let body = body(comment);
    if let Some(caps) = DISABLE_RE.captures(body) {
        return match caps.get(1)?.as_str() {
            "-line" => Some(Scope::Line),
            _ => Some(Scope::NextLine),
        };
    }
    let biome = BIOME_RE.captures(body).is_some_and(|caps| caps.get(1).is_none());
    (biome || body.starts_with("autoratchet-disable-next-line")).then_some(Scope::NextLine)
}

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
    /// JavaScript and TypeScript: where the comment is.
    #[serde(skip)]
    pub comment: Option<CommentSpan>,
}

/// Where a comment is, by byte, and the line it ends on.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentSpan {
    pub start: usize,
    pub end: usize,
    pub end_line: u32,
}

impl Override {
    fn end_line(&self) -> u32 {
        self.comment.as_ref().map_or(self.line, |c| c.end_line)
    }

    /// Whether this covers code reported on `line`, at byte `offset`.
    fn covers(&self, line: Option<u32>, offset: Option<usize>) -> bool {
        let (Some(line), Some(scope)) = (line, scope(&self.text)) else {
            return false;
        };
        // On the comment's own line, which side of it the code is on decides.
        let side = self.comment.as_ref().zip(offset);
        match scope {
            Scope::Line => line == self.line && side.is_none_or(|(c, at)| at < c.start),
            Scope::NextLine => {
                let end = self.end_line();
                line == end + 1 || (line == end && side.is_some_and(|(c, at)| at >= c.end))
            }
        }
    }
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
        let mut analysis = crate::rust::analyse_rust(source);
        if crate::slop::is_rust_test_file(file) {
            analysis.findings.retain(|f| f.rule != "anti-slop-rust(no-unwrap)");
        }
        analysis
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
                comment: None,
            })
            .collect());
    }
    let mut out = Vec::new();
    for (start, end) in analyse(file, &source).comments {
        let Some(text) = source.get(start as usize..end as usize) else {
            continue;
        };
        let line_of = |at: u32| source[..at as usize].matches('\n').count() as u32 + 1;
        let line = line_of(start);
        for metric in override_metrics(text) {
            out.push(Override {
                file_path: file_path.clone(),
                line,
                // On one line, however it was written: reports show it, and base and head compare it.
                text: text.split_whitespace().collect::<Vec<_>>().join(" "),
                metric,
                function: None,
                comment: Some(CommentSpan {
                    start: start as usize,
                    end: end as usize,
                    end_line: line_of(end),
                }),
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

type Directive<'a> = (&'a str, &'a str, Metric);

fn directive(o: &Override) -> Directive<'_> {
    (&o.file_path, &o.text, o.metric)
}

/// The directives head has more of than base, which has some: copies, to be told apart.
pub fn copied<'a>(base: &'a [Override], head: &'a [Override]) -> HashSet<Directive<'a>> {
    let mut count: HashMap<Directive, (usize, usize)> = HashMap::new();
    base.iter().for_each(|o| count.entry(directive(o)).or_default().0 += 1);
    head.iter().for_each(|o| count.entry(directive(o)).or_default().1 += 1);
    let copied = count.into_iter().filter(|(_, (old, new))| *old > 0 && new > old);
    copied.map(|(directive, _)| directive).collect()
}

/// What each override covers, by name.
type Targets = Vec<Vec<String>>;

/// Overrides in head that base did not have, matched by file and comment text, so moving an
/// existing comment down a few lines does not count as new. Each of `targets`, surest first, names
/// what base's and head's cover: one covering what a base override of the same text covered is that
/// one, before any other of the text, so a copy of a directive put above new code is the new one.
pub fn new_overrides(
    base: &[Override],
    head: &[Override],
    targets: impl Iterator<Item = (Targets, Targets)>,
) -> Vec<Override> {
    let mut used = vec![false; base.len()];
    let mut kept = vec![false; head.len()];
    let text_alone = (
        vec![vec![String::new()]; base.len()],
        vec![vec![String::new()]; head.len()],
    );
    for (base_targets, head_targets) in targets.chain([text_alone]) {
        let mut free = by_target(base, &base_targets);
        for ((o, targets), kept) in head.iter().zip(&head_targets).zip(&mut kept) {
            *kept = *kept || claim(o, targets, &mut free, &mut used);
        }
    }
    let new = head.iter().zip(kept).filter(|(_, kept)| !kept);
    new.map(|(o, _)| o.clone()).collect()
}

/// Base's overrides by directive and what they cover, the first of them last, to pop.
fn by_target<'a>(base: &'a [Override], targets: &'a Targets) -> HashMap<(Directive<'a>, &'a str), Vec<usize>> {
    let mut found: HashMap<_, Vec<usize>> = HashMap::new();
    for (i, (o, targets)) in base.iter().zip(targets).enumerate().rev() {
        for target in targets {
            found.entry((directive(o), target.as_str())).or_default().push(i);
        }
    }
    found
}

/// Take a base override, not used yet, of this directive covering one of these.
fn claim<'a>(
    o: &'a Override,
    targets: &'a [String],
    free: &mut HashMap<(Directive<'a>, &'a str), Vec<usize>>,
    used: &mut [bool],
) -> bool {
    targets.iter().any(|target| {
        let candidates = free.get_mut(&(directive(o), target.as_str()));
        candidates.is_some_and(|c| std::iter::from_fn(|| c.pop()).any(|i| !std::mem::replace(&mut used[i], true)))
    })
}

/// What overrides in some files may cover, by file and line.
#[derive(Default)]
pub struct Coverable<'s> {
    /// With the key of the function each is in, which base's and head's share.
    findings: HashMap<(&'s str, u32), Vec<(&'s str, &'s LintFinding)>>,
    functions: HashMap<(&'s str, u32), Vec<(&'s str, &'s FunctionSnapshot)>>,
    keys: HashMap<(&'s str, &'s str), &'s str>,
}

impl<'s> Coverable<'s> {
    pub fn of(snapshots: &'s Snapshots, files: &HashSet<&str>) -> Self {
        let mut all = Coverable::default();
        for (key, s) in snapshots.iter().filter(|(_, s)| files.contains(s.file_path.as_str())) {
            let file = s.file_path.as_str();
            all.keys.insert((file, s.function_name.as_str()), key);
            if let Some(line) = s.line {
                all.functions.entry((file, line)).or_default().push((key, s));
            }
            for (line, f) in s.lint.iter().flatten().filter_map(|f| Some((f.line?, f))) {
                all.findings.entry((file, line)).or_default().push((key, f));
            }
        }
        all
    }

    /// What an override covers: each lint finding it names there, or the function it sits on.
    /// `None` when it covers nothing measured.
    pub fn targets(
        &self,
        o: &Override,
        identity: impl Fn(&LintFinding) -> Option<Identity>,
    ) -> Option<Vec<(String, Option<Identity>)>> {
        if let Some(function) = o.function.as_ref().filter(|_| o.metric != Metric::Lint) {
            let key = self.keys.get(&(o.file_path.as_str(), function.as_str()));
            return Some(vec![(key.map_or(function.as_str(), |key| key).to_string(), None)]);
        }
        let lines = [o.end_line(), o.end_line() + 1]
            .into_iter()
            .chain((o.end_line() != o.line).then_some(o.line));
        let at = lines.map(|line| (o.file_path.as_str(), line));
        if o.metric != Metric::Lint {
            let functions = at.filter_map(|at| self.functions.get(&at)).flatten();
            let mut covered = functions.filter(|(_, s)| o.covers(s.line, s.offset));
            return covered.next().map(|(key, _)| vec![(key.to_string(), None)]);
        }
        let findings = at.filter_map(|at| self.findings.get(&at)).flatten();
        let covered: Vec<_> = findings.filter(|(_, f)| covers_finding(o, f)).collect();
        let targets = covered
            .iter()
            .filter_map(|(key, f)| Some((format!("{key}\0{}", f.rule), Some(identity(f)?))));
        (!covered.is_empty()).then(|| targets.collect())
    }
}

/// Match each new override to the function it targets: on its own line (`disable-line`) or on
/// the next line (`disable-next-line` / `biome-ignore`). File-wide or misplaced overrides match
/// nothing: the gate ignores them, and the report says so.
pub fn match_overrides(overrides: &[Override], current: &Snapshots) -> (Vec<Approval>, Vec<Override>) {
    let mut approvals = Vec::new();
    let mut unmatched = Vec::new();
    for o in overrides {
        // Rust, Python and Go overrides know their function from the syntax tree; a lint
        // override, in any language, sits above its finding instead.
        if is_native(&o.file_path) && o.metric != Metric::Lint {
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
        // A lint override sits on the finding, which may be anywhere in the function.
        let targets = |s: &&FunctionSnapshot| match o.metric {
            Metric::Lint => s.lint.iter().flatten().any(|f| covers_finding(o, f)),
            _ => o.covers(s.line, s.offset),
        };
        // A lint directive covers every function with a named finding on its line; a complexity
        // one the function that starts there.
        let mut targets: Vec<_> = current
            .values()
            .filter(|s| s.file_path == o.file_path)
            .filter(targets)
            .collect();
        if o.metric != Metric::Lint {
            targets.truncate(1);
        }
        if targets.is_empty() {
            unmatched.push(o.clone());
        }
        approvals.extend(targets.into_iter().map(|f| Approval {
            override_: o.clone(),
            function_name: f.function_name.clone(),
            score: f.score(o.metric),
        }));
    }
    (approvals, unmatched)
}

pub fn approval_key(file_path: &str, function_name: &str, metric: Metric) -> String {
    format!("{}\0{}", function_key(file_path, function_name), metric.as_str())
}

/// One lint finding's approval key: its function, place and rule.
pub fn finding_key(file_path: &str, function_name: &str, finding: &LintFinding) -> String {
    let key = approval_key(file_path, function_name, Metric::Lint);
    let at = finding.span.as_ref().map_or(0, |span| span.start);
    format!("{key}\0{}\0{at}\0{}", finding.line.unwrap_or(0), finding.rule)
}

/// The line an override's target is usually on: its own for `disable-line`, else the next.
pub fn target_line(o: &Override) -> Option<u32> {
    scope(&o.text).map(|scope| match scope {
        Scope::Line => o.line,
        Scope::NextLine => o.end_line() + 1,
    })
}

/// Whether a lint override approves a finding: it covers its code and names its rule.
fn covers_finding(o: &Override, f: &LintFinding) -> bool {
    o.covers(f.line, f.span.as_ref().map(|span| span.start)) && names_rule(&o.text, &f.rule)
}

/// What the gate skips: a function's score for a complexity override; for a lint override,
/// each finding it names on its line, so the function's other new findings still count.
pub fn approved_keys(approvals: &[Approval], current: &Snapshots) -> HashSet<String> {
    let mut keys = HashSet::new();
    for a in approvals {
        let o = &a.override_;
        if o.metric != Metric::Lint {
            keys.insert(approval_key(&o.file_path, &a.function_name, o.metric));
            continue;
        }
        let function = current.get(&function_key(&o.file_path, &a.function_name));
        let findings = function.and_then(|f| f.lint.as_ref()).into_iter().flatten();
        let named = findings.filter(|f| covers_finding(o, f));
        keys.extend(named.map(|f| finding_key(&o.file_path, &a.function_name, f)));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lint_override_approves_only_the_finding_it_names() {
        use crate::gate::{GateOptions, find_regressions};
        let finding = |line, rule: &str| LintFinding {
            rule: rule.into(),
            line: Some(line),
            message: String::new(),
            identity: None,
            span: None,
        };
        let snapshot = |lint| FunctionSnapshot {
            file_path: "a.py".into(),
            function_name: "f".into(),
            cyclomatic: 1,
            line: Some(1),
            cognitive: Some(0),
            lint: Some(lint),
            ..Default::default()
        };
        let cast = "anti-slop-python(require-safety-comment-for-cast)";
        let any = "anti-slop-python(no-any-returns)";
        let (mut base, mut head) = (Snapshots::default(), Snapshots::default());
        base.insert(snapshot(vec![]));
        head.insert(snapshot(vec![finding(3, cast), finding(3, any), finding(5, cast)]));
        let approval = Approval {
            override_: Override {
                file_path: "a.py".into(),
                line: 2,
                text: "# autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- checked"
                    .into(),
                metric: Metric::Lint,
                function: Some("f".into()),
                comment: None,
            },
            function_name: "f".into(),
            score: Some(3),
        };
        let approved = approved_keys(&[approval], &head);
        assert_eq!(approved.len(), 1);
        let regressions = find_regressions(&base, &head, &GateOptions::default(), &approved);
        let lint: Vec<_> = regressions.iter().filter(|r| r.metric == Metric::Lint).collect();
        // The approved cast on line 3 drops out; the other rule on that line and the cast on line 5 count.
        assert_eq!(lint.len(), 1);
        assert_eq!(lint[0].current, 2);
        let left: Vec<_> = lint[0].findings.iter().map(|f| (f.line, f.rule.as_str())).collect();
        assert_eq!(left, [(Some(3), any), (Some(5), cast)]);

        // Approving a finding base already had frees no slot for a new one.
        let (mut base, mut head) = (Snapshots::default(), Snapshots::default());
        base.insert(snapshot(vec![finding(3, "eslint(eqeqeq)")]));
        head.insert(snapshot(vec![
            finding(3, "eslint(eqeqeq)"),
            finding(4, "eslint(no-debugger)"),
        ]));
        let old = Approval {
            override_: Override {
                line: 2,
                text: "// oxlint-disable-next-line eqeqeq -- legacy".into(),
                ..approval_override()
            },
            function_name: "f".into(),
            score: Some(2),
        };
        let approved = approved_keys(&[old], &head);
        let regressions = find_regressions(&base, &head, &GateOptions::default(), &approved);
        let lint: Vec<_> = regressions.iter().filter(|r| r.metric == Metric::Lint).collect();
        assert_eq!((lint.len(), lint[0].previous, lint[0].current), (1, Some(0), 1));

        // Approving a new finding takes no slot from an unchanged old one of the same rule.
        let (mut base, mut head) = (Snapshots::default(), Snapshots::default());
        base.insert(snapshot(vec![finding(3, "eslint(eqeqeq)")]));
        head.insert(snapshot(vec![
            finding(3, "eslint(eqeqeq)"),
            finding(5, "eslint(eqeqeq)"),
        ]));
        let new = Approval {
            override_: Override {
                line: 4,
                text: "// oxlint-disable-next-line eqeqeq -- loose on purpose".into(),
                ..approval_override()
            },
            function_name: "f".into(),
            score: Some(2),
        };
        let approved = approved_keys(&[new], &head);
        let regressions = find_regressions(&base, &head, &GateOptions::default(), &approved);
        assert!(regressions.iter().all(|r| r.metric != Metric::Lint), "{regressions:?}");
    }

    #[test]
    fn an_approved_finding_the_alignment_lost_still_takes_its_old_slot() {
        use crate::gate::{GateOptions, find_regressions};
        let finding = |line, place: u64| LintFinding {
            rule: "eslint(eqeqeq)".into(),
            line: Some(line),
            message: String::new(),
            identity: Some(Identity {
                place: Some(place),
                told: vec![Some(place)],
                ..Default::default()
            }),
            span: None,
        };
        let snapshot = |lint| FunctionSnapshot {
            file_path: "a.ts".into(),
            function_name: "f".into(),
            cyclomatic: 1,
            line: Some(1),
            cognitive: Some(0),
            lint: Some(lint),
            ..Default::default()
        };
        let (mut base, mut head) = (Snapshots::default(), Snapshots::default());
        base.insert(snapshot(vec![finding(2, 20)]));
        // Neither head finding pairs: the approved one is the old one, moved past recognition.
        head.insert(snapshot(vec![finding(2, 2), finding(4, 4)]));
        let approval = Approval {
            override_: Override {
                file_path: "a.ts".into(),
                line: 3,
                text: "// oxlint-disable-next-line eqeqeq -- old".into(),
                ..approval_override()
            },
            function_name: "f".into(),
            score: Some(2),
        };
        let approved = approved_keys(&[approval], &head);
        let regressions = find_regressions(&base, &head, &GateOptions::default(), &approved);
        let lint: Vec<_> = regressions.iter().filter(|r| r.metric == Metric::Lint).collect();
        assert_eq!((lint.len(), lint[0].previous, lint[0].current), (1, Some(0), 1));
    }

    #[test]
    fn a_copied_directive_is_the_new_one() {
        let directive = |line| Override {
            file_path: "a.ts".into(),
            line,
            text: "// oxlint-disable-next-line eqeqeq -- loose".into(),
            metric: Metric::Lint,
            function: None,
            comment: None,
        };
        let (base, head) = ([directive(2)], [directive(2), directive(4)]);
        let lines = |added: Vec<Override>| added.iter().map(|o| o.line).collect::<Vec<_>>();
        let (old, none) = (vec!["eqeqeq 20..26".to_string()], Vec::new());
        // Head's second covers what base's one covered; the first is the copy.
        let targets = vec![none.clone(), old.clone()];
        assert_eq!(
            lines(new_overrides(&base, &head, [(vec![old], targets)].into_iter())),
            [2]
        );
        // Without anything to tell them apart, text alone decides.
        let targets = vec![none.clone(), none.clone()];
        assert_eq!(
            lines(new_overrides(&base, &head, [(vec![none], targets)].into_iter())),
            [4]
        );
    }

    #[test]
    fn a_directive_starts_its_comment_and_covers_what_oxlint_says() {
        // Prose, documentation and `///` mention directives; the keyword must come first.
        for prose in [
            "// Example syntax: oxlint-disable-next-line eqeqeq -- reason",
            "/* see oxlint-disable-next-line eqeqeq */",
            "/** oxlint-disable-next-line eqeqeq */",
            "/// oxlint-disable-next-line eqeqeq",
            "// try eslint-disable-line complexity",
            "// oxlint-disable-next-lines eqeqeq",
            "// e.g. biome-ignore lint/complexity/noExcessiveCognitiveComplexity: x",
            "// biome-ignore lint/x: as for noExcessiveCognitiveComplexity",
        ] {
            assert_eq!(override_metrics(prose), [], "{prose}");
        }
        for directive in [
            "//oxlint-disable-next-line eqeqeq",
            "/* oxlint-disable-next-line\n   no-eval,\n   eqeqeq -- reason */",
        ] {
            assert_eq!(override_metrics(directive), [Metric::Lint], "{directive}");
        }
        // A comment from byte 10 to 60, lines 3 to 4.
        let covers = |keyword: &str, line, at| {
            let o = Override {
                line: 3,
                text: format!("/* {keyword} eqeqeq -- r */"),
                comment: Some(CommentSpan {
                    start: 10,
                    end: 60,
                    end_line: 4,
                }),
                ..approval_override()
            };
            o.covers(Some(line), Some(at))
        };
        let next = |line, at| covers("oxlint-disable-next-line", line, at);
        assert_eq!(
            (next(5, 70), next(4, 62), next(4, 5), next(6, 90)),
            (true, true, false, false)
        );
        let same = |line, at| covers("oxlint-disable-line", line, at);
        assert_eq!((same(3, 5), same(4, 62), same(3, 62)), (true, false, false));
    }

    #[test]
    fn a_reason_does_not_change_what_a_directive_targets() {
        let target = |text: &str| {
            target_line(&Override {
                text: text.into(),
                line: 7,
                ..approval_override()
            })
        };
        assert_eq!(
            target("// oxlint-disable-next-line eqeqeq -- not a -disable-line"),
            Some(8)
        );
        assert_eq!(
            target("// eslint-disable-line eqeqeq -- see oxlint-disable-next-line"),
            Some(7)
        );
        assert_eq!(target("// biome-ignore lint/x: like eslint-disable-line"), Some(8));
        assert_eq!(target("# autoratchet-disable-next-line a/b -- why"), Some(8));
        // These cover more than a line, whatever their reason says.
        assert_eq!(
            target("/* oxlint-disable eqeqeq -- not oxlint-disable-next-line */"),
            None
        );
        assert_eq!(target("// biome-ignore-all lint/x: not eslint-disable-line"), None);
    }

    fn approval_override() -> Override {
        Override {
            file_path: "a.py".into(),
            line: 2,
            text: String::new(),
            metric: Metric::Lint,
            function: Some("f".into()),
            comment: None,
        }
    }

    #[test]
    fn a_lint_override_approves_every_function_on_its_line() {
        let snapshot = |name: &str| FunctionSnapshot {
            file_path: "a.ts".into(),
            function_name: name.into(),
            cyclomatic: 1,
            line: Some(2),
            cognitive: Some(0),
            lint: Some(vec![LintFinding {
                rule: "eslint(eqeqeq)".into(),
                line: Some(2),
                message: String::new(),
                identity: None,
                span: None,
            }]),
            ..Default::default()
        };
        let mut head = Snapshots::default();
        head.insert(snapshot("a"));
        head.insert(snapshot("b"));
        let directive = Override {
            file_path: "a.ts".into(),
            line: 1,
            text: "// oxlint-disable-next-line eqeqeq -- both compare loosely on purpose".into(),
            metric: Metric::Lint,
            function: None,
            comment: None,
        };
        let (approvals, unmatched) = match_overrides(std::slice::from_ref(&directive), &head);
        let mut names: Vec<_> = approvals.iter().map(|a| a.function_name.as_str()).collect();
        names.sort_unstable();
        assert_eq!((names, unmatched.len()), (vec!["a", "b"], 0));
        // A complexity directive approves the one function.
        let complexity = Override {
            text: "// oxlint-disable-next-line complexity -- table".into(),
            metric: Metric::Cyclomatic,
            ..directive
        };
        assert_eq!(match_overrides(&[complexity], &head).0.len(), 1);
    }

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
        assert_eq!(
            override_metrics("// eslint-disable-next-line sonarjs/cyclomatic-complexity -- spec"),
            [Cyclomatic]
        );
        assert_eq!(override_metrics("// oxlint-disable-next-line no-console"), []);
        assert_eq!(override_metrics("// biome-ignore lint/complexity/noForEach: fine"), []);
    }

    #[test]
    fn lint_override_names_the_rule() {
        assert!(names_rule("// eslint-disable-next-line eqeqeq -- x", "eslint(eqeqeq)"));
        assert!(names_rule(
            "# autoratchet-disable-next-line anti-slop-python/no-any-returns -- x",
            "anti-slop-python(no-any-returns)"
        ));
        assert!(!names_rule(
            "# autoratchet-disable-next-line anti-slop-python/no-any-returns -- x",
            "anti-slop-python(no-any-parameters)"
        ));
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
