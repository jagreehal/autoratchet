//! Measure source files: cyclomatic complexity from Oxlint; names and cognitive complexity from
//! the parser. Oxlint runs on a temporary copy with every disable comment defused, so no override
//! can hide a function from measurement.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::thread;

use regex::Regex;
use serde::Deserialize;

use crate::functions::{Analysis, analyse};
use crate::gate::FunctionSnapshot;
use crate::snapshots::{Snapshots, normalize_path};
use crate::tools::Tools;

const OXLINT_CONFIG: &str = include_str!("../config/oxlint-cyclomatic.json");

static COMPLEXITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<label>.+?) has a complexity of (?P<score>\d+)\. Maximum allowed is \d+\.?$").unwrap()
});
static LINTER_DIRECTIVE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(oxlint|eslint)-disable").unwrap());

/// Turn every disable directive into an ordinary comment of the same length, so offsets stay put.
/// Overrides decide what the gate enforces, never what it measures.
pub fn defuse_directives(source: &str) -> String {
    LINTER_DIRECTIVE_RE.replace_all(source, "${1}_disable").into_owned()
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
        for f in analysis.functions {
            snapshots.insert(FunctionSnapshot {
                file_path: file_path.clone(),
                function_name: f.name,
                cyclomatic: f.cyclomatic,
                line: Some(f.line),
                // Recorded as for JavaScript: 0 and 1 are both 1.
                cognitive: Some(f.cognitive.max(1)),
            });
        }
    }
    Ok(snapshots)
}

/// JavaScript and TypeScript: Oxlint for cyclomatic complexity, the parser for the rest.
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
    let copies: Vec<PathBuf> = files.iter().map(|f| copy.path().join(relative(root, f))).collect();

    // Copy and parse in parallel: every file is parsed once, for names and parse errors.
    let analyses = parallel(files.iter().zip(&copies).collect(), |(file, dest)| {
        let source = defuse_directives(&String::from_utf8_lossy(
            &std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?,
        ));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(dest, &source).map_err(|e| e.to_string())?;
        Ok(analyse(dest, &source))
    })?;
    let parsed: HashMap<&Path, &Analysis> = copies.iter().map(PathBuf::as_path).zip(&analyses).collect();

    let target = copy.path();
    let mut snapshots = snapshots_from_diagnostics(run_oxlint(tools, oxlint, target, config.path())?, target, &parsed);
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
    line: Option<u32>,
}

fn run_oxlint(tools: &Tools, oxlint: &Path, target: &Path, config_dir: &Path) -> Result<Vec<OxlintDiagnostic>, String> {
    let config = config_dir.join("oxlint.json");
    std::fs::write(&config, OXLINT_CONFIG).map_err(|e| e.to_string())?;
    let output = tools
        .command(oxlint)
        .arg("-c")
        .arg(&config)
        .args(["-f", "json"])
        .arg(target)
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
    Ok(parsed
        .diagnostics
        .into_iter()
        .map(|mut d| {
            d.filename = d.filename.map(|f| config_dir.join(f).to_string_lossy().into_owned());
            d
        })
        .collect())
}

fn snapshots_from_diagnostics(
    diagnostics: Vec<OxlintDiagnostic>,
    root: &Path,
    parsed: &HashMap<&Path, &Analysis>,
) -> Snapshots {
    let mut snapshots = Snapshots::default();
    let mut seen = HashSet::new();

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
            cognitive: Some(u32::from(cognitive)),
        });
    }
    snapshots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defuse_keeps_length() {
        let src = "// oxlint-disable-next-line complexity\n// eslint-disable complexity\n";
        let out = defuse_directives(src);
        assert_eq!(out.len(), src.len());
        assert!(out.contains("oxlint_disable") && out.contains("eslint_disable"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c/d.ts")), "c/d.ts");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/x.ts")), "../x.ts");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b")), "");
    }
}
