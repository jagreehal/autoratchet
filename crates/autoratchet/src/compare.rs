//! Compare two sides: directories of sources or JSON snapshot files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::thread;

use crate::align::{Alignment, Identity};
use crate::collect::collect_files;
use crate::config::{Limits, read_limits};
use crate::gate::{
    FunctionSnapshot, GateOptions, LintFinding, METRICS, Metric, Regression, find_regressions, find_untracked,
};
use crate::namesakes;
use crate::overrides::{
    Approval, Coverable, Override, approved_keys, copied, find_overrides_in, match_overrides, new_overrides,
    source_files, target_line,
};
use crate::snapshots::{Snapshots, function_key};
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

/// Marks each lint finding of these functions with where it sits across revisions, so the gate
/// can tell a finding a change kept from a new one, however code moved.
fn align(
    previous: &mut Snapshots,
    current: &mut Snapshots,
    functions: &HashSet<String>,
    alignments: &HashMap<String, Alignment>,
) {
    let mark = |snapshots: &mut Snapshots, identity: fn(&Alignment, &LintFinding) -> Option<Identity>| {
        for (_, snapshot) in snapshots.iter_mut().filter(|(key, _)| functions.contains(*key)) {
            let Some(alignment) = alignments.get(&snapshot.file_path) else {
                continue;
            };
            let findings = snapshot.lint.iter_mut().flatten();
            findings.for_each(|f| f.identity = identity(alignment, f));
        }
    };
    mark(previous, base_identity);
    mark(current, head_identity);
}

fn base_identity(alignment: &Alignment, finding: &LintFinding) -> Option<Identity> {
    alignment.base_identity(finding.span.as_ref()?)
}

fn head_identity(alignment: &Alignment, finding: &LintFinding) -> Option<Identity> {
    alignment.head_identity(finding.span.as_ref()?)
}

/// The overrides head added. Where head has more copies of a directive than base, its file is
/// lined up, and each copy is matched to the base directive covering the same code first.
fn added_overrides(
    (old, new): (&[Override], &[Override]),
    (previous, current): (&Snapshots, &Snapshots),
    (base_root, head_root): (&Path, &Path),
    alignments: &mut HashMap<String, Alignment>,
) -> Vec<Override> {
    let copied = copied(old, new);
    let files: HashSet<&str> = copied.iter().map(|(file, ..)| *file).collect();
    line_up(files.iter().copied(), (base_root, head_root), alignments);
    let (before, now) = (Coverable::of(previous, &files), Coverable::of(current, &files));
    let copy = |o: &Override| copied.contains(&(o.file_path.as_str(), o.text.as_str(), o.metric));
    let lined_up = |o: &Override| alignments.get(&o.file_path).filter(|_| copy(o));
    let covered_before = |o: &Override| {
        let alignment = lined_up(o)?;
        let found = before.targets(o, |f| base_identity(alignment, f));
        found.or_else(|| Some(vec![(format!("line {}", target_line(o)?), None)]))
    };
    let covered_now = |o: &Override| {
        let alignment = lined_up(o)?;
        let found = now.targets(o, |f| head_identity(alignment, f));
        found.or_else(|| Some(vec![(format!("line {}", alignment.base_line(target_line(o)?)?), None)]))
    };
    let old_covered: Vec<_> = old.iter().map(|o| covered_before(o).unwrap_or_default()).collect();
    let new_covered: Vec<_> = new.iter().map(|o| covered_now(o).unwrap_or_default()).collect();
    // One level at a time: each names every override there is.
    let levels = (0..Identity::LEVELS).map(|level| (names(&old_covered, level), names(&new_covered, level)));
    new_overrides(old, new, levels)
}

/// What each override covers, as told at a level of `Identity`.
fn names(covered: &[Vec<(String, Option<Identity>)>], level: usize) -> Vec<Vec<String>> {
    let name = |(name, identity): &(String, Option<Identity>)| match identity {
        Some(identity) => Some(format!("{name}\0{:?}", identity.key(level)?)),
        None => Some(name.clone()),
    };
    covered
        .iter()
        .map(|targets| targets.iter().filter_map(name).collect())
        .collect()
}

