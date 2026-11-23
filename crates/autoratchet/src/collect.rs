//! Measure source files: cyclomatic complexity and lint findings from Oxlint; names and cognitive
//! complexity from the parser. Oxlint runs on a temporary copy with every disable comment defused,
//! so no override can hide a function from measurement.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::thread;

use regex::Regex;
use serde::Deserialize;

use crate::functions::{Analysis, analyse};
use crate::gate::{FunctionSnapshot, LintFinding, TOP_LEVEL};
use crate::snapshots::{Snapshots, function_key, normalize_path};
use crate::tools::Tools;

/// Every function gets a complexity diagnostic (`max: 0`); the lint rules are the anti-slop
/// plugin plus the native rules below.
fn oxlint_config() -> String {
    let mut rules = serde_json::Map::new();
    rules.insert(
        "complexity".into(),
        serde_json::json!(["error", { "max": 0, "variant": "modified" }]),
    );
    for rule in NATIVE_RULES {
        rules.insert(rule.into(), "error".into());
    }
    for rule in crate::plugin::RULES {
        rules.insert(format!("anti-slop/{rule}"), "error".into());
    }
    serde_json::json!({
        "plugins": ["typescript", "unicorn", "oxc", "import", "promise"],
        "categories": { "correctness": "off" },
        "jsPlugins": [{ "name": "anti-slop", "specifier": "./anti-slop/index.js" }],
        "rules": rules,
    })
    .to_string()
}

/// Oxlint's own rules that need no type information.
pub const NATIVE_RULES: [&str; 87] = [
    "array-callback-return",
    "constructor-super",
    "eqeqeq",
    "for-direction",
    "getter-return",
    "no-async-promise-executor",
    "no-case-declarations",
    "no-class-assign",
    "no-compare-neg-zero",
    "no-cond-assign",
    "no-const-assign",
    "no-constant-binary-expression",
    "no-constant-condition",
    "no-control-regex",
    "no-debugger",
    "no-dupe-else-if",
    "no-dupe-keys",
    "no-duplicate-case",
    "no-empty-character-class",
    "no-empty-pattern",
    "no-eval",
    "no-ex-assign",
    "no-fallthrough",
    "no-func-assign",
    "no-global-assign",
    "no-implied-eval",
    "no-import-assign",
    "no-invalid-regexp",
    "no-loss-of-precision",
    "no-misleading-character-class",
    "no-new-native-nonconstructor",
    "no-new-wrappers",
    "no-nonoctal-decimal-escape",
    "no-obj-calls",
    "no-self-assign",
    "no-self-compare",
    "no-setter-return",
    "no-shadow-restricted-names",
    "no-sparse-arrays",
    "no-this-before-super",
    "no-throw-literal",
    "no-unexpected-multiline",
    "no-unreachable",
    "no-unsafe-finally",
    "no-unsafe-negation",
    "no-unsafe-optional-chaining",
    "no-unused-labels",
    "no-unused-private-class-members",
    "no-useless-backreference",
    "no-useless-catch",
    "no-with",
    "prefer-promise-reject-errors",
    "preserve-caught-error",
    "require-yield",
    "use-isnan",
    "valid-typeof",
    "import/no-self-import",
    "oxc/approx-constant",
    "oxc/bad-bitwise-operator",
    "oxc/bad-char-at-comparison",
    "oxc/bad-comparison-sequence",
    "oxc/bad-min-max-func",
    "oxc/bad-object-literal-comparison",
    "oxc/bad-replace-all-arg",
    "oxc/const-comparisons",
    "oxc/double-comparisons",
    "oxc/misrefactored-assign-op",
    "oxc/missing-throw",
    "oxc/no-accumulating-spread",
    "oxc/number-arg-out-of-range",
    "oxc/uninvoked-array-callback",
    "promise/no-multiple-resolved",
    "promise/no-return-in-finally",
    "typescript/ban-ts-comment",
    "typescript/no-duplicate-enum-values",
    "typescript/no-explicit-any",
    "typescript/no-extra-non-null-assertion",
    "typescript/no-misused-new",
    "typescript/no-non-null-asserted-nullish-coalescing",
    "typescript/no-non-null-asserted-optional-chain",
    "typescript/no-non-null-assertion",
    "typescript/no-unsafe-declaration-merging",
    "typescript/no-unsafe-function-type",
    "typescript/no-wrapper-object-types",
    "unicorn/no-array-fill-with-reference-type",
    "unicorn/no-invalid-fetch-options",
    "unicorn/no-invalid-remove-event-listener",
];

