//! Go: every function and method, their cyclomatic and cognitive complexity, and override comments,
//! from one tree-sitter parse. Names follow gocyclo and gocognit: `Parse`, `(*Server).Handle`,
//! `(Point).String`. Function literals count toward the function they sit in, as in both tools.
//!
//! Cyclomatic complexity follows gocyclo: 1, plus each `if`, `for` (and `range`), non-default
//! `case` in a `switch` or `select`, `&&` and `||`.
//!
//! Cognitive complexity follows gocognit (SonarSource's specification): `if`, `switch`, `select`
//! and loops cost 1 plus their nesting; `else if` and `else` cost 1; function literals nest; each
//! run of one logical operator costs 1, through parentheses; labelled `break`, `continue` and
//! `goto` cost 1; and a function that calls itself costs 1 more.

use std::collections::HashSet;

use tree_sitter::{Node, Parser};

use crate::gate::Metric;
use crate::lang::{
    FileCalls, FunctionCalls, MeasuredFunction, NativeAnalysis, attached_overrides, directive_metrics, named_children,
    tree_sitter_error,
};

/// A file's syntax, for lining up revisions; empty when it does not parse.
pub fn syntax(source: &str) -> crate::lang::Syntax {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("Go grammar");
    parser.parse(source, None).map_or_else(Default::default, |tree| {
        crate::lang::tree_syntax(
            tree.root_node(),
            &[
                "interpreted_string_literal",
                "raw_string_literal",
                "rune_literal",
                "int_literal",
                "float_literal",
                "imaginary_literal",
            ],
        )
    })
}

pub fn analyse_go(source: &str) -> NativeAnalysis {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("Go grammar");
    let Some(mut tree) = parser.parse(source, None) else {
        return NativeAnalysis {
            error: Some("could not parse".into()),
            ..Default::default()
        };
    };
    // Go 1.26 lets `new` take a value (`new("tag")`), which this grammar version reads as a type.
    // `nex(…)` has the same length and parses as an ordinary call; a `new` call holds no decisions.
    for all in [false, true] {
        if !tree.root_node().has_error() {
            break;
        }
        if let Some(retry) = parser.parse(new_as_call(source, all), None)
            && !retry.root_node().has_error()
        {
            tree = retry;
        }
    }
    if let Some(error) = tree_sitter_error(tree.root_node()) {
        return NativeAnalysis {
            error: Some(error),
            ..Default::default()
        };
    }
    let comments = crate::lang::comments(tree.root_node(), source);
    let mut out = NativeAnalysis::default();
    let mut names = crate::lang::Names::default();
    let package = named_children(tree.root_node())
        .into_iter()
        .find(|c| c.kind() == "package_clause")
        .and_then(|c| named_children(c).into_iter().next())
        .map_or(String::new(), |n| text(n, source).to_string());
    // gocognit's recursion spans the package: the collector links its files.
    let mut calls = FileCalls {
        scope: package,
        by_function: Vec::new(),
    };
    for f in functions(tree.root_node(), source) {
        let base = match &f.receiver {
            Some((_, typ)) => format!("({typ}).{}", f.name),
            None => f.name.to_string(),
        };
        let name = names.add(base, None);
        let line = f.node.start_position().row as u32 + 1;
        let mut cognitive = Cognitive {
            source,
            nesting: 0,
            score: 0,
            else_ifs: HashSet::new(),
            counted: HashSet::new(),
        };
        cognitive.visit(f.body);
        out.functions.push(MeasuredFunction {
            origin: names.origin(&name),
            name: name.clone(),
            line,
            cyclomatic: 1 + gocyclo(f.body, source),
            cognitive: cognitive.score,
            span: f.node.byte_range(),
        });
        let receiver_name = f.receiver.as_ref().and_then(|(r, _)| r.as_deref());
        let (functions, methods) = callees(f.body, receiver_name, source);
        calls.by_function.push(FunctionCalls {
            name: f.name.to_string(),
            is_method: f.receiver.is_some(),
            functions,
            methods,
        });
        out.overrides
            .extend(attached_overrides(&comments, line, line, &name, comment_metrics));
    }
    out.calls = Some(calls);
    out.findings = crate::slop::go(tree.root_node(), source, &comments);
    out.overrides.extend(crate::slop::tree_lint_overrides(&comments));
    out
}

struct GoFunction<'t, 's> {
    node: Node<'t>,
    body: Node<'t>,
    name: &'s str,
    receiver: Option<(Option<String>, String)>,
}

