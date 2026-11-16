//! What the in-process language analysers (Rust, Python, Go) report for one file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::gate::Metric;

pub struct MeasuredFunction {
    pub name: String,
    pub line: u32,
    pub cyclomatic: u32,
    pub cognitive: u32,
}

/// An override on a function, or (with no function) one that covers a whole file or module.
pub struct NativeOverride {
    pub line: u32,
    pub text: String,
    pub metric: Metric,
    pub function: Option<String>,
}

#[derive(Default)]
pub struct NativeAnalysis {
    pub functions: Vec<MeasuredFunction>,
    pub overrides: Vec<NativeOverride>,
    pub error: Option<String>,
    /// What each function calls, for a language whose recursion spans files (Go: a package).
    pub calls: Option<FileCalls>,
}

/// A file's scope (Go: its package name) and what each of its functions calls, in the order of
/// `NativeAnalysis::functions`.
pub struct FileCalls {
    pub scope: String,
    pub by_function: Vec<FunctionCalls>,
}

pub struct FunctionCalls {
    pub name: String,
    pub is_method: bool,
    /// Functions called by bare name.
    pub functions: HashSet<String>,
    /// Methods called through the function's own receiver.
    pub methods: HashSet<String>,
}

/// The files that share one scope: the same directory and scope name.
#[derive(PartialEq, Eq, Hash)]
struct Scope<'a> {
    dir: &'a str,
    name: &'a str,
}

/// For files measured with `FileCalls`: each function on a call cycle within its scope (a function
/// calling itself, or functions calling each other, in any of the scope's files) costs 1 more.
/// A method is on a cycle only by calling itself through its receiver, as in gocognit.
pub fn add_cross_file_recursion(analyses: &mut [(String, NativeAnalysis)]) {
    let mut scopes: HashMap<Scope, Vec<usize>> = HashMap::new();
    for (i, (path, analysis)) in analyses.iter().enumerate() {
        if let Some(calls) = &analysis.calls {
            let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            scopes
                .entry(Scope {
                    dir,
                    name: &calls.scope,
                })
                .or_default()
                .push(i);
        }
    }
    let on_cycle: Vec<(usize, usize)> = scopes.values().flat_map(|files| recursive(analyses, files)).collect();
    for (file, function) in on_cycle {
        analyses[file].1.functions[function].cognitive += 1;
    }
}

/// The (file, function) pairs on a call cycle among `files`.
fn recursive(analyses: &[(String, NativeAnalysis)], files: &[usize]) -> Vec<(usize, usize)> {
    let calls = |f: usize| analyses[f].1.calls.as_ref().map_or(&[][..], |c| &c.by_function[..]);
    let nodes: Vec<(usize, usize)> = files
        .iter()
        .flat_map(|&f| (0..calls(f).len()).map(move |i| (f, i)))
        .collect();
    let info = |n: usize| &calls(nodes[n].0)[nodes[n].1];
    let edges = |n: usize| -> Vec<usize> {
        let from = info(n);
        (0..nodes.len())
            .filter(|&m| {
                let to = info(m);
                if to.is_method {
                    m == n && from.methods.contains(&to.name)
                } else {
                    from.functions.contains(&to.name)
                }
            })
            .collect()
    };
    let on_cycle = |start: usize| {
        let mut seen = HashSet::new();
        let mut stack = edges(start);
        while let Some(next) = stack.pop() {
            if next == start {
                return true;
            }
            if seen.insert(next) {
                stack.extend(edges(next));
            }
        }
        false
    };
    (0..nodes.len()).filter(|&n| on_cycle(n)).map(|n| nodes[n]).collect()
}

/// `// autoratchet-disable-next-line complexity, cognitive-complexity -- reason`, or with `#`.
static DIRECTIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?://|#)\s*autoratchet-disable-next-line\s+([^\n]*)").unwrap());