/// Diagnostic codes of the ratcheted rules: `eslint(no-eval)`, `oxc(missing-throw)`, `anti-slop(…)`.
static LINT_CODES: LazyLock<HashSet<String>> = LazyLock::new(|| {
    NATIVE_RULES
        .iter()
        .map(|r| match r.split_once('/') {
            Some((plugin, rule)) => format!("{plugin}({rule})"),
            None => format!("eslint({r})"),
        })
        .chain(crate::plugin::RULES.iter().map(|r| format!("anti-slop({r})")))
        .collect()
});

fn is_lint_code(code: &str) -> bool {
    LINT_CODES.contains(code)
}

/// Whether a rule named in a disable comment is a ratcheted one, under any plugin prefix:
/// `no-eval`, `eslint/no-eval`, `@typescript-eslint/no-explicit-any`, `quality/no-eval`.
pub fn is_lint_rule(name: &str) -> bool {
    let rule = name.rsplit('/').next().unwrap_or(name);
    crate::plugin::RULES.contains(&rule)
        || NATIVE_RULES.iter().any(|r| r.rsplit('/').next() == Some(rule))
        || crate::slop::rules().any(|(_, r)| r == rule)
}

/// A file the anti-slop plugin must flag. The standalone Oxlint binary skips JS plugins, so this
/// proves the plugin ran.
const PROBE_FILE: &str = "autoratchet-probe.ts";
const PROBE_SOURCE: &str = "Reflect.get(globalThis, 'probe');\n";

static COMPLEXITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<label>.+?) has a complexity of (?P<score>\d+)\. Maximum allowed is \d+\.?$").unwrap()
});
static LINTER_DIRECTIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u:\b)(oxlint|eslint)-disable").expect("valid regex"));

/// Turn every disable directive in a comment into an ordinary comment of the same length, so
/// offsets stay put. Overrides decide what the gate enforces, never what it measures; strings
/// such as `import './oxlint-disable'` are code and stay as they are.
pub fn defuse_directives(source: &str, comments: &[(u32, u32)]) -> String {
    let mut out = source.to_string();
    for &(start, end) in comments {
        let range = start as usize..end as usize;
        if let Some(text) = source.get(range.clone()) {
            out.replace_range(range, &LINTER_DIRECTIVE_RE.replace_all(text, "${1}_disable"));
        }
    }
    out
}

/// `path.relative(root, file)`: the path from `root` to `file`, with `..` where needed.
pub fn relative(root: &Path, file: &Path) -> String {
    let root: Vec<_> = root.components().filter(|c| !matches!(c, Component::CurDir)).collect();
    let file: Vec<_> = file.components().filter(|c| !matches!(c, Component::CurDir)).collect();
    let common = root.iter().zip(&file).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..root.len() {
        out.push("..");
    }
    for c in &file[common..] {
        out.push(c);
    }
    out.to_string_lossy().into_owned()
}

fn relative_to(root: &Path, absolute: &Path) -> String {
    let rel = normalize_path(&relative(root, absolute));
    if rel.is_empty() {
        absolute
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        rel
    }
}