fn line_up<'f>(
    files: impl Iterator<Item = &'f str>,
    (base, head): (&Path, &Path),
    alignments: &mut HashMap<String, Alignment>,
) {
    for file in files {
        alignments
            .entry(file.to_string())
            .or_insert_with(|| Alignment::new(&base.join(file), &head.join(file)));
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
    let (mut previous, mut current) = (previous?, current?);
    let mut alignments: HashMap<String, Alignment> = HashMap::new();
    let roots = match (base, head) {
        (Source::Files { root: base, .. }, Source::Files { root: head, .. }) => Some((base.as_path(), head.as_path())),
        _ => None,
    };
    if let Some(roots) = roots {
        namesakes::reconcile(&mut previous, &current, roots, &mut alignments);
    }
    let overrides = |source: &Source| match source {
        Source::Files { root, files } => find_overrides_in(root, files).map_err(|e| e.to_string()),
        Source::Json(_) => Ok(Vec::new()),
    };
    let added = match roots {
        Some(roots) => added_overrides(
            (&overrides(base)?, &overrides(head)?),
            (&previous, &current),
            roots,
            &mut alignments,
        ),
        None => Vec::new(),
    };
    let strict = options.require_override_approval;
    let (matched, unmatched) = match_overrides(&added, &current);
    let approvals = if strict { Vec::new() } else { matched };
    // Which findings a change kept matters only where a lint override approves one.
    let lint = || approvals.iter().filter(|a| a.override_.metric == Metric::Lint);
    let approving: HashSet<String> = lint()
        .map(|a| function_key(&a.override_.file_path, &a.function_name))
        .collect();
    if let Some(roots) = roots {
        line_up(lint().map(|a| a.override_.file_path.as_str()), roots, &mut alignments);
        align(&mut previous, &mut current, &approving, &alignments);
    }
    let approved = approved_keys(&approvals, &current);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Base and head trees holding one Python file each, compared as the CLI compares them.
    fn compare(base: &str, head: &str) -> CompareResult {
        let temp = tempfile::tempdir().expect("temp dir");
        let dir = temp.path();
        for (side, source) in [("base", base), ("head", head)] {
            std::fs::create_dir_all(dir.join(side)).expect("temp dir");
            std::fs::write(dir.join(side).join("a.py"), source).expect("write");
        }
        let tools = Tools {
            oxlint: Err("not needed for Python".into()),
            node: None,
        };
        let result = compare_paths(&dir.join("base"), &dir.join("head"), &GateOptions::default(), &tools);
        result.expect("compare")
    }

    #[test]
    fn a_copied_directive_approves_the_finding_it_was_put_above() {
        let directive = "    # autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok\n";
        let base = format!("from typing import cast\ndef f(v):\n{directive}    a = cast(int, v)\n    return a\n");
        // The old line also gains code in front: what the directive covers is still the old cast.
        let head = format!(
            "from typing import cast\ndef f(v):\n{directive}    b = cast(str, v)\n{directive}    audit(); a = cast(int, v)\n    return a, b\n"
        );
        let result = compare(&base, &head);
        assert_eq!(
            result.approvals.iter().map(|a| a.override_.line).collect::<Vec<_>>(),
            [3]
        );
        assert!(result.regressions.is_empty());
    }

    #[test]
    fn a_finding_that_moved_is_told_by_its_code() {
        let directive = "    # autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok\n";
        let (old, new) = ("    a = cast(int, v)\n", "    b = cast(str, v)\n");
        let rest = "    c = 1\n    d = 2\n    e = 3\n";
        let file = |body: &[&str]| format!("from typing import cast\ndef f(v):\n{}", body.concat());
        let lint = |base: &[&str], head: &[&str]| compare(&file(base), &file(head)).regressions.len();
        // The old cast moved past unchanged code; only the new one is approved, or only the old.
        assert_eq!(lint(&[old, rest], &[rest, old, directive, new]), 0);
        assert_eq!(lint(&[old, rest], &[rest, directive, old, new]), 1);
        // Of two alike, either may be the old one: approving one leaves no slot for the other.
        assert_eq!(lint(&[old, rest], &[rest, directive, old, old]), 1);
        // An approved cast that moved, with its directive copied above a new one.
        assert_eq!(
            lint(&[directive, old, rest], &[directive, new, rest, directive, old]),
            0
        );
        assert_eq!(lint(&[directive, old, rest], &[new, rest, directive, old]), 1);
    }

    #[test]
    fn the_same_code_in_another_call_is_another_finding() {
        let directive = "    # autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok\n";
        let (old, new) = ("    log(cast(int, v), 1)\n", "    warn(cast(int, v), 1)\n");
        let edited = "    log(cast(int, v), 2)\n";
        let rest = "    c = 1\n    d = 2\n    e = 3\n";
        let file = |body: &[&str]| format!("from typing import cast\ndef f(v):\n{}", body.concat());
        let lint = |base: &[&str], head: &[&str]| compare(&file(base), &file(head)).regressions.len();
        // The new call took the old one's place, which moved: only the new one may be approved.
        assert_eq!(lint(&[old, rest], &[new, rest, directive, old]), 1);
        assert_eq!(lint(&[old, rest], &[directive, new, rest, old]), 0);
        // The same once the old call's other argument changed.
        assert_eq!(lint(&[old, rest], &[new, rest, directive, edited]), 1);
        assert_eq!(lint(&[old, rest], &[directive, new, rest, edited]), 0);
        // Neither call is the old one's, but one is in its place: that one was renamed.
        let renamed = "    info(cast(int, v), 1)\n";
        assert_eq!(lint(&[old, rest], &[new, rest, directive, renamed]), 0);
        // Edited in place, it is still the old one, beside a new one that is approved.
        let edited_cast = "    log(cast(str, v), 1)\n";
        let other = "    warn(cast(bytes, v), 1)\n";
        assert_eq!(lint(&[old, rest], &[edited_cast, rest, directive, other]), 0);
        // But where the approved one is the old code, it may be the old one, moved.
        assert_eq!(lint(&[old, rest], &[edited_cast, rest, directive, new]), 1);
    }

    #[test]
    fn the_same_code_in_another_branch_is_another_finding() {
        let directive =
            "        # autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok\n";
        let call = "        log(cast(int, v))\n";
        let branch =
            |header: &str, approved: bool| format!("    {header}:\n{}{call}", if approved { directive } else { "" });
        let rest = "    c = 1\n    d = 2\n    e = 3\n";
        let file = |body: &[&str]| format!("from typing import cast\ndef f(v):\n{}", body.concat());
        for (old, new) in [
            ("if a", "if b"),
            ("while a", "while b"),
            ("for a in v", "for b in v"),
            ("with a", "with b"),
        ] {
            let base = file(&[&branch(old, false), rest]);
            // The new branch took the old one's place, which moved: only the new may be approved.
            let hidden = file(&[&branch(new, false), rest, &branch(old, true)]);
            assert_eq!(compare(&base, &hidden).regressions.len(), 1, "{old}");
            let honest = file(&[&branch(new, true), rest, &branch(old, false)]);
            assert_eq!(compare(&base, &honest).regressions.len(), 0, "{old}");
        }
    }

    #[test]
    fn the_same_code_beside_other_code_is_another_finding() {
        let directive = |indent: &str| {
            format!("{indent}# autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok\n")
        };
        // Told apart by where `x` was set, or by a block's other code, also once that was edited.
        let set = |from: &str, approved: bool, extra: &str| {
            let approval = if approved { directive("    ") } else { String::new() };
            format!("    x = value.{from}\n{extra}{approval}    log(cast(int, x))\n")
        };
        let block = |from: &str, approved: bool, extra: &str| {
            let approval = if approved { directive("        ") } else { String::new() };
            format!("    if z:\n        note(value.{from})\n{approval}        log(cast(int, v))\n    {extra}")
        };
        let rest = "    c = 1\n    d = 2\n    e = 3\n";
        let file = |body: &[&str]| format!("from typing import cast\ndef f(v, z, value):\n{}", body.concat());
        type Unit<'a> = &'a dyn Fn(&str, bool, &str) -> String;
        let units: [Unit; 2] = [&set, &block];
        for unit in units {
            for extra in ["", "    more()\n"] {
                let base = file(&[&unit("first", false, ""), rest]);
                let hidden = file(&[&unit("second", false, ""), rest, &unit("first", true, extra)]);
                assert_eq!(compare(&base, &hidden).regressions.len(), 1, "{hidden}");
                let honest = file(&[&unit("second", true, ""), rest, &unit("first", false, extra)]);
                assert_eq!(compare(&base, &honest).regressions.len(), 0, "{honest}");
            }
        }
    }

    #[test]
    fn functions_that_repeat_a_name_are_told_by_their_code() {
        let def = |condition: &str, body: &str| format!("if {condition}:\n    def g(v, w):\n        {body}\n");
        let file = |defs: &[String]| format!("from typing import cast\n{}", defs.concat());
        let (old, clean) = ("return cast(int, v)", "return int(v)");
        let base = file(&[def("a", old)]);
        // A clean one put first takes the name `g`, not the old one's finding.
        assert_eq!(
            compare(&base, &file(&[def("b", clean), def("a", old)]))
                .regressions
                .len(),
            0
        );
        // Nor does one with a finding take its slot, once it is cleaned up.
        assert_eq!(
            compare(&base, &file(&[def("b", old), def("a", clean)]))
                .regressions
                .len(),
            1
        );
        // An edited one is compared with itself.
        let worse = compare(
            &base,
            &file(&[def("b", clean), def("a", "return cast(int, v), cast(str, w)")]),
        );
        let [worse] = &worse.regressions[..] else {
            panic!("{:?}", worse.regressions)
        };
        assert_eq!(
            (worse.function_name.as_str(), worse.kind, worse.previous, worse.current),
            ("g#2", crate::gate::Kind::Worsened, Some(1), 2)
        );
    }

    #[test]
    fn an_approval_names_the_finding_it_sits_on_across_revisions() {
        let base = "from typing import cast\ndef f(v):\n    a = cast(int, v)\n    return a\n";
        let directive =
            "    # autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- legacy\n";
        let lint = |r: &CompareResult| r.regressions.iter().filter(|r| r.metric == Metric::Lint).count();
        // Approving the old cast frees no slot for the new one beside it.
        let old = format!(
            "from typing import cast\ndef f(v):\n{directive}    a = cast(int, v)\n    b = cast(str, v)\n    return a, b\n"
        );
        assert_eq!(lint(&compare(base, &old)), 1);
        // Approving the new cast leaves the old one where it was.
        let new = format!(
            "from typing import cast\ndef f(v):\n    a = cast(int, v)\n{directive}    b = cast(str, v)\n    return a, b\n"
        );
        assert_eq!(lint(&compare(base, &new)), 0);
        // A copy of the old line, added later, is the new one: the old line is where it was.
        let copy = format!(
            "from typing import cast\ndef f(v):\n{directive}    a = cast(int, v)\n    v = 1\n    a = cast(int, v)\n    return a\n"
        );
        assert_eq!(lint(&compare(base, &copy)), 1);
        // An explanatory comment added to the old line keeps it the old line.
        let explained = format!(
            "from typing import cast\ndef f(v):\n{directive}    a = cast(int, v)  # checked by the schema\n    b = cast(str, v)\n    return a, b\n"
        );
        assert_eq!(lint(&compare(base, &explained)), 1);
        // Spaces inside a string are code: "a b" is not "a  b".
        let spaced = "from typing import cast\ndef f(v):\n    a = cast(int, \"a  b\")\n    return a\n";
        let other = format!(
            "from typing import cast\ndef f(v):\n    b = cast(int, \"a b\")\n{directive}    a = cast(int, \"a  b\")\n    return a, b\n"
        );
        assert_eq!(lint(&compare(spaced, &other)), 1);
        // The same when the new cast comes first and moves the old one down.
        let first = format!(
            "from typing import cast\ndef f(v):\n    b = cast(str, v)\n{directive}    a = cast(int, v)\n    return a, b\n"
        );
        assert_eq!(lint(&compare(base, &first)), 1);
    }
}
