//! Text, Markdown and JSON reports.

use serde::Serializer;
use serde_json::json;

use crate::compare::{CompareResult, Verdict, locale_order};
use crate::gate::{Kind, LintFinding, Metric, Regression};
use crate::overrides::Override;

/// Serialize a number the way JavaScript prints it: `10`, not `10.0`.
pub fn js_number<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        s.serialize_i64(*value as i64)
    } else {
        s.serialize_f64(*value)
    }
}

fn num(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

fn opt(value: Option<u32>) -> String {
    value.map_or("?".into(), |v| v.to_string())
}

fn block<T>(title: &str, items: &[T], line: impl Fn(&T) -> Vec<String>) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    // One comment that switches off both metrics is two overrides: list it once.
    let mut lines: Vec<String> = std::iter::once(title.to_string())
        .chain(items.iter().flat_map(line))
        .collect();
    lines.dedup();
    lines
}

fn override_line(o: &Override) -> Vec<String> {
    vec![format!("  {}:{}  {}", o.file_path, o.line, o.text)]
}

pub fn format_text(r: &CompareResult) -> String {
    let failed = r.verdict() != Verdict::Pass;
    let failures: Vec<Vec<String>> = [
        if r.regressions.is_empty() {
            Vec::new()
        } else {
            vec![format_regressions(&r.regressions)]
        },
        block(
            "New overrides need approval (--require-override-approval):",
            &r.blocked_overrides,
            override_line,
        ),
        block(
            "Measurement incomplete. The gate fails until every function can be measured:",
            &r.incomplete,
            |p| vec![format!("  {p}")],
        ),
    ]
    .into_iter()
    .filter(|b| !b.is_empty())
    .collect();

    let mut approvals = block(
        "Approved by a new override. A reviewer must accept each reason:",
        &r.approvals,
        |a| {
            vec![
                format!(
                    "  {}::{} {} {}",
                    a.override_.file_path,
                    a.function_name,
                    a.override_.metric.as_str(),
                    opt(a.score)
                ),
                format!("    line {}: {}", a.override_.line, a.override_.text),
            ]
        },
    );
    if !r.approvals.is_empty() {
        approvals.push("Still measured: if one of these gets worse in a later PR, the gate fails.".into());
    }
    let notes: Vec<Vec<String>> = [
        approvals,
        block(
            "Warning: new overrides that sit on no single function (file-wide or misplaced). The gate ignores them:",
            &r.unmatched_overrides,
            override_line,
        ),
        block(
            "Warning: over a threshold on base, missing on head (deleted, renamed or moved):",
            &r.untracked,
            |s| {
                let cognitive = s.cognitive.map_or(String::new(), |c| format!(", cognitive {c}"));
                vec![format!(
                    "  {}::{} (was cyclomatic {}{cognitive})",
                    s.file_path, s.function_name, s.cyclomatic
                )]
            },
        ),
    ]
    .into_iter()
    .filter(|b| !b.is_empty())
    .collect();

    let verdict = if failed {
        "Quality gate failed."
    } else {
        "No regressions detected.\nQuality gate passed."
    };
    let mut parts: Vec<String> = failures.iter().map(|b| format!("{}\n", b.join("\n"))).collect();
    parts.push(verdict.to_string());
    parts.extend(notes.iter().map(|b| format!("\n{}", b.join("\n"))));
    parts.join("\n")
}

/// A file's regressions, grouped by function.
type FileGroup<'a> = (&'a str, Vec<(&'a str, Vec<&'a Regression>)>);

/// Names what failed, so a lint-only failure does not read as a complexity one.
fn heading(regressions: &[Regression]) -> &'static str {
    let lint = regressions.iter().filter(|r| r.metric == Metric::Lint).count();
    match (lint, regressions.len() - lint) {
        (0, _) => "Complexity regressions detected:",
        (_, 0) => "Lint regressions detected:",
        _ => "Complexity and lint regressions detected:",
    }
}

fn format_regressions(regressions: &[Regression]) -> String {
    // file → function → that function's failing metrics, in first-seen order
    let mut sorted: Vec<&Regression> = regressions.iter().collect();
    sorted.sort_by(|a, b| locale_order(&a.file_path, &b.file_path));
    let mut files: Vec<FileGroup> = Vec::new();
    for reg in sorted {
        if files.last().is_none_or(|(f, _)| *f != reg.file_path) {
            files.push((&reg.file_path, Vec::new()));
        }
        let fns = &mut files.last_mut().expect("just pushed").1;
        match fns.iter_mut().find(|(name, _)| *name == reg.function_name) {
            Some((_, regs)) => regs.push(reg),
            None => fns.push((&reg.function_name, vec![reg])),
        }
    }
    let mut lines = vec![heading(regressions).to_string(), String::new()];
    for (file, fns) in files {
        lines.push(file.to_string());
        for (name, regs) in fns {
            let new = if regs[0].kind == Kind::New {
                " (new function)"
            } else {
                ""
            };
            lines.push(format!("  {name}{new}"));
            for reg in regs {
                lines.push(format!("    {}", format_metric(reg)));
                lines.extend(reg.findings.iter().map(|f| format!("      {}", finding(f))));
            }
        }
        lines.push(String::new());
    }
    lines.join("\n").trim_end().to_string()
}

fn format_metric(reg: &Regression) -> String {
    match reg.kind {
        Kind::New => format!(
            "{}: {} (maximum for new code: {}{})",
            reg.metric.as_str(),
            reg.current,
            num(reg.threshold),
            source_suffix(reg)
        ),
        Kind::Worsened => {
            let delta = i64::from(reg.current) - i64::from(reg.previous.unwrap_or(0));
            let limit = match source_suffix(reg) {
                from if !from.is_empty() => format!(" (maximum for new code: {}{from})", num(reg.threshold)),
                _ => String::new(),
            };
            format!(
                "{}: {} → {} (+{delta}){limit}",
                reg.metric.as_str(),
                opt(reg.previous),
                reg.current
            )
        }
    }
}