/// Measure every source file under a directory.
pub fn collect_from_directory(dir: &Path, tools: &Tools) -> Result<Snapshots, String> {
    let dir = std::path::absolute(dir).map_err(|e| e.to_string())?;
    let files = crate::overrides::source_files(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    collect_files(&dir, &files, tools)
}

/// Measure the given source files. Paths in the result are relative to `root`.
pub fn collect_files(root: &Path, files: &[PathBuf], tools: &Tools) -> Result<Snapshots, String> {
    let (native, js): (Vec<PathBuf>, Vec<PathBuf>) = files
        .iter()
        .cloned()
        .partition(|f| crate::overrides::is_native(&f.to_string_lossy()));
    let mut snapshots = collect_native(root, &native)?;
    let js = collect_js(root, &js, tools)?;
    js.values().for_each(|s| snapshots.insert(s.clone()));
    snapshots.problems.extend(js.problems);
    Ok(snapshots)
}

/// Rust, Python and Go files: measured in-process, one parse each.
fn collect_native(root: &Path, files: &[PathBuf]) -> Result<Snapshots, String> {
    let mut analyses = parallel(files.iter().collect(), |file| {
        let source =
            String::from_utf8_lossy(&std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?).into_owned();
        let file_path = normalize_path(&relative(root, file));
        let analysis = crate::overrides::analyse_native(&file_path, &source);
        Ok((file_path, analysis))
    })?;
    crate::lang::add_cross_file_recursion(&mut analyses);
    let mut snapshots = Snapshots::default();
    for (file_path, analysis) in analyses {
        if let Some(error) = analysis.error {
            snapshots
                .problems
                .push(format!("{file_path}: does not parse ({error})"));
        }
        for f in &analysis.functions {
            snapshots.insert(FunctionSnapshot {
                file_path: file_path.clone(),
                function_name: f.name.clone(),
                cyclomatic: f.cyclomatic,
                line: Some(f.line),
                offset: None,
                origin: Some(f.origin.clone()),
                span: Some(f.span.clone()),
                // Recorded as for JavaScript: 0 and 1 are both 1.
                cognitive: Some(f.cognitive.max(1)),
                lint: Some(Vec::new()),
            });
        }
        for finding in analysis.findings {
            // The innermost function: the narrowest span around the finding.
            let name = analysis
                .functions
                .iter()
                .filter(|f| f.span.contains(&finding.offset))
                .min_by_key(|f| f.span.len())
                .map(|f| f.name.clone());
            let lint = LintFinding {
                rule: finding.rule,
                line: Some(finding.line),
                message: finding.message.into(),
                identity: None,
                span: Some(finding.offset..finding.end),
            };
            push_finding(&mut snapshots, &file_path, name, lint);
        }
    }
    Ok(snapshots)
}

/// JavaScript and TypeScript: Oxlint for cyclomatic complexity and lint, the parser for the rest.
fn collect_js(root: &Path, files: &[PathBuf], tools: &Tools) -> Result<Snapshots, String> {
    // Nothing to measure (a base with no copy of a brand-new file): an empty, complete result.
    if files.is_empty() {
        return Ok(Snapshots::default());
    }
    let oxlint = tools.oxlint.as_ref().map_err(Clone::clone)?;
    let copy = tempfile::Builder::new()
        .prefix("autoratchet-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let config = tempfile::Builder::new()
        .prefix("autoratchet-config-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    // Real paths: on macOS the temp dir sits behind the /var symlink, and Oxlint's import
    // resolver then sees a file and its own import as two paths (import/no-self-import misses).
    let target = std::fs::canonicalize(copy.path()).map_err(|e| e.to_string())?;
    let config_dir = std::fs::canonicalize(config.path()).map_err(|e| e.to_string())?;
    let copies: Vec<PathBuf> = files.iter().map(|f| target.join(relative(root, f))).collect();

    // Copy and parse in parallel: every file is parsed once, for names and parse errors.
    let analyses = parallel(files.iter().zip(&copies).collect(), |(file, dest)| {
        let raw =
            String::from_utf8_lossy(&std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?).into_owned();
        // Defusing keeps every length, so the analysis of the original holds for the copy.
        let analysis = analyse(dest, &raw);
        let source = defuse_directives(&raw, &analysis.comments);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(dest, &source).map_err(|e| e.to_string())?;
        Ok(analysis)
    })?;
    let parsed: HashMap<&Path, &Analysis> = copies.iter().map(PathBuf::as_path).zip(&analyses).collect();

    let target = target.as_path();
    let mut snapshots = snapshots_from_diagnostics(run_oxlint(tools, oxlint, target, &config_dir)?, target, &parsed);
    for file in &copies {
        if let Some(error) = parsed[file.as_path()].errors.first() {
            let rel = normalize_path(&relative(target, file));
            snapshots.problems.push(format!("{rel}: does not parse ({error})"));
        }
    }
    Ok(snapshots)
}

/// Run `work` over `items` on every core, keeping order. The first error wins.
fn parallel<T: Sync, R: Send>(items: Vec<T>, work: impl Fn(&T) -> Result<R, String> + Sync) -> Result<Vec<R>, String> {
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = items.len().div_ceil(threads).max(1);
    thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                // Walking a deeply nested syntax tree recurses: give each worker room for it.
                thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(s, || part.iter().map(&work).collect::<Result<Vec<_>, _>>())
                    .expect("spawn worker thread")
            })
            .collect();
        let mut out = Vec::with_capacity(items.len());
        for h in handles {
            out.extend(h.join().expect("worker thread")?);
        }
        Ok(out)
    })
}

#[derive(Deserialize)]
struct OxlintOutput {
    #[serde(default)]
    diagnostics: Vec<OxlintDiagnostic>,
}

#[derive(Deserialize)]
pub struct OxlintDiagnostic {
    message: String,
    code: Option<String>,
    filename: Option<String>,
    #[serde(default)]
    labels: Vec<OxlintLabel>,
}

