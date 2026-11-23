//! What the in-process language analysers (Rust, Python, Go) report for one file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::gate::Metric;

/// What a function's name comes from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Origin {
    /// The name without the number it, or a function it is in, got for repeating one.
    pub source_name: String,
    /// The function it is in, as parsed: a `>` in a name may be its own, as a method can be
    /// written `"run>2"`.
    pub outer: Option<String>,
}

/// Names for a file's functions, in order: one that repeats gets `#2`, `#3`, ….
#[derive(Default)]
pub struct Names {
    repeats: HashMap<String, u32>,
    /// What each name given comes from.
    pub origins: HashMap<String, Origin>,
}

impl Names {
    /// The name of a function that would be called `base`, in function `outer`.
    pub fn add(&mut self, base: String, outer: Option<&str>) -> String {
        let own = |outer: &str| base.strip_prefix(outer).filter(|own| own.starts_with('>'));
        let outer = outer.and_then(|outer| Some((outer, own(outer)?)));
        let source_name = outer.map_or(base.clone(), |(outer, own)| self.origin(outer).source_name + own);
        let origin = Origin {
            source_name,
            outer: outer.map(|(outer, _)| outer.to_string()),
        };
        let repeats = self.repeats.entry(base.clone()).or_default();
        let mut name = base.clone();
        // A method can be written `"run#2"`: that name is taken too.
        while self.origins.contains_key(&name) {
            *repeats = (*repeats).max(1) + 1;
            name = format!("{base}#{repeats}");
        }
        self.origins.insert(name.clone(), origin);
        name
    }

    pub fn origin(&self, name: &str) -> Origin {
        self.origins.get(name).cloned().unwrap_or_default()
    }
}

pub struct MeasuredFunction {
    pub name: String,
    pub origin: Origin,
    pub line: u32,
    pub cyclomatic: u32,
    pub cognitive: u32,
    /// Byte range of the function, so lint findings inside it count against it.
    pub span: std::ops::Range<usize>,
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
    /// Anti-slop findings anywhere in the file.
    pub findings: Vec<crate::slop::NativeFinding>,
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

/// The rules an `autoratchet-disable-next-line` comment names, up to the `--` before its reason.
pub fn directive_rules(comment: &str) -> Vec<&str> {
    let Some(caps) = DIRECTIVE_RE.captures(comment.trim()) else {
        return Vec::new();
    };
    let rules = caps.get(1).map_or("", |m| m.as_str()).split("--").next().unwrap_or("");
    rules
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect()
}

/// The complexity metrics an `autoratchet-disable-next-line` comment names.
pub fn directive_metrics(comment: &str) -> Vec<Metric> {
    let names = directive_rules(comment);
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

/// What lining up two revisions of a file needs of its syntax, by byte.
#[derive(Default)]
pub struct Syntax {
    pub comments: Vec<std::ops::Range<usize>>,
    /// Literals, whose contents are code, spaces and all.
    pub literals: Vec<std::ops::Range<usize>>,
    /// Every syntax node: expressions, statements and what they are made of.
    pub nodes: Vec<std::ops::Range<usize>>,
    /// Those of them that are statements, or what holds statements.
    pub scopes: Vec<(std::ops::Range<usize>, Role)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    Function,
    /// The statements of a function, branch or loop, together.
    Block,
    /// A statement, a declaration, or a part of one that holds a block: `else`, `case`, `except`.
    Statement,
}

impl Role {
    fn of(kind: &str) -> Option<Role> {
        let kind = kind.to_ascii_lowercase();
        let ends = |with: &[&str]| with.iter().any(|end| kind.ends_with(end));
        let function = ["function", "method", "lambda", "func_literal"]
            .iter()
            .any(|k| kind.contains(k));
        if kind.contains("parameter") || ends(&["type", "identifier"]) {
            None
        } else if function && ends(&["expression", "declaration", "definition", "lambda", "func_literal"]) {
            Some(Role::Function)
        } else if ends(&["block", "blockstatement", "classbody"]) {
            Some(Role::Block)
        } else if ends(&["statement", "declaration", "definition", "clause", "case"]) {
            Some(Role::Statement)
        } else {
            None
        }
    }
}

impl Syntax {
    pub fn node(&mut self, kind: &str, span: std::ops::Range<usize>) {
        if let Some(scope) = Role::of(kind) {
            self.scopes.push((span.clone(), scope));
        }
        self.nodes.push(span);
    }
}

/// A tree-sitter tree's syntax, given the kinds its literals have.
pub fn tree_syntax(root: tree_sitter::Node, literals: &[&str]) -> Syntax {
    let mut syntax = Syntax::default();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            syntax.comments.push(node.byte_range());
            continue;
        }
        if node.is_named() {
            syntax.node(node.kind(), node.byte_range());
        }
        if literals.contains(&node.kind()) {
            syntax.literals.push(node.byte_range());
            continue;
        }
        stack.extend(node.children(&mut cursor).collect::<Vec<_>>());
    }
    syntax
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
    fn a_repeated_name_gets_a_number_no_name_has() {
        let mut names = Names::default();
        let given = [
            ("run", None),
            ("run#2", None),
            ("run", None),
            ("run>a", Some("run")),
            ("run#3>a", Some("run#3")),
            ("run#2", None),
            // A name with a `>` of its own, in no function, and one that says nothing of its.
            ("run>a", None),
            ("C.m", Some("run")),
        ];
        let added = given.map(|(base, outer)| names.add(base.into(), outer));
        let expected = ["run", "run#2", "run#3", "run>a", "run#3>a", "run#2#2", "run>a#2", "C.m"];
        assert_eq!(added, expected);
        // A name written `run#2` is its own; a number, the function's or its outer one's, is not.
        let origins = added.map(|name| names.origin(&name));
        let sources = origins.each_ref().map(|o| o.source_name.as_str());
        assert_eq!(
            sources,
            ["run", "run#2", "run", "run>a", "run>a", "run#2", "run>a", "C.m"]
        );
        let outers = origins.each_ref().map(|o| o.outer.as_deref());
        assert_eq!(outers, [None, None, None, Some("run"), Some("run#3"), None, None, None]);
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