/// The metrics an `autoratchet-disable-next-line` comment names.
pub fn directive_metrics(comment: &str) -> Vec<Metric> {
    let Some(caps) = DIRECTIVE_RE.captures(comment.trim()) else {
        return Vec::new();
    };
    let rules = caps[1].split("--").next().unwrap_or("");
    let names: Vec<&str> = rules
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    let mut metrics = Vec::new();
    if names.contains(&"complexity") {
        metrics.push(Metric::Cyclomatic);
    }
    if names
        .iter()
        .any(|r| matches!(*r, "cognitive-complexity" | "clippy::cognitive_complexity"))
    {
        metrics.push(Metric::Cognitive);
    }
    metrics
}

/// A syntax error from a tree-sitter parse: the first error or missing node, with its line.
pub fn tree_sitter_error(root: tree_sitter::Node) -> Option<String> {
    if !root.has_error() {
        return None;
    }
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let what = if node.is_missing() {
                format!("missing {}", node.kind())
            } else {
                "unexpected syntax".into()
            };
            return Some(format!("{what} (line {})", node.start_position().row + 1));
        }
        let children: Vec<_> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    Some("syntax error".into())
}

/// A comment the parser found: its line, whether it starts the line, and its text. Text inside a
/// string literal is never a comment.
pub struct Comment<'s> {
    pub line: u32,
    pub starts_line: bool,
    pub text: &'s str,
}

/// Override comments on a function: one on its `def` or `func` line, and each comment on its own
/// line directly above its first line (decorators included), however many are stacked.
pub fn attached_overrides(
    comments: &std::collections::HashMap<u32, Comment>,
    def_line: u32,
    first_line: u32,
    name: &str,
    metrics: fn(&str) -> Vec<Metric>,
) -> Vec<NativeOverride> {
    let mut attached: Vec<&Comment> = comments.get(&def_line).into_iter().collect();
    let mut above = first_line;
    while let Some(comment) = above
        .checked_sub(1)
        .and_then(|line| comments.get(&line))
        .filter(|c| c.starts_line)
    {
        attached.push(comment);
        above -= 1;
    }
    let mut found: Vec<NativeOverride> = attached
        .into_iter()
        .flat_map(|c| {
            metrics(c.text).into_iter().map(|metric| NativeOverride {
                line: c.line,
                text: c.text.to_string(),
                metric,
                function: Some(name.to_string()),
            })
        })
        .collect();
    found.sort_by_key(|o| o.line);
    found
}

/// Named children, without comments: a comment can sit anywhere, even inside an expression.
pub fn named_children(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .collect()
}

/// Every comment in a tree-sitter tree, by line.
pub fn comments<'s>(root: tree_sitter::Node, source: &'s str) -> std::collections::HashMap<u32, Comment<'s>> {
    let mut found = std::collections::HashMap::new();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            let line_start = source[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
            let starts_line = source[line_start..node.start_byte()].trim().is_empty();
            let line = node.start_position().row as u32 + 1;
            found.entry(line).or_insert(Comment {
                line,
                starts_line,
                text: source[node.byte_range()].trim(),
            });
            continue;
        }
        stack.extend(node.children(&mut cursor).collect::<Vec<_>>());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cognitive(analyses: &[(String, NativeAnalysis)]) -> Vec<(&str, u32)> {
        analyses
            .iter()
            .flat_map(|(_, a)| a.functions.iter().map(|f| (f.name.as_str(), f.cognitive)))
            .collect()
    }

    #[test]
    fn recursion_spans_the_files_of_one_scope() {
        let go = |path: &str, source: &str| (path.to_string(), crate::go::analyse_go(source));
        let mut analyses = vec![
            go("p/a.go", "package p\nfunc A() { B() }\n"),
            go("p/b.go", "package p\nfunc B() { A() }\nfunc C() { A() }\n"),
            // Same package name, other directory: another package.
            go("q/a.go", "package p\nfunc D() { A() }\nfunc A() {}\n"),
        ];
        add_cross_file_recursion(&mut analyses);
        assert_eq!(cognitive(&analyses), [("A", 1), ("B", 1), ("C", 0), ("D", 0), ("A", 0)]);
    }
}