#[derive(Deserialize)]
struct OxlintLabel {
    span: Option<OxlintSpan>,
}

#[derive(Deserialize)]
struct OxlintSpan {
    offset: Option<i64>,
    length: Option<u64>,
    line: Option<u32>,
}

fn run_oxlint(tools: &Tools, oxlint: &Path, target: &Path, config_dir: &Path) -> Result<Vec<OxlintDiagnostic>, String> {
    let config = config_dir.join("oxlint.json");
    std::fs::write(&config, oxlint_config()).map_err(|e| e.to_string())?;
    for (file, text) in crate::plugin::FILES {
        let path = config_dir.join("anti-slop").join(file);
        std::fs::create_dir_all(path.parent().expect("plugin file in a directory")).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    let probe = config_dir.join(PROBE_FILE);
    std::fs::write(&probe, PROBE_SOURCE).map_err(|e| e.to_string())?;
    let output = tools
        .command(oxlint)
        .arg("-c")
        .arg(&config)
        .args(["-f", "json"])
        .arg(target)
        .arg(&probe)
        .current_dir(config_dir)
        .output()
        .map_err(|e| format!("could not run oxlint ({}): {e}", oxlint.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Err(format!(
            "oxlint printed nothing: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let parsed: OxlintOutput =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("Failed to parse oxlint JSON output: {e}"))?;
    let (probed, diagnostics): (Vec<_>, Vec<_>) = parsed
        .diagnostics
        .into_iter()
        .map(|mut d| {
            d.filename = d.filename.map(|f| config_dir.join(f).to_string_lossy().into_owned());
            d
        })
        .partition(|d| d.filename.as_deref().is_some_and(|f| Path::new(f) == probe));
    if !probed.iter().any(|d| d.code.as_deref().is_some_and(is_lint_code)) {
        return Err(format!(
            "oxlint ({}) did not run AutoRatchet's lint rules. They are an Oxlint JS plugin, which only \
             Oxlint's npm package running under Node.js loads; the standalone binary skips it. \
             Point AUTORATCHET_OXLINT at node_modules/oxlint/bin/oxlint, or run AutoRatchet through npx.{}",
            oxlint.display(),
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .map_or(String::new(), |l| format!(" Oxlint said: {l}"))
        ));
    }
    Ok(diagnostics)
}

fn snapshots_from_diagnostics(
    diagnostics: Vec<OxlintDiagnostic>,
    root: &Path,
    parsed: &HashMap<&Path, &Analysis>,
) -> Snapshots {
    let mut snapshots = Snapshots::default();
    let mut seen = HashSet::new();

    let (lint, diagnostics): (Vec<_>, Vec<_>) = diagnostics
        .into_iter()
        .partition(|d| d.code.as_deref().is_some_and(is_lint_code));
    for d in diagnostics {
        if !d.code.as_deref().is_some_and(|c| c.contains("complexity")) {
            continue;
        }
        let Some(caps) = COMPLEXITY_RE.captures(&d.message) else {
            continue;
        };
        let label = caps["label"].to_string();
        let score: u32 = caps["score"].parse().unwrap_or(0);
        let absolute = d.filename.as_deref().map_or_else(|| root.to_path_buf(), PathBuf::from);
        let span = d.labels.first().and_then(|l| l.span.as_ref());
        let offset = span.and_then(|s| s.offset).unwrap_or(-1);
        let line = span.and_then(|s| s.line);
        let file_path = relative_to(root, &absolute);
        let analysis = parsed.get(absolute.as_path());
        let name = (offset >= 0)
            .then(|| analysis.and_then(|a| a.functions.get(&(offset as u32))))
            .flatten();
        let Some(name) = name else {
            // Oxlint also scores class field initialisers that are not functions (`x = a ? 1 : 2`)
            // and bodiless declarations such as overload signatures.
            // A file that does not parse is reported once, by collect_files.
            let field_initialiser = label.starts_with("class field initializer");
            let bodiless = analysis.is_some_and(|a| a.bodiless.contains(&(offset as u32)));
            if !field_initialiser && !bodiless && analysis.is_none_or(|a| a.errors.is_empty()) {
                snapshots.problems.push(format!(
                    "{file_path}:{}: Oxlint scored {label} but no function was found there",
                    line.map_or("?".to_string(), |l| l.to_string())
                ));
            }
            continue;
        };
        // Oxlint reports a class field arrow twice (as the field and as the method): keep one.
        if !seen.insert((absolute.clone(), offset)) {
            continue;
        }
        // Biome never reported scores of 0 or 1, so AutoRatchet recorded both as 1. Keep that
        // floor, so scores recorded by earlier versions still compare.
        let cognitive = analysis
            .and_then(|a| a.cognitive.get(&(offset as u32)))
            .copied()
            .unwrap_or(0)
            .max(1);
        snapshots.insert(FunctionSnapshot {
            file_path,
            function_name: name.clone(),
            cyclomatic: score,
            line,
            offset: usize::try_from(offset).ok(),
            origin: analysis.and_then(|a| a.origins.get(name).cloned()),
            span: analysis
                .and_then(|a| a.spans.get(&(offset as u32)))
                .map(|&(from, end)| from as usize..end as usize),
            cognitive: Some(u32::from(cognitive)),
            lint: Some(Vec::new()),
        });
    }
    for d in lint {
        add_lint_finding(&mut snapshots, d, root, parsed);
    }
    snapshots
}

/// Count a finding against the innermost function around it, or against the file's top level.
fn add_lint_finding(snapshots: &mut Snapshots, d: OxlintDiagnostic, root: &Path, parsed: &HashMap<&Path, &Analysis>) {
    let absolute = d.filename.as_deref().map_or_else(|| root.to_path_buf(), PathBuf::from);
    let file_path = relative_to(root, &absolute);
    let span = d.labels.first().and_then(|l| l.span.as_ref());
    let offset = span.and_then(|s| s.offset).and_then(|o| u32::try_from(o).ok());
    let length = span.and_then(|s| s.length).map_or(0, |l| l as usize);
    let line = span.and_then(|s| s.line);
    let name = parsed.get(absolute.as_path()).and_then(|a| {
        let offset = offset?;
        // The innermost: the span that starts last.
        let start = a
            .spans
            .iter()
            .filter(|&(_, &(from, end))| from <= offset && offset < end)
            .max_by_key(|&(_, &(from, _))| from)
            .map(|(&start, _)| start)?;
        a.functions.get(&start).cloned()
    });
    let finding = LintFinding {
        rule: d.code.unwrap_or_default(),
        line,
        message: d.message,
        identity: None,
        span: offset.map(|o| o as usize..o as usize + length),
    };
    push_finding(snapshots, &file_path, name, finding);
}

/// Count a finding against its function, or the file's top level when it is in none we measure
/// (a bodiless JavaScript declaration counts at the top level too).
fn push_finding(snapshots: &mut Snapshots, file_path: &str, function: Option<String>, finding: LintFinding) {
    let line = finding.line;
    let key = function
        .map(|n| function_key(file_path, &n))
        .filter(|k| snapshots.get(k).is_some())
        .unwrap_or_else(|| {
            let top = function_key(file_path, TOP_LEVEL);
            if snapshots.get(&top).is_none() {
                snapshots.insert(FunctionSnapshot {
                    file_path: file_path.to_string(),
                    function_name: TOP_LEVEL.into(),
                    line,
                    lint: Some(Vec::new()),
                    ..Default::default()
                });
            }
            top
        });
    if let Some(list) = snapshots.get_mut(&key).and_then(|s| s.lint.as_mut()) {
        list.push(finding);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defuse_keeps_length() {
        let src = "// oxlint-disable-next-line complexity\n// eslint-disable complexity\nimport './oxlint-disable';\n";
        let out = defuse_directives(src, &analyse(Path::new("a.ts"), src).comments);
        assert_eq!(out.len(), src.len());
        assert!(out.contains("oxlint_disable") && out.contains("eslint_disable"));
        assert!(out.contains("'./oxlint-disable'"), "strings are code: {out}");
    }

    #[test]
    fn lint_rules_and_codes() {
        for name in [
            "eqeqeq",
            "eslint/no-eval",
            "@typescript-eslint/no-explicit-any",
            "quality/no-reflect-get",
        ] {
            assert!(is_lint_rule(name), "{name}");
        }
        for name in ["complexity", "sonarjs/cognitive-complexity", "no-console"] {
            assert!(!is_lint_rule(name), "{name}");
        }
        for code in [
            "eslint(no-eval)",
            "typescript(no-explicit-any)",
            "oxc(missing-throw)",
            "anti-slop(no-runtime-typeof)",
        ] {
            assert!(is_lint_code(code), "{code}");
        }
        assert!(!is_lint_code("eslint(complexity)"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c/d.ts")), "c/d.ts");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/x.ts")), "../x.ts");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b")), "");
    }
}