/// `line 3 anti-slop(no-reflect-get): Replace …`
fn finding(f: &LintFinding) -> String {
    format!("line {} {}: {}", opt(f.line), f.rule, f.message)
}

/// `, from ruff.toml` when a linter config set the threshold.
fn source_suffix(reg: &Regression) -> String {
    reg.threshold_source
        .as_ref()
        .map_or(String::new(), |source| format!(", from {source}"))
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "pass",
        Verdict::Fail => "fail",
        Verdict::Incomplete => "incomplete",
    }
}

/// Machine-readable result for dashboards, CI annotations and coding agents.
pub fn format_json(r: &CompareResult) -> String {
    let mut value = json!({ "verdict": verdict_name(r.verdict()) });
    let obj = value.as_object_mut().expect("object");
    if let Some(commit) = &r.base_commit {
        obj.insert("baseCommit".into(), json!(commit));
    }
    obj.insert("regressions".into(), json!(r.regressions));
    obj.insert("approvals".into(), json!(r.approvals));
    obj.insert("blockedOverrides".into(), json!(r.blocked_overrides));
    obj.insert("unmatchedOverrides".into(), json!(r.unmatched_overrides));
    obj.insert("incomplete".into(), json!(r.incomplete));
    obj.insert("untracked".into(), json!(r.untracked));
    obj.insert("functions".into(), json!(r.current.len()));
    // The same result as a PR comment, so CI needs one run for both.
    obj.insert("markdown".into(), json!(format_markdown(r)));
    serde_json::to_string_pretty(&value).expect("serializable")
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn section<T>(title: &str, items: &[T], bullet: impl Fn(&T) -> String) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = vec![title.to_string(), String::new()];
    out.extend(items.iter().map(|i| format!("- {}", bullet(i))));
    out.dedup();
    out.push(String::new());
    out
}

fn at(o: &Override) -> String {
    format!("`{}:{}` {}", o.file_path, o.line, cell(&o.text))
}

/// A pull request comment: verdict first, then one table or list per kind of finding.
pub fn format_markdown(r: &CompareResult) -> String {
    let verdict = match r.verdict() {
        Verdict::Pass => "**Passed.** No function got worse than the ratchet allows.",
        Verdict::Fail => "**Failed.** A function got worse than the ratchet allows.",
        Verdict::Incomplete => {
            "**Failed.** AutoRatchet could not measure every function, so it cannot vouch for this change."
        }
    };
    let mut lines = vec![
        "## AutoRatchet".to_string(),
        String::new(),
        verdict.to_string(),
        String::new(),
    ];
    if !r.regressions.is_empty() {
        lines.push("| Function | Metric | Before | After | Limit |".into());
        lines.push("|---|---|--:|--:|--:|".into());
        for reg in &r.regressions {
            let limit = match reg.kind {
                Kind::New => reg.threshold,
                Kind::Worsened => f64::from(reg.previous.unwrap_or(0)).max(reg.threshold),
            };
            lines.push(format!(
                "| `{}::{}` | {} | {} | {} | {}{} |",
                cell(&reg.file_path),
                cell(&reg.function_name),
                reg.metric.as_str(),
                reg.previous.map_or("new".into(), |p| p.to_string()),
                reg.current,
                num(limit),
                match &reg.threshold_source {
                    None => String::new(),
                    // Over the config limit already: the limit is its old score, so show both.
                    Some(source) if limit > reg.threshold =>
                        format!(" (new code: {} from {source})", num(reg.threshold)),
                    Some(source) => format!(" from {source}"),
                }
            ));
        }
        lines.push(String::new());
    }
    let findings: Vec<(&Regression, &LintFinding)> = r
        .regressions
        .iter()
        .flat_map(|reg| reg.findings.iter().map(move |f| (reg, f)))
        .collect();
    lines.extend(section(
        "**Lint findings in those functions:**",
        &findings,
        |(reg, f)| {
            format!(
                "`{}:{}` {}: {}",
                cell(&reg.file_path),
                opt(f.line),
                f.rule,
                cell(&f.message)
            )
        },
    ));
    lines.extend(section("**New overrides need approval:**", &r.blocked_overrides, at));
    lines.extend(section("**Could not measure:**", &r.incomplete, |p| p.clone()));
    lines.extend(section(
        "**Approved by a new override** (a reviewer must accept each reason; still measured):",
        &r.approvals,
        |a| {
            format!(
                "`{}::{}` {} {}: {}",
                a.override_.file_path,
                a.function_name,
                a.override_.metric.as_str(),
                opt(a.score),
                cell(&a.override_.text)
            )
        },
    ));
    lines.extend(section(
        "**Overrides on no single function** (ignored by the gate):",
        &r.unmatched_overrides,
        at,
    ));
    lines.extend(section(
        "**Over a limit on base, missing on head** (deleted, renamed or moved):",
        &r.untracked,
        |u| format!("`{}::{}`", u.file_path, u.function_name),
    ));
    let base = r
        .base_commit
        .as_ref()
        .map_or(String::new(), |c| format!(" against `{}`", &c[..c.len().min(7)]));
    lines.push(format!(
        "<sub>{} functions measured{base}. Limits for new code: cyclomatic 10, cognitive 15 unless configured, no lint findings.</sub>",
        r.current.len()
    ));
    lines.join("\n")
}