/// The functions gocyclo reports: function and method declarations, and function literals
/// assigned to package-level variables (`var handler = func(…) {…}`), named after the variable.
fn functions<'t, 's>(root: Node<'t>, source: &'s str) -> Vec<GoFunction<'t, 's>> {
    let mut out = Vec::new();
    for decl in named_children(root) {
        match decl.kind() {
            "function_declaration" | "method_declaration" => {
                if let (Some(body), Some(name)) = (decl.child_by_field_name("body"), decl.child_by_field_name("name")) {
                    out.push(GoFunction {
                        node: decl,
                        body,
                        name: text(name, source),
                        receiver: receiver(decl, source),
                    });
                }
            }
            "var_declaration" => out.extend(package_literals(decl, source)),
            _ => {}
        }
    }
    out
}

/// `var handler = func(…) {…}` at package level: a function named after its variable.
fn package_literals<'t, 's>(decl: Node<'t>, source: &'s str) -> Vec<GoFunction<'t, 's>> {
    let specs = named_children(decl)
        .into_iter()
        .flat_map(|c| {
            if c.kind() == "var_spec_list" {
                named_children(c)
            } else {
                vec![c]
            }
        })
        .filter(|s| s.kind() == "var_spec");
    let mut out = Vec::new();
    for spec in specs {
        let Some(first) = spec.child_by_field_name("name") else {
            continue;
        };
        let values = spec
            .child_by_field_name("value")
            .map(named_children)
            .unwrap_or_default();
        for literal in values.into_iter().filter(|v| v.kind() == "func_literal") {
            if let Some(body) = literal.child_by_field_name("body") {
                out.push(GoFunction {
                    node: literal,
                    body,
                    name: text(first, source),
                    receiver: None,
                });
            }
        }
    }
    out
}

/// `new(value)` → `nex(value)`. Without `all`, `new([]T)`, `new(map[…]…)` and other arguments that
/// start like a type keep `new`; with it, those become calls too (`new([]T{…})` is a value).
fn new_as_call(source: &str, all: bool) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    while let Some(found) = source[i..].find("new(") {
        let at = i + found;
        let rest = source[at + 4..].trim_start();
        let type_argument = ["[", "map[", "struct", "interface", "chan ", "chan<-", "<-chan", "func("]
            .iter()
            .any(|start| rest.starts_with(start));
        if (at == 0 || !ident(bytes[at - 1])) && (all || !type_argument) {
            out[at + 2] = b'x';
        }
        i = at + 4;
    }
    String::from_utf8(out).expect("one ASCII byte replaced")
}

fn text<'s>(node: Node, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

/// A method's receiver: its name, if it has one, and its type as gocyclo prints it (`*Server`).
fn receiver(decl: Node, source: &str) -> Option<(Option<String>, String)> {
    let list = decl.child_by_field_name("receiver")?;
    let parameter = named_children(list)
        .into_iter()
        .find(|p| p.kind() == "parameter_declaration")?;
    let name = parameter
        .child_by_field_name("name")
        .map(|n| text(n, source).to_string());
    Some((name, receiver_type(parameter.child_by_field_name("type")?, source)))
}

fn receiver_type(node: Node, source: &str) -> String {
    match node.kind() {
        "pointer_type" => {
            format!(
                "*{}",
                named_children(node)
                    .first()
                    .map_or(String::new(), |t| receiver_type(*t, source))
            )
        }
        "generic_type" => node
            .child_by_field_name("type")
            .map_or(String::new(), |t| receiver_type(t, source)),
        "parenthesized_type" => named_children(node)
            .first()
            .map_or(String::new(), |t| receiver_type(*t, source)),
        _ => text(node, source).to_string(),
    }
}

fn is_logical(node: Node, source: &str) -> bool {
    node.kind() == "binary_expression"
        && node
            .child_by_field_name("operator")
            .is_some_and(|op| matches!(text(op, source), "&&" | "||"))
}

/// gocyclo's decision points: `if`, `for`, non-default cases, `&&` and `||`, function literals included.
fn gocyclo(node: Node, source: &str) -> u32 {
    let own = match node.kind() {
        "if_statement" | "for_statement" | "expression_case" | "type_case" | "communication_case" => 1,
        "binary_expression" if is_logical(node, source) => 1,
        _ => 0,
    };
    own + named_children(node)
        .into_iter()
        .map(|c| gocyclo(c, source))
        .sum::<u32>()
}

