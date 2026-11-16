//! `check`: compare the working tree with `merge-base(base, HEAD)`. Uncommitted changes count,
//! files git ignores do not, and only JavaScript and TypeScript sources are measured.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::compare::{CompareResult, Source, compare_sources};
use crate::gate::GateOptions;
use crate::overrides::is_source_file;
use crate::tools::Tools;

fn git(cwd: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn nul_list(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Write each `commit:path` blob under `dest`, through one `git cat-file --batch` process.
fn extract(root: &Path, commit: &str, files: &[String], dest: &Path) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["cat-file", "--batch"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run git cat-file: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let requests: String = files.iter().map(|f| format!("{commit}:{f}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(requests.as_bytes()));
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    for file in files {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|e| e.to_string())?;
        let size: usize = header
            .split_whitespace()
            .nth(2)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("git cat-file: unexpected reply for {file}: {}", header.trim()))?;
        let mut blob = vec![0; size + 1]; // contents plus the trailing newline
        reader.read_exact(&mut blob).map_err(|e| e.to_string())?;
        blob.pop();
        let path = dest.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, blob).map_err(|e| e.to_string())?;
    }
    writer.join().expect("writer thread").map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

pub struct CheckOptions {
    /// Branch, tag or commit to compare against, through its merge base with HEAD.
    pub base: String,
    /// Git pathspecs to measure, such as `src` or `:!src/generated`. Empty: the whole repository.
    pub paths: Vec<String>,
    pub cwd: PathBuf,
    pub gate: GateOptions,
}

pub fn check(options: &CheckOptions, tools: &Tools) -> Result<CompareResult, String> {
    let root = PathBuf::from(git(&options.cwd, &["rev-parse", "--show-toplevel"], None)?.trim());
    let base_commit = git(&root, &["merge-base", &options.base, "HEAD"], None)?
        .trim()
        .to_string();
    let mut pathspec = vec!["--"];
    if options.paths.is_empty() {
        pathspec.push(".");
    } else {
        pathspec.extend(options.paths.iter().map(String::as_str));
    }

    let head_files: Vec<PathBuf> = nul_list(&git(
        &root,
        &[
            &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            pathspec.as_slice(),
        ]
        .concat(),
        None,
    )?)
    .into_iter()
    .filter(|f| is_source_file(f))
    .map(|f| root.join(f))
    .filter(|p| p.exists())
    .collect();

    let base_root = tempfile::Builder::new()
        .prefix("autoratchet-base-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    // A throwaway index of the base commit lets `ls-files` apply the same pathspecs,
    // `:!` exclusions included, to both sides.
    let index = base_root.path().join(".autoratchet-index");
    git(&root, &["read-tree", &base_commit], Some(&index))?;
    let base_files: Vec<String> = nul_list(&git(
        &root,
        &[&["ls-files", "-z"], pathspec.as_slice()].concat(),
        Some(&index),
    )?)
    .into_iter()
    .filter(|f| is_source_file(f))
    .collect();
    std::fs::remove_file(&index).map_err(|e| e.to_string())?;
    extract(&root, &base_commit, &base_files, base_root.path())?;

    let base = Source::Files {
        root: base_root.path().to_path_buf(),
        files: base_files.iter().map(|f| base_root.path().join(f)).collect(),
    };
    let head = Source::Files {
        root: root.clone(),
        files: head_files,
    };
    let mut result = compare_sources(&base, &head, &options.gate, tools)?;
    result.base_commit = Some(base_commit);
    Ok(result)
}