struct Cognitive<'s> {
    source: &'s str,
    nesting: u32,
    score: u32,
    /// `else if` statements: they cost 1, without nesting.
    else_ifs: HashSet<usize>,
    /// Logical expressions already counted as part of an outer one.
    counted: HashSet<usize>,
}

impl Cognitive<'_> {
    fn nested(&mut self, node: Option<Node>) {
        self.nesting += 1;
        if let Some(node) = node {
            self.visit(node);
        }
        self.nesting -= 1;
    }

    fn structure(&mut self) {
        self.score += 1 + self.nesting;
    }

    fn visit(&mut self, node: Node) {
        let field = |name: &str| node.child_by_field_name(name);
        match node.kind() {
            "if_statement" => {
                if self.else_ifs.contains(&node.id()) {
                    self.score += 1
                } else {
                    self.structure()
                }
                field("initializer")
                    .into_iter()
                    .chain(field("condition"))
                    .for_each(|n| self.visit(n));
                self.nested(field("consequence"));
                match field("alternative") {
                    Some(alt) if alt.kind() == "block" => {
                        self.score += 1;
                        self.nested(Some(alt));
                    }
                    Some(alt) => {
                        self.else_ifs.insert(alt.id());
                        self.visit(alt);
                    }
                    None => {}
                }
            }
            "expression_switch_statement" | "type_switch_statement" | "select_statement" => {
                self.structure();
                let (cases, header): (Vec<Node>, Vec<Node>) = named_children(node)
                    .into_iter()
                    .partition(|c| c.kind().ends_with("_case"));
                header.into_iter().for_each(|h| self.visit(h));
                self.nesting += 1;
                cases.into_iter().for_each(|c| self.visit(c));
                self.nesting -= 1;
            }
            "for_statement" => {
                self.structure();
                let body = field("body");
                for child in named_children(node).into_iter().filter(|c| Some(*c) != body) {
                    self.visit(child);
                }
                self.nested(body);
            }
            "func_literal" => {
                field("parameters")
                    .into_iter()
                    .chain(field("result"))
                    .for_each(|n| self.visit(n));
                self.nested(field("body"));
            }
            "break_statement" | "continue_statement" | "goto_statement" => {
                if named_children(node).iter().any(|c| c.kind() == "label_name") {
                    self.score += 1;
                }
            }
            "binary_expression" => {
                if is_logical(node, self.source) && !self.counted.contains(&node.id()) {
                    let mut operators = Vec::new();
                    self.collect(node, &mut operators);
                    self.score += operators.windows(2).filter(|w| w[0] != w[1]).count() as u32 + 1;
                }
                named_children(node).into_iter().for_each(|c| self.visit(c));
            }
            _ => named_children(node).into_iter().for_each(|c| self.visit(c)),
        }
    }

    /// The logical operators of an expression in source order, through parentheses and through
    /// other binary operators, as gocognit collects them.
    fn collect(&mut self, node: Node, out: &mut Vec<String>) {
        self.counted.insert(node.id());
        match node.kind() {
            "parenthesized_expression" => {
                if let Some(inner) = named_children(node).into_iter().next() {
                    self.collect(inner, out);
                }
            }
            "binary_expression" => {
                if let Some(left) = node.child_by_field_name("left") {
                    self.collect(left, out);
                }
                if is_logical(node, self.source) {
                    out.push(text(node.child_by_field_name("operator").expect("operator"), self.source).to_string());
                }
                if let Some(right) = node.child_by_field_name("right") {
                    self.collect(right, out);
                }
            }
            _ => {}
        }
    }
}

/// What a function calls, as gocognit resolves calls without type information: plain functions
/// by name (`Name(…)`, `Name[T](…)`), and a method only through its own receiver (`recv.Name(…)`).
fn callees(body: Node, receiver: Option<&str>, source: &str) -> (HashSet<String>, HashSet<String>) {
    let (mut functions, mut methods) = (HashSet::new(), HashSet::new());
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if let Some(target) = call_target(node) {
            match target.kind() {
                "identifier" => {
                    functions.insert(text(target, source).to_string());
                }
                "selector_expression" => {
                    if let Some(method) = own_method(target, receiver, source) {
                        methods.insert(method.to_string());
                    }
                }
                _ => {}
            }
        }
        stack.extend(named_children(node));
    }
    (functions, methods)
}

/// What a call expression calls, without parentheses or type arguments: `(F)(x)` and `F[T](x)` call `F`.
fn call_target(node: Node) -> Option<Node> {
    if node.kind() != "call_expression" {
        return None;
    }
    let mut target = node.child_by_field_name("function")?;
    while matches!(
        target.kind(),
        "parenthesized_expression" | "index_expression" | "generic_type"
    ) {
        let Some(inner) = target
            .child_by_field_name("operand")
            .or_else(|| named_children(target).into_iter().next())
        else {
            break;
        };
        target = inner;
    }
    Some(target)
}

/// `recv.Name` inside a method whose receiver is `recv`: the method name.
fn own_method<'s>(selector: Node, receiver: Option<&str>, source: &'s str) -> Option<&'s str> {
    let through_receiver = selector
        .child_by_field_name("operand")
        .is_some_and(|o| Some(text(o, source)) == receiver);
    through_receiver
        .then(|| selector.child_by_field_name("field").map(|f| text(f, source)))
        .flatten()
}

/// `//nolint:gocyclo,gocognit` (also `cyclop`) and `//gocognit:ignore`, plus `// autoratchet-disable-next-line`.
fn comment_metrics(comment: &str) -> Vec<Metric> {
    let mut metrics = directive_metrics(comment);
    let compact = comment.replace(' ', "");
    if let Some(linters) = compact.strip_prefix("//nolint:") {
        let names: Vec<&str> = linters.split([',', '/']).collect();
        if names.iter().any(|n| matches!(*n, "gocyclo" | "cyclop")) && !metrics.contains(&Metric::Cyclomatic) {
            metrics.push(Metric::Cyclomatic);
        }
        if names.contains(&"gocognit") && !metrics.contains(&Metric::Cognitive) {
            metrics.push(Metric::Cognitive);
        }
    }
    if compact.starts_with("//gocognit:ignore") && !metrics.contains(&Metric::Cognitive) {
        metrics.push(Metric::Cognitive);
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(source: &str) -> Vec<(String, u32, u32)> {
        let a = analyse_go(source);
        assert!(a.error.is_none(), "{:?}", a.error);
        a.functions
            .into_iter()
            .map(|f| (f.name, f.cyclomatic, f.cognitive))
            .collect()
    }

    #[test]
    fn matches_gocyclo_and_gocognit() {
        // gocyclo 9, gocognit 13 for this function.
        let src = "package p\n\nfunc F(a, b, c, d bool, xs []int) int {\n\tif a && (b || c) && d {\n\t\treturn 1\n\t} else if b {\n\t\treturn 2\n\t} else {\n\t\tgo func() {\n\t\t\tif c {\n\t\t\t\treturn\n\t\t\t}\n\t\t}()\n\t}\nouter:\n\tfor _, x := range xs {\n\t\tselect {\n\t\tcase <-make(chan int):\n\t\t\tcontinue outer\n\t\tdefault:\n\t\t}\n\t\t_ = x\n\t}\n\treturn 0\n}\n";
        assert_eq!(scores(src), [("F".into(), 9, 13)]);
    }

    #[test]
    fn names_and_value_new() {
        let src = "package p\n\ntype S[T any] struct{}\n\nfunc (s *S[T]) M() { _ = new(\"tag\") }\nfunc (S[T]) N() {}\nvar H = func() {}\n";
        let names: Vec<String> = scores(src).into_iter().map(|(n, ..)| n).collect();
        assert_eq!(names, ["(*S).M", "(S).N", "H"]);
    }

    #[test]
    fn overrides() {
        let src = "package p\n\n//nolint:gocyclo,gocognit // reason\nfunc A() {}\n\n//gocognit:ignore\nfunc B() {}\n\n//nolint:errcheck\nfunc C() {}\n";
        let found: Vec<_> = analyse_go(src)
            .overrides
            .into_iter()
            .map(|o| (o.function.unwrap(), o.metric))
            .collect();
        assert_eq!(
            found,
            [
                ("A".into(), Metric::Cyclomatic),
                ("A".into(), Metric::Cognitive),
                ("B".into(), Metric::Cognitive)
            ]
        );
    }

    #[test]
    fn directives_inside_strings_are_not_overrides() {
        let src = "package p\n\nfunc F(s string) string { return \"//nolint:gocognit // example\" }\n\nvar raw = `\n//gocognit:ignore\n`\nfunc G() {}\n";
        assert!(analyse_go(src).overrides.is_empty());
    }
}
